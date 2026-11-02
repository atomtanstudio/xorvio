//! Refreshing a source (the catalog first, then the guide) and the guides
//! added on their own.
//!
//! Everything is downloaded and parsed before anything is written, the
//! catalog is replaced in one transaction, and the guide imports in its own.
//! A failure at any step is recorded in the source's status and never
//! damages what's already there. A refresh can be stopped (see [`Stop`]);
//! what was downloaded is dropped and the catalog stays as it was.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use xorvio_model::{
    DEMO_SOURCE_ID, Program, RefreshProgress, SourceConfig, SourceId, SourceKind, SourceSecrets, SourceStatus, Timestamp,
};

use crate::error::{SourceError, describe};
use crate::http::Downloaded;
use crate::snapshot::{CatalogSnapshot, EMPTY_LIST_THRESHOLD};
use crate::sources::xmltv::{ChannelName, XmltvParser};
use crate::sources::xtream::{XtreamClient, XtreamCredentials};
use crate::sources::{demo, m3u, phase, xtream};
use crate::store::catalog::{CatalogStore, GuideLinks};
use crate::store::lock;
use crate::watchlist::{self, MATCH_WINDOW_SECONDS};
use crate::{Core, Result, now, redact};

/// Guide listings are kept from a week back (for replay) to two weeks ahead.
const GUIDE_DAYS_BACK: i64 = 8;
const GUIDE_DAYS_FORWARD: i64 = 15;

/// Added guides load again once a day; one that failed waits an hour.
const GUIDE_REFRESH_SECONDS: i64 = 24 * 3600;
const GUIDE_RETRY_SECONDS: i64 = 3600;

/// The way to ask each running refresh to stop, by source or guide id.
pub(crate) type RefreshStops = Arc<Mutex<HashMap<String, watch::Sender<bool>>>>;

/// Marks a source as refreshing for as long as it lives, so a refresh that's
/// dropped half-way (the window closed) never leaves the flag behind, and
/// gives it a [`Stop`] that [`Core::cancel_refresh`] can trip.
struct RefreshGuard {
    set: Arc<Mutex<HashSet<String>>>,
    stops: RefreshStops,
    id: String,
}

impl RefreshGuard {
    fn begin(set: &Arc<Mutex<HashSet<String>>>, stops: &RefreshStops, id: &str) -> Option<(Self, Stop)> {
        if !lock(set).insert(id.to_string()) {
            return None;
        }
        let (sender, receiver) = watch::channel(false);
        lock(stops).insert(id.to_string(), sender);
        Some((Self { set: set.clone(), stops: stops.clone(), id: id.to_string() }, Stop(receiver)))
    }
}

impl Drop for RefreshGuard {
    fn drop(&mut self) {
        lock(&self.stops).remove(&self.id);
        lock(&self.set).remove(&self.id);
    }
}

/// A refresh's way of hearing it should stop. It's heard while the refresh
/// waits on the network and between steps, never in the middle of writing:
/// the catalog and the guide each go in one transaction, so a write that
/// has begun finishes, and nothing half-written is ever kept.
#[derive(Clone)]
pub(crate) struct Stop(watch::Receiver<bool>);

impl Stop {
    fn requested(&self) -> bool {
        *self.0.borrow()
    }

    /// `work`'s result, or `None` when a stop is asked for first (and
    /// `work`, with whatever it was downloading, is dropped).
    async fn unless_stopped<T>(&self, work: impl Future<Output = T>) -> Option<T> {
        let mut stop = self.0.clone();
        let stopped = async move {
            // The sender goes only when the refresh is over, so a closed
            // channel never counts as a stop.
            if stop.wait_for(|stop| *stop).await.is_err() {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            biased;
            () = stopped => None,
            value = work => Some(value),
        }
    }
}

type Report<'a> = &'a (dyn Fn(&str) + Send + Sync);

impl Core {
    /// Refreshes a source's catalog, then its guide, reporting each step.
    ///
    /// A provider that can't be reached or refuses the login doesn't make
    /// this fail: the problem is recorded in the returned status (and the
    /// previous catalog stays). It fails only for a source that doesn't exist
    /// or is paused. While a refresh of the same source is already running,
    /// this returns the current status without starting another.
    ///
    /// [`Core::cancel_refresh`] stops it. Until the new catalog is written,
    /// everything downloaded is dropped and the source's catalog and status
    /// stay as they were; after that, the guide is left as it was.
    pub async fn refresh_source(
        &self,
        id: &str,
        progress: &(dyn Fn(RefreshProgress) + Send + Sync),
    ) -> Result<SourceStatus> {
        let config = self.config(id)?;
        if config.is_paused {
            anyhow::bail!("This source is paused. Resume it to refresh it.");
        }
        let Some((_guard, stop)) = RefreshGuard::begin(&self.refreshing, &self.refresh_stops, id) else {
            return Ok(self.catalog.status(id)?.unwrap_or_default());
        };
        let report = |step: &str| {
            progress(RefreshProgress { source_id: id.to_string(), phase: step.to_string(), fraction: None });
        };
        let now = now();
        let started = std::time::Instant::now();
        let mut status = self.catalog.status(id)?.unwrap_or_default();
        status.last_attempt = Some(now);
        let secrets = self.load_secrets(id)?;
        let hidden = redact::secret_values(&secrets);

        report(phase::CONNECTING);
        let fetched = match stop.unless_stopped(self.fetch_snapshot(&config, &secrets, now, &report)).await {
            Some(fetched) if !stop.requested() => fetched,
            _ => {
                tracing::info!("stopped refreshing {} before anything was written", config.name);
                return Ok(self.catalog.status(id)?.unwrap_or_default());
            }
        };
        match fetched {
            Ok(snapshot) => {
                report(phase::ORGANIZING);
                match self.store_snapshot(&config, snapshot).await {
                    Ok(snapshot) => {
                        self.record_catalog(id, &mut status, &snapshot, now)?;
                        if let Err(error) = self.merge_server_progress(id, &snapshot.progress) {
                            tracing::warn!("what the server remembers watching could not be taken in: {error}");
                        }
                        // Titles new to the catalog pick up what Trakt says now, not at the next sync.
                        if self.trakt_history_on()
                            && let Err(error) = self.apply_trakt_history()
                        {
                            tracing::warn!("what Trakt says was watched could not be applied: {error}");
                        }
                        report(phase::LOADING_GUIDE);
                        let guide = self.import_guide(&config, &secrets, snapshot.discovered_epg_url.as_deref(), now, &stop).await;
                        record_guide(&mut status, guide, now, &hidden);
                        if let Err(error) = self.reconcile_watchlist(id, now) {
                            tracing::warn!("saved programmes could not follow the new guide: {error}");
                        }
                        tracing::info!(
                            "refreshed {} in {:.0} s: {} channels, {} films, {} series, {} programmes",
                            config.name,
                            started.elapsed().as_secs_f32(),
                            status.channel_count,
                            status.movie_count,
                            status.series_count,
                            status.program_count
                        );
                    }
                    Err(error) => {
                        status.last_error = Some(redact::redact(&describe(&error), &hidden));
                        tracing::error!("the catalog for {} could not be saved: {}", config.name, status.last_error.as_deref().unwrap_or_default());
                    }
                }
            }
            Err(error) => {
                status.last_error = Some(redact::redact(&error.to_string(), &hidden));
                status.authentication_expired = error.is_authentication();
                tracing::warn!("refresh failed for {}: {}", config.name, status.last_error.as_deref().unwrap_or_default());
            }
        }
        self.catalog.save_status(id, &status)?;
        Ok(status)
    }

    /// Refreshes only the guide.
    pub async fn refresh_guide(&self, id: &str) -> Result<SourceStatus> {
        let config = self.config(id)?;
        if config.is_paused {
            anyhow::bail!("This source is paused. Resume it to refresh it.");
        }
        let Some((_guard, stop)) = RefreshGuard::begin(&self.refreshing, &self.refresh_stops, id) else {
            return Ok(self.catalog.status(id)?.unwrap_or_default());
        };
        let now = now();
        let mut status = self.catalog.status(id)?.unwrap_or_default();
        let secrets = self.load_secrets(id)?;
        let hidden = redact::secret_values(&secrets);
        // A playlist names its guide in its header, which isn't kept (it can
        // carry a login), so it's read again when no guide link is set.
        let discovered = match config.kind {
            SourceKind::Xtream => XtreamCredentials::from_secrets(&secrets).map(|credentials| credentials.guide_url()),
            SourceKind::M3uUrl | SourceKind::M3uFile if secrets.epg_url.is_none() => {
                match self.read_playlist(&config, &secrets).await {
                    Ok(playlist) => playlist.epg_url,
                    Err(error) => {
                        status.guide_last_error = Some(redact::redact(&error.to_string(), &hidden));
                        self.catalog.save_status(id, &status)?;
                        return Ok(status);
                    }
                }
            }
            _ => None,
        };
        let guide = self.import_guide(&config, &secrets, discovered.as_deref(), now, &stop).await;
        record_guide(&mut status, guide, now, &hidden);
        if let Err(error) = self.reconcile_watchlist(id, now) {
            tracing::warn!("saved programmes could not follow the new guide: {error}");
        }
        if status.guide_last_error.is_none() {
            tracing::info!("refreshed the guide for {}: {} programmes", config.name, status.program_count);
        }
        self.catalog.save_status(id, &status)?;
        Ok(status)
    }

    /// Asks the refresh running for a source (or an added guide) to stop;
    /// false when none is running. The refresh returns soon after, keeping
    /// what was there before (see [`Core::refresh_source`]).
    pub fn cancel_refresh(&self, id: &str) -> bool {
        match lock(&self.refresh_stops).get(id) {
            Some(stop) => {
                stop.send_replace(true);
                true
            }
            None => false,
        }
    }

    /// Sources whose refresh interval has passed since their last good
    /// refresh, or that have never refreshed. The demo is due each new day,
    /// since its guide is made around "now".
    pub fn sources_due_for_refresh(&self, now: Timestamp) -> Result<Vec<SourceId>> {
        let statuses = self.catalog.statuses()?;
        let refreshing = lock(&self.refreshing).clone();
        let today = local_day(now);
        Ok(self
            .configs()
            .into_iter()
            .filter(|config| !config.is_paused && !refreshing.contains(&config.id))
            .filter(|config| {
                let status = statuses.get(&config.id);
                let Some(last) = status.and_then(|status| status.last_success) else { return true };
                // Media folders are read again each launch, and while one of their folders is away.
                let folder_due = last < self.opened_at || status.is_some_and(|status| status.catalog_warning.is_some());
                if config.kind == SourceKind::Folder && folder_due {
                    return true;
                }
                if config.refresh_interval.seconds().is_some_and(|interval| now - last > interval) {
                    return true;
                }
                config.kind == SourceKind::Demo && status.and_then(|status| status.guide_last_success).and_then(local_day) != today
            })
            .map(|config| config.id)
            .collect())
    }

    /// Downloads an added guide again. Like a source's refresh, a guide that
    /// can't be read doesn't make this fail: the problem goes in the returned
    /// status, and the guide's previous listings stay.
    pub async fn refresh_guide_source(&self, id: &str) -> Result<SourceStatus> {
        let guide = self.guide_source(id)?;
        let Some((_guard, stop)) = RefreshGuard::begin(&self.refreshing, &self.refresh_stops, id) else {
            return Ok(self.catalog.status(id)?.unwrap_or_default());
        };
        let now = now();
        let mut status = self.catalog.status(id)?.unwrap_or_default();
        status.last_attempt = Some(now);
        let secrets = self.load_secrets(id)?;
        let hidden = redact::secret_values(&secrets);
        let outcome = match secrets.epg_url.as_deref() {
            Some(link) => match self.import_xmltv_link(id, link, None, guide.offset_minutes, now, &stop).await {
                // Stopped: the guide's listings and status stay as they were.
                Err(error) if is_cancelled(&error) => return Ok(self.catalog.status(id)?.unwrap_or_default()),
                outcome => outcome.map(Some),
            },
            None => Err(anyhow::anyhow!("This guide has no link. Edit it to add one.")),
        };
        record_guide(&mut status, outcome, now, &hidden);
        if let Err(error) = self.reconcile_watchlist(id, now) {
            tracing::warn!("saved programmes could not follow the new guide: {error}");
        }
        self.catalog.save_status(id, &status)?;
        Ok(status)
    }

    /// Added guides last loaded more than a day ago, or never, except one
    /// tried in the last hour.
    pub fn guides_due_for_refresh(&self, now: Timestamp) -> Result<Vec<String>> {
        let statuses = self.catalog.statuses()?;
        let refreshing = lock(&self.refreshing).clone();
        Ok(self
            .library
            .guide_sources()?
            .into_iter()
            .filter(|guide| !refreshing.contains(&guide.id))
            .filter(|guide| {
                let status = statuses.get(&guide.id);
                let stale = status
                    .and_then(|status| status.guide_last_success)
                    .is_none_or(|last| now - last > GUIDE_REFRESH_SECONDS);
                let tried = status.and_then(|status| status.last_attempt).is_some_and(|last| now - last < GUIDE_RETRY_SECONDS);
                stale && !tried
            })
            .map(|guide| guide.id)
            .collect())
    }

    /// Fills the demo source's catalog and guide, all from local data.
    pub(crate) fn install_demo(&self, now: Timestamp) -> Result<()> {
        let snapshot = demo::snapshot(now);
        let links = self.guide_links()?;
        self.catalog.replace_catalog(DEMO_SOURCE_ID, &snapshot, false, &links)?;
        let mut status = self.catalog.status(DEMO_SOURCE_ID)?.unwrap_or_default();
        status.last_attempt = Some(now);
        self.record_catalog(DEMO_SOURCE_ID, &mut status, &snapshot, now)?;
        let programs = demo::programs(now, 2, 3);
        let count = self.catalog.import_guide(DEMO_SOURCE_ID, &links, |import| {
            import.insert_channel_names(&demo::guide_channel_names())?;
            import.insert_programs(&programs)
        })?;
        record_guide(&mut status, Ok(Some(count)), now, &[]);
        self.catalog.save_status(DEMO_SOURCE_ID, &status)
    }

    /// Downloads and parses a source's whole catalog, writing nothing.
    async fn fetch_snapshot(
        &self,
        config: &SourceConfig,
        secrets: &SourceSecrets,
        now: Timestamp,
        report: Report<'_>,
    ) -> std::result::Result<CatalogSnapshot, SourceError> {
        let mut snapshot = match config.kind {
            SourceKind::Demo => {
                report(phase::READING_CATALOG);
                demo::snapshot(now)
            }
            SourceKind::M3uUrl | SourceKind::M3uFile => {
                let bytes = self.playlist_bytes(config, secrets).await?;
                report(phase::READING_CATALOG);
                let source_id = config.id.clone();
                tokio::task::spawn_blocking(move || m3u::parse_bytes(&bytes).map(|playlist| m3u::build(&playlist, &source_id)))
                    .await
                    .map_err(|_| SourceError::InvalidPlaylist("it could not be read".into()))??
            }
            SourceKind::Xtream => {
                let credentials = XtreamCredentials::from_secrets(secrets).ok_or_else(|| {
                    SourceError::MissingConfiguration("a server address, username and password".into())
                })?;
                let client =
                    XtreamClient { http: &self.http, credentials: &credentials, user_agent: config.user_agent.as_deref() };
                xtream::fetch_catalog(&client, &config.id, config.live_format, now, || report(phase::READING_CATALOG)).await?
            }
            // A folder that's away is listed as it was, so nothing here needs keeping.
            SourceKind::Folder => return self.scan_folder_source(config, report).await,
            SourceKind::Plex | SourceKind::Jellyfin | SourceKind::Emby => {
                let session = crate::servers::session_for(config, secrets, &self.device_id())?;
                // Refused here means the token went, not that a password was wrong.
                crate::sources::server::fetch_catalog(&self.http, &session, &config.id, report).await.map_err(|error| match error {
                    SourceError::AuthenticationFailed => SourceError::SignedOut,
                    other => other,
                })?
            }
        };
        let previous = self.catalog.counts(&config.id).unwrap_or_default();
        snapshot.keep_lists_that_came_back_empty(previous.movies, previous.series, EMPTY_LIST_THRESHOLD);
        Ok(snapshot)
    }

    async fn playlist_bytes(&self, config: &SourceConfig, secrets: &SourceSecrets) -> std::result::Result<Vec<u8>, SourceError> {
        match config.kind {
            SourceKind::M3uFile => {
                let path = config
                    .file_path
                    .as_deref()
                    .ok_or_else(|| SourceError::MissingConfiguration("a playlist file".into()))?;
                tokio::fs::read(path).await.map_err(|_| SourceError::FileUnavailable)
            }
            _ => {
                let link = secrets
                    .playlist_url
                    .as_deref()
                    .ok_or_else(|| SourceError::MissingConfiguration("a playlist URL".into()))?;
                self.http.bytes(link, config.user_agent.as_deref()).await
            }
        }
    }

    async fn read_playlist(&self, config: &SourceConfig, secrets: &SourceSecrets) -> std::result::Result<m3u::M3uPlaylist, SourceError> {
        let bytes = self.playlist_bytes(config, secrets).await?;
        tokio::task::spawn_blocking(move || m3u::parse_bytes(&bytes))
            .await
            .map_err(|_| SourceError::InvalidPlaylist("it could not be read".into()))?
    }

    /// Writes a snapshot in one transaction, off the async threads.
    async fn store_snapshot(&self, config: &SourceConfig, snapshot: CatalogSnapshot) -> Result<CatalogSnapshot> {
        let catalog = self.catalog.clone();
        let source_id = config.id.clone();
        // Xtream episodes are fetched as series are opened, so a catalog refresh keeps them.
        let preserve_episodes = config.kind == SourceKind::Xtream;
        // A media folder's details all come from its files, so a deleted fanart.jpg takes its backdrop with it.
        let fresh_details = config.kind == SourceKind::Folder;
        let links = self.guide_links()?;
        let snapshot = tokio::task::spawn_blocking(move || {
            if fresh_details {
                catalog.forget_listed_details(&source_id)?;
            }
            catalog.replace_catalog(&source_id, &snapshot, preserve_episodes, &links)?;
            if fresh_details {
                catalog.forget_guessed_posters(&source_id)?;
                catalog.forget_stale_facts(&source_id)?;
            }
            anyhow::Ok(snapshot)
        })
        .await??;
        let prefix = format!("{}|", config.id);
        lock(&self.episodes_fetched).retain(|series| !series.starts_with(&prefix));
        Ok(snapshot)
    }

    /// Notes a successful catalog refresh in the status.
    fn record_catalog(&self, source_id: &str, status: &mut SourceStatus, snapshot: &CatalogSnapshot, now: Timestamp) -> Result<()> {
        // Counts are what's stored: kinds that couldn't be downloaded keep their previous rows.
        let stored = self.catalog.counts(source_id)?;
        status.last_success = Some(now);
        status.last_error = None;
        status.authentication_expired = false;
        status.catalog_warning = (!snapshot.warnings.is_empty()).then(|| snapshot.warnings.join(" "));
        status.channel_count = stored.channels;
        status.movie_count = stored.movies;
        status.series_count = stored.series;
        status.capabilities = snapshot.capabilities.clone();
        status.max_connections = snapshot.max_connections;
        status.account_expires = snapshot.account_expires;
        status.server_time_zone = snapshot.server_time_zone.clone();
        Ok(())
    }

    /// Downloads and imports a guide. `Ok(None)` means the source has no
    /// guide, or that it was stopped before the listings were written (so
    /// they stay, and no problem is recorded).
    async fn import_guide(
        &self,
        config: &SourceConfig,
        secrets: &SourceSecrets,
        discovered: Option<&str>,
        now: Timestamp,
        stop: &Stop,
    ) -> Result<Option<i64>> {
        if config.kind == SourceKind::Demo {
            let catalog = self.catalog.clone();
            let links = self.guide_links()?;
            let count = tokio::task::spawn_blocking(move || {
                let programs = demo::programs(now, 2, 3);
                catalog.import_guide(DEMO_SOURCE_ID, &links, |import| {
                    import.insert_channel_names(&demo::guide_channel_names())?;
                    import.insert_programs(&programs)
                })
            })
            .await??;
            return Ok(Some(count));
        }
        let Some(link) = secrets.epg_url.as_deref().or(discovered) else { return Ok(None) };
        match self.import_xmltv_link(&config.id, link, config.user_agent.as_deref(), config.guide_offset_minutes, now, stop).await {
            Err(error) if is_cancelled(&error) => Ok(None),
            outcome => outcome.map(Some),
        }
    }

    /// Downloads an XMLTV guide and imports it as `guide_id`'s listings, moved
    /// by `offset_minutes`. Stopped while it downloads, it's
    /// [`SourceError::Cancelled`]; once the import has begun, it finishes.
    async fn import_xmltv_link(
        &self,
        guide_id: &str,
        link: &str,
        user_agent: Option<&str>,
        offset_minutes: i64,
        now: Timestamp,
        stop: &Stop,
    ) -> Result<i64> {
        let file = stop.unless_stopped(self.http.download(link, user_agent)).await.ok_or(SourceError::Cancelled)??;
        if stop.requested() {
            return Err(SourceError::Cancelled.into());
        }
        let catalog = self.catalog.clone();
        let guide_id = guide_id.to_string();
        let links = self.guide_links()?;
        let window = (now - GUIDE_DAYS_BACK * 86_400, now + GUIDE_DAYS_FORWARD * 86_400);
        tokio::task::spawn_blocking(move || import_xmltv(&catalog, &guide_id, &file, window, offset_minutes, &links)).await?
    }

    /// After a guide refreshes, saved programmes on the channels it serves
    /// follow their listings: a game moved from 7:00 to 7:30 keeps its place
    /// and its reminder.
    fn reconcile_watchlist(&self, guide_id: &str, now: Timestamp) -> Result<()> {
        let saved = self
            .library
            .watchlist()?
            .into_iter()
            .filter(|entry| entry.kind == xorvio_model::MediaKind::Program && !watchlist::has_ended(entry, now));
        for entry in saved {
            let (Some(start), Some(channel_id)) = (entry.start, entry.channel_id.as_deref()) else { continue };
            let Some((guide, epg_id)) = self.catalog.guide_link(channel_id)? else { continue };
            if guide != guide_id {
                continue;
            }
            let Some(channel) = self.catalog.channel(channel_id)? else { continue };
            let listings =
                self.catalog.programs(&guide, &epg_id, start - MATCH_WINDOW_SECONDS, start + MATCH_WINDOW_SECONDS + 1)?;
            let Some(found) = watchlist::matching(&entry, &listings, MATCH_WINDOW_SECONDS) else { continue };
            let mut updated = watchlist::entry_for_program(found, &channel.id, entry.remind, entry.added_at);
            updated.artwork_url = found.image_url.clone().or_else(|| entry.artwork_url.clone());
            if updated != entry {
                self.library.replace_watchlist_entry(&entry.id, &updated)?;
            }
        }
        Ok(())
    }
}

/// Streams an XMLTV file into one guide transaction.
fn import_xmltv(
    catalog: &CatalogStore,
    guide_id: &str,
    file: &Downloaded,
    window: (Timestamp, Timestamp),
    offset_minutes: i64,
    links: &GuideLinks,
) -> Result<i64> {
    catalog.import_guide(guide_id, links, |import| {
        let shared = RefCell::new(import);
        let mut on_programs = |batch: Vec<Program>| shared.borrow_mut().insert_programs(&batch);
        let mut on_channels = |names: Vec<ChannelName>| shared.borrow_mut().insert_channel_names(&names);
        XmltvParser::new(guide_id, window, &mut on_programs, &mut on_channels)
            .shifted(offset_minutes)
            .parse_file(file.path())
    })
}

fn is_cancelled(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref::<SourceError>(), Some(SourceError::Cancelled))
}

/// Notes a guide import's outcome in the status.
fn record_guide(status: &mut SourceStatus, outcome: Result<Option<i64>>, now: Timestamp, hidden: &[String]) {
    match outcome {
        Ok(Some(count)) => {
            status.guide_last_success = Some(now);
            status.guide_last_error = None;
            status.program_count = count;
            status.capabilities.guide = true;
        }
        Ok(None) => {}
        Err(error) => {
            let message = redact::redact(&describe(&error), hidden);
            tracing::warn!("guide import failed: {message}");
            status.guide_last_error = Some(message);
        }
    }
}

/// The calendar day of a moment in this computer's time zone.
fn local_day(moment: Timestamp) -> Option<chrono::NaiveDate> {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(moment, 0).single().map(|local| local.date_naive())
}
