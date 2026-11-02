//! Episodes and rich details, fetched from the provider once per title and
//! kept in the catalog, with TVmaze filling gaps in series details.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use xorvio_model::{DEMO_SOURCE_ID, Episode, MediaDetails, MediaKind, Movie, Series, SourceKind, StreamLocator, XtreamPath};

use crate::details::DetailsRecord;
use crate::error::SourceError;
use crate::sources::xtream::{self, XtreamClient};
use crate::sources::{tvmaze, xtream_details};
use crate::store::lock;
use crate::tmdb::{self, TitleLookup, TmdbTitle};
use crate::{Core, Result, names, now};

/// Spaces out requests to a provider's detail endpoints, so browsing never
/// looks like a burst of API calls: at most two at once, 350 ms apart
/// (TMDB, which allows far more, sets its own).
pub(crate) struct ProviderTurns {
    running: Arc<Semaphore>,
    spacing: Duration,
    last_start: Mutex<Option<Instant>>,
}

const SPACING: Duration = Duration::from_millis(350);

impl ProviderTurns {
    fn new() -> Self {
        Self::with(2, SPACING)
    }

    /// Up to `at_once` requests at a time, starting at least `spacing` apart.
    pub(crate) fn with(at_once: usize, spacing: Duration) -> Self {
        Self { running: Arc::new(Semaphore::new(at_once)), spacing, last_start: Mutex::new(None) }
    }

    pub(crate) async fn wait(&self) -> Option<OwnedSemaphorePermit> {
        let permit = self.running.clone().acquire_owned().await.ok()?;
        let start = {
            let mut last = lock(&self.last_start);
            let now = Instant::now();
            let start = last.map_or(now, |previous| (previous + self.spacing).max(now));
            *last = Some(start);
            start
        };
        tokio::time::sleep_until(start).await;
        Some(permit)
    }
}

impl Core {
    /// A series' episodes, with TMDB's stills and synopses where the source
    /// has none.
    pub async fn episodes(&self, series_id: &str) -> Result<Vec<Episode>> {
        let Some(series) = self.catalog.series_by_ids(&[series_id.to_string()])?.pop() else { return Ok(Vec::new()) };
        let episodes = self.source_episodes(&series).await?;
        Ok(self.with_tmdb_episodes(&series, episodes).await)
    }

    /// A series' episodes from its source. Xtream providers send them only on
    /// request, so the first ask each session fetches them (with the series'
    /// details); after that, and whenever the provider can't be reached, the
    /// stored ones are used.
    pub(crate) async fn source_episodes(&self, series: &Series) -> Result<Vec<Episode>> {
        let series_id = series.id.as_str();
        let cached = self.catalog.episodes(series_id)?;
        let is_xtream = self.config(&series.source_id).is_ok_and(|config| config.kind == SourceKind::Xtream);
        let Some(remote_id) = series.remote_id.clone().filter(|_| is_xtream) else { return Ok(cached) };
        if !cached.is_empty() && lock(&self.episodes_fetched).contains(series_id) {
            return Ok(cached);
        }

        let item_lock = self.item_lock(series_id);
        let _held = item_lock.lock().await;
        // Someone else may have fetched them while this waited.
        if lock(&self.episodes_fetched).contains(series_id) {
            return self.catalog.episodes(series_id);
        }
        match self.fetch_series_info(series, &remote_id).await {
            Ok(episodes) if !episodes.is_empty() => Ok(episodes),
            Ok(_) => Ok(cached),
            // Keep showing what's there when the provider is unreachable.
            Err(error) if !cached.is_empty() => {
                tracing::warn!("episodes could not be refreshed: {}", crate::error::describe(&error));
                Ok(cached)
            }
            Err(error) => Err(error),
        }
    }

    /// Rich details for a film or series. A provider's detail endpoint is
    /// asked once per title, and TMDB's details (with a key) are laid over
    /// them, or under a media server's. Series TMDB can't help with have
    /// gaps (backdrops, cast with photos) filled from TVmaze, which never
    /// replaces the provider's own data. Titles that aren't in the catalog
    /// have empty details.
    pub async fn details(&self, item_id: &str) -> Result<MediaDetails> {
        let mut tmdb = None;
        let mut on_server = false;
        let mut in_folder = false;
        if let Some(movie) = self.catalog.movies_by_ids(&[item_id.to_string()])?.pop() {
            if let StreamLocator::Server { key, .. } = &movie.stream {
                self.fetch_server_details(&movie.source_id, MediaKind::Movie, &movie.id, key).await;
                on_server = true;
            } else {
                self.fetch_movie_details(&movie).await;
            }
            in_folder = self.is_folder_source(&movie.source_id);
            tmdb = self.tmdb_title(&TitleLookup::of_movie(&movie)).await;
        } else if let Some(series) = self.catalog.series_by_ids(&[item_id.to_string()])?.pop() {
            on_server = self.config(&series.source_id).is_ok_and(|config| config.kind.is_media_server());
            in_folder = self.is_folder_source(&series.source_id);
            tmdb = match series.remote_id.as_deref().filter(|_| on_server) {
                Some(key) => {
                    self.fetch_server_details(&series.source_id, MediaKind::Series, &series.id, key).await;
                    self.tmdb_title(&TitleLookup::of_series(&series)).await
                }
                None => self.fetch_series_details(&series).await,
            };
        }
        let stored = self.catalog.details(item_id)?.map(|(record, _)| record).unwrap_or_default();
        let details = match &tmdb {
            // A media folder's own .nfo files and artwork come first too.
            Some(title) if in_folder => crate::folders::tmdb_under_folder(stored, &title.details),
            Some(title) => tmdb::overlay(stored, title, on_server),
            None => stored,
        };
        Ok(details.into_model())
    }

    /// The provider's `get_vod_info` for an Xtream film, once.
    async fn fetch_movie_details(&self, movie: &Movie) {
        let StreamLocator::Xtream { path: XtreamPath::Movie, stream_id, .. } = &movie.stream else { return };
        let Ok(config) = self.config(&movie.source_id) else { return };
        if config.kind != SourceKind::Xtream || self.was_fetched(&movie.id) {
            return;
        }
        let Ok(credentials) = self.credentials(&movie.source_id) else { return };
        let item_lock = self.item_lock(&movie.id);
        let _held = item_lock.lock().await;
        if self.was_fetched(&movie.id) {
            return;
        }
        let _turn = self.provider_turn(&movie.source_id).await;
        let client = XtreamClient { http: &self.http, credentials: &credentials, user_agent: config.user_agent.as_deref() };
        let info = match client.vod_info(stream_id).await {
            Ok(root) => xtream_details::movie_info(&root),
            Err(error) => {
                tracing::warn!("film details could not be fetched: {error}");
                return;
            }
        };
        let info = info.unwrap_or_default();
        let year = info.details.release_year();
        let saved = self
            .catalog
            .update_movie(&movie.id, info.plot.as_deref(), info.genre.as_deref(), info.duration_seconds, year)
            .and_then(|()| self.catalog.save_details(&movie.source_id, &BTreeMap::from([(movie.id.clone(), info.details)]), Some(now())));
        if let Err(error) = saved {
            tracing::warn!("film details could not be saved: {error}");
        }
    }

    /// The provider's series record (once a session, with the episodes),
    /// then TMDB's, or TVmaze for what's still missing when TMDB can't help.
    async fn fetch_series_details(&self, series: &Series) -> Option<TmdbTitle> {
        let is_xtream = self.config(&series.source_id).is_ok_and(|config| config.kind == SourceKind::Xtream);
        if is_xtream && series.remote_id.is_some() && !self.was_fetched(&series.id) {
            // Fetching episodes stores the series' details too.
            let _ = self.source_episodes(series).await;
        }
        let tmdb = self.tmdb_title(&TitleLookup::of_series(series)).await;
        if tmdb.is_none() {
            self.supplement_from_tvmaze(series).await;
        }
        tmdb
    }

    async fn supplement_from_tvmaze(&self, series: &Series) {
        if series.source_id == DEMO_SOURCE_ID || !self.tvmaze_enabled.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let existing = self.catalog.details(&series.id).ok().flatten().map(|(record, _)| record).unwrap_or_default();
        if !existing.backdrop_urls.is_empty() && !existing.people.is_empty() {
            return;
        }
        if !lock(&self.tvmaze_tried).insert(series.id.clone()) {
            return;
        }
        let title = names::media_title(&series.title);
        let base = lock(&self.tvmaze_base).clone();
        let _turn = self.provider_turn("tvmaze").await;
        let extra = match tvmaze::series_details(&self.http, &base, &title, series.year).await {
            Ok(Some(extra)) => extra,
            Ok(None) => return,
            Err(error) => {
                tracing::debug!("TVmaze had nothing for a series: {error}");
                return;
            }
        };
        // Read again: the provider's own details may have arrived meanwhile.
        let existing = self.catalog.details(&series.id).ok().flatten().map(|(record, _)| record).unwrap_or_default();
        let combined = existing.filling_from(&extra);
        if let Err(error) = self.catalog.save_details(&series.source_id, &BTreeMap::from([(series.id.clone(), combined)]), None) {
            tracing::warn!("series details could not be saved: {error}");
        }
    }

    /// Fetches `get_series_info`, stores the episodes and details, and
    /// returns the episodes.
    async fn fetch_series_info(&self, series: &Series, remote_id: &str) -> Result<Vec<Episode>> {
        let config = self.config(&series.source_id)?;
        let credentials = self.credentials(&series.source_id)?;
        let _turn = self.provider_turn(&series.source_id).await;
        let client = XtreamClient { http: &self.http, credentials: &credentials, user_agent: config.user_agent.as_deref() };
        let info = client.series_info(remote_id).await.map_err(anyhow::Error::from)?;
        if info.as_object().is_none() {
            return Err(SourceError::InvalidPlaylist("the provider sent no information for this series".into()).into());
        }
        let episodes = xtream::episodes(&info, series);
        let details: DetailsRecord = xtream_details::from_series_info(&info);
        if !episodes.is_empty() {
            self.catalog.replace_episodes(&series.id, &episodes)?;
            // Only now are these episodes in the catalog for Trakt's watched marks.
            self.apply_trakt_to_episodes(series, &episodes);
        }
        self.catalog.save_details(&series.source_id, &BTreeMap::from([(series.id.clone(), details)]), Some(now()))?;
        lock(&self.episodes_fetched).insert(series.id.clone());
        Ok(episodes)
    }

    /// A media server's full details for a film or series, once. They're
    /// merged over what its lists carried.
    async fn fetch_server_details(&self, source_id: &str, kind: MediaKind, item_id: &str, key: &str) {
        if self.was_fetched(item_id) {
            return;
        }
        let Ok(session) = self.server_session(source_id) else { return };
        let item_lock = self.item_lock(item_id);
        let _held = item_lock.lock().await;
        if self.was_fetched(item_id) {
            return;
        }
        let fetched = match crate::sources::server::fetch_details(&self.http, &session, source_id, kind, key).await {
            Ok(details) => details,
            Err(error) => {
                tracing::warn!("details could not be fetched from the server: {}", crate::redact::redact(&error.to_string(), &session.token.iter().cloned().collect::<Vec<_>>()));
                return;
            }
        };
        let existing = self.catalog.details(item_id).ok().flatten().map(|(record, _)| record).unwrap_or_default();
        let saved = self.catalog.save_details(source_id, &BTreeMap::from([(item_id.to_string(), existing.merged(&fetched))]), Some(now()));
        if let Err(error) = saved {
            tracing::warn!("server details could not be saved: {error}");
        }
    }

    /// Whether a title's details already came from the provider's detail endpoint.
    fn was_fetched(&self, item_id: &str) -> bool {
        matches!(self.catalog.details(item_id), Ok(Some((_, Some(_)))))
    }

    pub(crate) fn item_lock(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.detail_locks).entry(key.to_string()).or_default().clone()
    }

    async fn provider_turn(&self, source_id: &str) -> Option<OwnedSemaphorePermit> {
        let turns = lock(&self.provider_turns).entry(source_id.to_string()).or_insert_with(|| Arc::new(ProviderTurns::new())).clone();
        turns.wait().await
    }
}
