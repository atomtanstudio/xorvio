//! TMDB (themoviedb.org), with the person's own free key or one built into
//! this copy: title logos, wordless backdrops, cast with photos, trailers,
//! seasons and episode stills, what TMDB recommends, and people's pages.
//!
//! Titles are matched by the TMDB id their source gives, else their IMDb id,
//! else their name and year, which is all that's sent; the demo library never
//! is. What TMDB says is kept in the catalog for a month and laid over the
//! source's own details as they're read: over an IPTV provider's (TMDB's are
//! usually better), under a media server's (the person's own library, as they
//! keep it). TMDB's terms ask that it be credited, which Settings does.

mod api;
mod posters;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::future::Future;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use xorvio_model::{DEMO_SOURCE_ID, Episode, MediaDetails, Movie, PersonDetails, Series, TmdbStatus};

use crate::details::DetailsRecord;
use crate::details_api::ProviderTurns;
use crate::error::SourceError;
use crate::names;
use crate::secrets::SecretStore;
use crate::store::catalog::{MatchKeys, title_key};
use crate::store::lock;
use crate::{Core, Result, now};
use api::{Client, Key, Kind, TITLE_FORMAT, TmdbEpisode, TmdbPerson, TmdbRef};
pub(crate) use api::TmdbTitle;

/// The keychain entry holding the person's own key.
pub(crate) const KEY_ENTRY: &str = "tmdb:key";
/// How long what TMDB said is kept before it's asked again.
const KEEP_FOR: i64 = 30 * 86_400;
/// How long a title TMDB had nothing for waits before it's looked up again.
const RETRY_MISSES_AFTER: i64 = 14 * 86_400;
/// How long something waits after TMDB couldn't be reached about it.
const RETRY_FAILURES_AFTER: Duration = Duration::from_secs(10 * 60);
const MOST_RELATED: usize = 20;
const MOST_CREDITS: usize = 60;

pub(crate) struct Tmdb {
    /// The person's own key, from Settings.
    own_key: Mutex<Option<Key>>,
    /// A key this copy was built with (`XORVIO_TMDB_KEY` when compiled).
    built_in: Option<Key>,
    /// Where TMDB's API is (a stand-in in tests).
    base: Mutex<String>,
    /// TMDB turned the key down; nothing is asked until a key is saved again.
    rejected: AtomicBool,
    /// Settings › Details allows TMDB at all.
    enabled: AtomicBool,
    /// When TMDB last couldn't be reached about something, by what it was.
    failures: Mutex<HashMap<String, Instant>>,
    /// TMDB allows dozens of requests a second; a few at a time is plenty.
    turns: ProviderTurns,
    /// Posters for lists are looked for one batch at a time.
    filling_posters: tokio::sync::Mutex<()>,
}

impl Tmdb {
    pub fn new(secrets: &dyn SecretStore) -> Tmdb {
        let own_key = match secrets.get(KEY_ENTRY) {
            Ok(text) => text.as_deref().and_then(Key::new),
            Err(error) => {
                tracing::warn!("the TMDB key could not be read: {error}");
                None
            }
        };
        Tmdb {
            own_key: Mutex::new(own_key),
            built_in: option_env!("XORVIO_TMDB_KEY").and_then(Key::new),
            base: Mutex::new(api::DEFAULT_BASE.into()),
            rejected: AtomicBool::new(false),
            enabled: AtomicBool::new(true),
            failures: Mutex::default(),
            turns: ProviderTurns::with(4, Duration::from_millis(60)),
            filling_posters: tokio::sync::Mutex::new(()),
        }
    }

    /// The key to ask with, when TMDB is allowed and there is one it hasn't
    /// turned down. Every request goes through here, so switching TMDB off
    /// stops them all.
    fn key(&self) -> Option<Key> {
        if !self.enabled.load(Ordering::Relaxed) || self.rejected.load(Ordering::Relaxed) {
            return None;
        }
        lock(&self.own_key).clone().or_else(|| self.built_in.clone())
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn status(&self) -> TmdbStatus {
        let custom_key = lock(&self.own_key).is_some();
        TmdbStatus {
            configured: custom_key || self.built_in.is_some(),
            custom_key,
            built_in: self.built_in.is_some(),
            rejected: self.rejected.load(Ordering::Relaxed),
        }
    }

    fn base(&self) -> String {
        lock(&self.base).clone()
    }

    fn failed_lately(&self, what: &str) -> bool {
        lock(&self.failures).get(what).is_some_and(|at| at.elapsed() < RETRY_FAILURES_AFTER)
    }

    /// A key TMDB turns down stops every request until one is saved again;
    /// anything else waits a while before it's asked about again.
    fn note_failure(&self, what: &str, error: &SourceError) {
        if error.is_authentication() {
            if !self.rejected.swap(true, Ordering::Relaxed) {
                tracing::warn!("TMDB turned the key down");
            }
        } else {
            tracing::debug!("TMDB could not be asked: {error}");
            lock(&self.failures).insert(what.to_string(), Instant::now());
        }
    }
}

/// A catalog film or series, as TMDB is asked about it.
pub(crate) struct TitleLookup {
    kind: Kind,
    item_id: String,
    source_id: String,
    tmdb_id: Option<String>,
    imdb_id: Option<String>,
    title: String,
    year: Option<i64>,
}

impl TitleLookup {
    pub fn of_movie(movie: &Movie) -> Self {
        Self {
            kind: Kind::Movie,
            item_id: movie.id.clone(),
            source_id: movie.source_id.clone(),
            tmdb_id: movie.tmdb_id.clone(),
            imdb_id: movie.imdb_id.clone(),
            title: movie.title.clone(),
            year: movie.year,
        }
    }

    pub fn of_series(series: &Series) -> Self {
        Self {
            kind: Kind::Tv,
            item_id: series.id.clone(),
            source_id: series.source_id.clone(),
            tmdb_id: series.tmdb_id.clone(),
            imdb_id: series.imdb_id.clone(),
            title: series.title.clone(),
            year: series.year,
        }
    }

    /// An add-on's film or series by its id ("addon|movie|tt0111161", or
    /// one of its episodes), when that holds an IMDb id, which is all TMDB
    /// needs to find it.
    fn of_addon_id(item_id: &str) -> Option<Self> {
        let (kind, media_id) = item_id.strip_prefix("addon|")?.split_once('|')?;
        let kind = match kind {
            "movie" => Kind::Movie,
            "series" => Kind::Tv,
            _ => return None,
        };
        let imdb_id = media_id.split(['|', ':']).next().filter(|id| is_imdb_id(id))?;
        Some(Self {
            kind,
            item_id: format!("addon|{}|{imdb_id}", if kind == Kind::Movie { "movie" } else { "series" }),
            source_id: "addon".into(),
            tmdb_id: None,
            imdb_id: Some(imdb_id.to_string()),
            title: String::new(),
            year: None,
        })
    }
}

/// A title's details with TMDB's laid over them, or, for a media server's
/// (`source_first`), only filling what the server left out.
pub(crate) fn overlay(details: DetailsRecord, tmdb: &TmdbTitle, source_first: bool) -> DetailsRecord {
    if source_first { details.filling_from(&tmdb.details) } else { details.merged(&tmdb.details) }
}

impl Core {
    pub fn tmdb_status(&self) -> TmdbStatus {
        self.tmdb.status()
    }

    /// Saves the person's own key once TMDB has taken it (an API key or a
    /// read access token), or with an empty one, goes back to the built-in
    /// key or none.
    pub async fn tmdb_set_key(&self, key: &str) -> Result<TmdbStatus> {
        const WHERE: &str = "Copy the API key or the read access token from themoviedb.org › Settings › API.";
        if key.trim().is_empty() {
            self.secrets.delete(KEY_ENTRY)?;
            *lock(&self.tmdb.own_key) = None;
        } else {
            let key = Key::new(key).ok_or_else(|| SourceError::Unsupported(format!("That isn't a TMDB key. {WHERE}")))?;
            let base = self.tmdb.base();
            match (Client { http: &self.http, base: &base, key: &key }).check_key().await {
                Ok(()) => {}
                Err(error) if error.is_authentication() => {
                    return Err(SourceError::Unsupported(format!("TMDB didn't accept that key. {WHERE}")).into());
                }
                Err(_) => return Err(SourceError::Unsupported("TMDB couldn't be reached to check the key. Try again in a moment.".into()).into()),
            }
            self.secrets.set(KEY_ENTRY, key.as_str())?;
            *lock(&self.tmdb.own_key) = Some(key);
        }
        self.tmdb.rejected.store(false, Ordering::Relaxed);
        lock(&self.tmdb.failures).clear();
        Ok(self.tmdb.status())
    }

    #[cfg(test)]
    pub(crate) fn set_tmdb_base(&self, base: &str) {
        *lock(&self.tmdb.base) = base.to_string();
    }

    /// Sources whose titles TMDB may be asked about: every one that isn't
    /// paused, except the demo library.
    fn tmdb_sources(&self) -> Vec<String> {
        self.active_source_ids().into_iter().filter(|id| id != DEMO_SOURCE_ID).collect()
    }

    /// What TMDB says about a catalog title, or `None` without a key, for
    /// the demo library, or when TMDB has nothing convincing.
    pub(crate) async fn tmdb_title(&self, lookup: &TitleLookup) -> Option<TmdbTitle> {
        let key = self.tmdb.key()?;
        if lookup.source_id == DEMO_SOURCE_ID {
            return None;
        }
        // A series' details and episodes are asked for at once: one asks
        // TMDB, the other then reads what it kept.
        let item_lock = self.item_lock(&format!("tmdb|{}", lookup.item_id));
        let _held = item_lock.lock().await;
        let id = self.tmdb_id(&key, lookup).await?;
        self.tmdb_fetch_title(&key, lookup.kind, id).await.filter(|title| !title.is_empty())
    }

    /// The title's TMDB id: the one its source gives, else the one its IMDb
    /// id or its name and year lead to.
    async fn tmdb_id(&self, key: &Key, lookup: &TitleLookup) -> Option<i64> {
        self.tmdb_id_lookup(key, lookup).await.flatten()
    }

    /// [`Core::tmdb_id`], telling TMDB having nothing (`Some(None)`) from
    /// TMDB not being asked (`None`).
    async fn tmdb_id_lookup(&self, key: &Key, lookup: &TitleLookup) -> Option<Option<i64>> {
        if let Some(id) = lookup.tmdb_id.as_deref().and_then(parse_id) {
            return Some(Some(id));
        }
        let base = self.tmdb.base();
        let client = Client { http: &self.http, base: &base, key };
        let kind = lookup.kind;
        if let Some(imdb_id) = lookup.imdb_id.as_deref().filter(|id| is_imdb_id(id)) {
            match self.tmdb_lookup(kind.path(), &format!("imdb:{imdb_id}"), client.find_by_imdb(kind, imdb_id)).await {
                Some(Some(id)) => return Some(Some(id)),
                // TMDB doesn't have that IMDb id; the name may still find it.
                Some(None) => {}
                None => return None,
            }
        }
        let (title, title_year) = names::split_year(&names::media_title(&lookup.title));
        let year = lookup.year.or(title_year);
        let key = names::match_key(&title);
        if key.is_empty() {
            return Some(None);
        }
        self.tmdb_lookup(kind.path(), &format!("{key}|{}", year.unwrap_or(0)), client.best_match(kind, &title, year)).await
    }

    /// A lookup's answer, as kept (a miss only for a while) or asked of TMDB
    /// and kept. `None` when TMDB couldn't be asked.
    async fn tmdb_lookup(
        &self,
        kind: &str,
        lookup: &str,
        ask: impl Future<Output = Result<Option<i64>, SourceError>>,
    ) -> Option<Option<i64>> {
        match self.catalog.tmdb_match(kind, lookup) {
            Ok(Some((Some(id), _))) => return Some(Some(id)),
            Ok(Some((None, at))) if now() - at < RETRY_MISSES_AFTER => return Some(None),
            Ok(_) => {}
            Err(error) => tracing::warn!("a TMDB match could not be read: {error}"),
        }
        let what = format!("{kind}:{lookup}");
        if self.tmdb.failed_lately(&what) {
            return None;
        }
        let found = {
            let _turn = self.tmdb.turns.wait().await;
            ask.await
        };
        match found {
            Ok(found) => {
                if let Err(error) = self.catalog.save_tmdb_match(kind, lookup, found, now()) {
                    tracing::warn!("a TMDB match could not be saved: {error}");
                }
                Some(found)
            }
            Err(error) => {
                self.tmdb.note_failure(&what, &error);
                None
            }
        }
    }

    async fn tmdb_fetch_title(&self, key: &Key, kind: Kind, id: i64) -> Option<TmdbTitle> {
        let base = self.tmdb.base();
        let client = Client { http: &self.http, base: &base, key };
        let ask = async {
            let mut title = {
                let _turn = self.tmdb.turns.wait().await;
                match kind {
                    Kind::Movie => return client.movie(id).await,
                    Kind::Tv => client.series(id).await?,
                }
            };
            for seasons in title.season_numbers.clone().chunks(api::MOST_APPENDED) {
                let _turn = self.tmdb.turns.wait().await;
                title.episodes.extend(client.episodes(id, seasons).await?);
            }
            Ok(title)
        };
        self.tmdb_fetch(kind.path(), id, |title: &TmdbTitle| title.format >= TITLE_FORMAT, ask).await
    }

    async fn tmdb_fetch_person(&self, key: &Key, id: i64) -> Option<TmdbPerson> {
        let base = self.tmdb.base();
        let client = Client { http: &self.http, base: &base, key };
        let ask = async {
            let _turn = self.tmdb.turns.wait().await;
            client.person(id).await
        };
        self.tmdb_fetch("person", id, |_: &TmdbPerson| true, ask).await.filter(|person| !person.name.is_empty())
    }

    /// Something TMDB says, as kept while it's fresh (and `current`: kept in
    /// the form this version keeps), else asked again and kept; what was
    /// kept before when TMDB can't be asked. An id TMDB doesn't know (one a
    /// source made up, or since removed) is kept as nothing, so it isn't
    /// asked about every time.
    async fn tmdb_fetch<T: Serialize + DeserializeOwned + Default>(
        &self,
        kind: &str,
        id: i64,
        current: impl Fn(&T) -> bool,
        ask: impl Future<Output = Result<T, SourceError>>,
    ) -> Option<T> {
        let kept = match self.catalog.tmdb_cached(kind, id) {
            Ok(kept) => kept.and_then(|(text, at)| Some((serde_json::from_str::<T>(&text).ok()?, at))),
            Err(error) => {
                tracing::warn!("what TMDB said could not be read: {error}");
                None
            }
        };
        let kept = match kept {
            Some((value, at)) if now() - at < KEEP_FOR && current(&value) => return Some(value),
            kept => kept.map(|(value, _)| value),
        };
        let what = format!("{kind}/{id}");
        if self.tmdb.failed_lately(&what) {
            return kept;
        }
        let value = match ask.await {
            Ok(value) => value,
            Err(SourceError::Http(404)) => T::default(),
            Err(error) => {
                self.tmdb.note_failure(&what, &error);
                return kept;
            }
        };
        let saved = serde_json::to_string(&value).map_err(anyhow::Error::from).and_then(|text| self.catalog.save_tmdb_cached(kind, id, &text, now()));
        if let Err(error) = saved {
            tracing::warn!("what TMDB said could not be saved: {error}");
        }
        Some(value)
    }

    /// Episodes with what TMDB adds where their source left gaps: a still,
    /// a synopsis, the running time, the air date, and a name for episodes
    /// that are only numbered.
    pub(crate) async fn with_tmdb_episodes(&self, series: &Series, episodes: Vec<Episode>) -> Vec<Episode> {
        if episodes.is_empty() {
            return episodes;
        }
        match self.tmdb_title(&TitleLookup::of_series(series)).await {
            Some(title) if !title.episodes.is_empty() => fill_episodes(&series.title, episodes, &title.episodes),
            _ => episodes,
        }
    }

    /// An add-on's film with TMDB's details laid over the add-on's own, as
    /// over an IPTV provider's: add-ons seldom carry cast photos, wordless
    /// backdrops or a trailer. The add-on's stay wherever TMDB has nothing.
    pub async fn addon_movie_details(&self, movie: &Movie, details: MediaDetails) -> MediaDetails {
        self.with_tmdb_details(&TitleLookup::of_movie(movie), details).await
    }

    /// The same for an add-on's series.
    pub async fn addon_series_details(&self, series: &Series, details: MediaDetails) -> MediaDetails {
        self.with_tmdb_details(&TitleLookup::of_series(series), details).await
    }

    async fn with_tmdb_details(&self, lookup: &TitleLookup, details: MediaDetails) -> MediaDetails {
        match self.tmdb_title(lookup).await {
            Some(title) => overlay(DetailsRecord::from_model(details), &title, false).into_model(),
            None => details,
        }
    }

    /// Films TMDB recommends with this one that the sources carry, in
    /// TMDB's order, each once. Empty without a key.
    pub async fn related_movies(&self, movie_id: &str) -> Result<Vec<Movie>> {
        let Some(movie) = self.catalog.movies_by_ids(&[movie_id.to_string()])?.pop() else { return Ok(Vec::new()) };
        self.movies_related_to(&movie).await
    }

    /// The same for a film that isn't in the catalog (an add-on's).
    pub async fn movies_related_to(&self, movie: &Movie) -> Result<Vec<Movie>> {
        let Some(title) = self.tmdb_title(&TitleLookup::of_movie(movie)).await else { return Ok(Vec::new()) };
        let sources = self.tmdb_sources();
        let found = self.visible_movies(self.catalog.movies_matching(&sources, &match_keys(&title.recommendations))?)?;
        let found = found.into_iter().filter(|other| other.id != movie.id).collect();
        self.present_movies(pick(&title.recommendations, found, &sources, MOST_RELATED))
    }

    /// Series TMDB recommends with this one that the sources carry.
    pub async fn related_series(&self, series_id: &str) -> Result<Vec<Series>> {
        let Some(series) = self.catalog.series_by_ids(&[series_id.to_string()])?.pop() else { return Ok(Vec::new()) };
        self.series_related_to(&series).await
    }

    /// The same for a series that isn't in the catalog (an add-on's).
    pub async fn series_related_to(&self, series: &Series) -> Result<Vec<Series>> {
        let Some(title) = self.tmdb_title(&TitleLookup::of_series(series)).await else { return Ok(Vec::new()) };
        let sources = self.tmdb_sources();
        let found = self.visible_series(self.catalog.series_matching(&sources, &match_keys(&title.recommendations))?)?;
        let found = found.into_iter().filter(|other| other.id != series.id).collect();
        self.present_series_list(pick(&title.recommendations, found, &sources, MOST_RELATED))
    }

    /// Someone from a cast: who they are, and their films and series the
    /// sources carry, best known first. With a TMDB key it's TMDB's page:
    /// casts from TMDB give `tmdb_id`; otherwise the name is looked up,
    /// preferring the person known for `item_id` (the title whose cast
    /// they're in). Without a key, or when TMDB doesn't know them, it's
    /// their filmography from Wikidata. `None` when neither knows them.
    pub async fn person(&self, name: &str, tmdb_id: Option<&str>, item_id: Option<&str>) -> Result<Option<PersonDetails>> {
        if let Some(person) = self.tmdb_person(name, tmdb_id, item_id).await? {
            return Ok(Some(person));
        }
        self.wikidata_person(name, item_id).await
    }

    async fn tmdb_person(&self, name: &str, tmdb_id: Option<&str>, item_id: Option<&str>) -> Result<Option<PersonDetails>> {
        let Some(key) = self.tmdb.key() else { return Ok(None) };
        let id = match tmdb_id.and_then(parse_id) {
            Some(id) => Some(id),
            None => self.tmdb_person_id(&key, name, item_id).await,
        };
        let Some(person) = (match id {
            Some(id) => self.tmdb_fetch_person(&key, id).await,
            None => None,
        }) else {
            return Ok(None);
        };
        let sources = self.tmdb_sources();
        let (shows, films): (Vec<TmdbRef>, Vec<TmdbRef>) = {
            let (shows, films): (Vec<_>, Vec<_>) = person.credits.iter().partition(|credit| credit.series);
            (shows.into_iter().map(|credit| credit.work.clone()).collect(), films.into_iter().map(|credit| credit.work.clone()).collect())
        };
        let movies = self.visible_movies(self.catalog.movies_matching(&sources, &match_keys(&films))?)?;
        let series = self.visible_series(self.catalog.series_matching(&sources, &match_keys(&shows))?)?;
        Ok(Some(PersonDetails {
            tmdb_id: person.id.to_string(),
            name: person.name,
            photo_url: person.photo_url,
            biography: person.biography,
            department: person.department,
            birthday: person.birthday,
            deathday: person.deathday,
            place_of_birth: person.place_of_birth,
            movies: self.present_movies(pick(&films, movies, &sources, MOST_CREDITS))?,
            series: self.present_series_list(pick(&shows, series, &sources, MOST_CREDITS))?,
            summary: None,
            attribution: Some("TMDB".into()),
            // TMDB's photos are credited with TMDB itself.
            photo_credit: None,
            photo_page: None,
        }))
    }

    async fn tmdb_person_id(&self, key: &Key, name: &str, item_id: Option<&str>) -> Option<i64> {
        let name_key = names::match_key(name);
        if name_key.is_empty() {
            return None;
        }
        // The title they were chosen from, as TMDB knows it, tells people
        // who share a name apart.
        let known_for = match item_id {
            Some(item_id) => self.tmdb_id_of_item(key, item_id).await,
            None => None,
        };
        let lookup = match known_for {
            Some((kind, id)) => format!("{name_key}|{}/{id}", kind.path()),
            None => name_key.clone(),
        };
        let base = self.tmdb.base();
        let client = Client { http: &self.http, base: &base, key };
        let ask = async {
            let found = client.search_person(name).await?;
            let chosen = found
                .iter()
                .find(|person| known_for.is_some_and(|work| person.known_for.contains(&work)))
                .or_else(|| found.iter().find(|person| names::match_key(&person.name) == name_key));
            Ok(chosen.map(|person| person.id))
        };
        self.tmdb_lookup("person", &lookup, ask).await.flatten()
    }

    async fn tmdb_id_of_item(&self, key: &Key, item_id: &str) -> Option<(Kind, i64)> {
        let ids = [item_id.to_string()];
        let lookup = if let Some(movie) = self.catalog.movies_by_ids(&ids).ok()?.pop() {
            TitleLookup::of_movie(&movie)
        } else if let Some(series) = self.catalog.series_by_ids(&ids).ok()?.pop() {
            TitleLookup::of_series(&series)
        } else {
            TitleLookup::of_addon_id(item_id)?
        };
        if lookup.source_id == DEMO_SOURCE_ID {
            return None;
        }
        Some((lookup.kind, self.tmdb_id(key, &lookup).await?))
    }

    /// Films that browsing would show: not hidden, locked behind the PIN or
    /// left out by the profile.
    pub(crate) fn visible_movies(&self, movies: Vec<Movie>) -> Result<Vec<Movie>> {
        let excluded = self.excluded(false)?;
        Ok(movies.into_iter().filter(|movie| !excluded.movie(movie)).collect())
    }

    pub(crate) fn visible_series(&self, series: Vec<Series>) -> Result<Vec<Series>> {
        let excluded = self.excluded(false)?;
        Ok(series.into_iter().filter(|series| !excluded.series(series)).collect())
    }
}

/// TMDB's ids are positive whole numbers.
fn parse_id(text: &str) -> Option<i64> {
    text.trim().parse::<i64>().ok().filter(|id| *id > 0)
}

fn is_imdb_id(text: &str) -> bool {
    text.len() > 2 && text.starts_with("tt") && text[2..].chars().all(|c| c.is_ascii_digit())
}

/// What finds these works in the catalog: their TMDB ids and names.
fn match_keys(works: &[TmdbRef]) -> MatchKeys {
    let mut title_keys: Vec<String> = works
        .iter()
        .flat_map(|work| std::iter::once(&work.title).chain(work.original_title.as_ref()))
        .map(|title| names::match_key(title))
        .filter(|key| !key.is_empty())
        .collect();
    title_keys.sort();
    title_keys.dedup();
    MatchKeys { imdb: Vec::new(), tmdb: works.iter().map(|work| work.id.to_string()).collect(), title_keys }
}

/// What [`pick`] needs of a film or series.
trait CatalogTitle {
    fn source_id(&self) -> &str;
    fn title(&self) -> &str;
    fn year(&self) -> Option<i64>;
    fn tmdb_id(&self) -> Option<&str>;
}

impl CatalogTitle for Movie {
    fn source_id(&self) -> &str {
        &self.source_id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn year(&self) -> Option<i64> {
        self.year
    }
    fn tmdb_id(&self) -> Option<&str> {
        self.tmdb_id.as_deref()
    }
}

impl CatalogTitle for Series {
    fn source_id(&self) -> &str {
        &self.source_id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn year(&self) -> Option<i64> {
        self.year
    }
    fn tmdb_id(&self) -> Option<&str> {
        self.tmdb_id.as_deref()
    }
}

/// A catalog title for each work, in the works' order, up to `limit`: the
/// one with its TMDB id, else one by its name and year (with no TMDB id of
/// its own), from the earliest source that has it. Each work is shown once,
/// however many sources carry it.
fn pick<T: CatalogTitle>(works: &[TmdbRef], mut titles: Vec<T>, sources: &[String], limit: usize) -> Vec<T> {
    titles.sort_by_key(|title| sources.iter().position(|source| source == title.source_id()).unwrap_or(usize::MAX));
    let keys: Vec<String> = titles.iter().map(|title| title_key(title.title())).collect();
    let ids: Vec<Option<i64>> = titles.iter().map(|title| title.tmdb_id().and_then(parse_id)).collect();
    let mut used = vec![false; titles.len()];
    let mut chosen = Vec::new();
    for work in works {
        if chosen.len() == limit {
            break;
        }
        let is_it = |index: usize| ids[index] == Some(work.id) || (ids[index].is_none() && work.matches(&keys[index], titles[index].year()));
        let first = (0..titles.len())
            .find(|&index| !used[index] && ids[index] == Some(work.id))
            .or_else(|| (0..titles.len()).find(|&index| !used[index] && is_it(index)));
        let Some(first) = first else { continue };
        for (index, taken) in used.iter_mut().enumerate() {
            *taken |= is_it(index);
        }
        chosen.push(first);
    }
    let mut slots: Vec<Option<T>> = titles.into_iter().map(Some).collect();
    chosen.into_iter().filter_map(|index| slots[index].take()).collect()
}

/// Whether TMDB's name for an episode should replace its source's title:
/// one that only numbers it ("Episode 4", "S01E04", "Paper Boats S01 E04"),
/// or that is TMDB's name dressed in tags and numbering ("A+ - Paper Boats
/// (2022) (US) - S01E04 - Undertow").
fn renamed_by_tmdb(title: &str, series_title: &str, name: &str) -> bool {
    static EPISODE_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"s\d{1,3}e\d{1,4}").unwrap());
    if is_numbered_only(title, series_title) {
        return true;
    }
    let (key, name_key) = (names::match_key(title), names::match_key(name));
    let Some(dressing) = key.strip_suffix(name_key.as_str()).filter(|dressing| !dressing.is_empty() && !name_key.is_empty()) else {
        return false;
    };
    let series_key = title_key(series_title);
    EPISODE_CODE.is_match(dressing) || (!series_key.is_empty() && dressing.contains(series_key.as_str()))
}

/// Episode titles that only number the episode ("Episode 4", "S01E04",
/// "Paper Boats S01 E04").
fn is_numbered_only(title: &str, series_title: &str) -> bool {
    static NUMBERING: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(?:s\d{1,3}e\d{1,4}|season\d{1,3}episode\d{1,4}|episode\d{1,4}|ep\d{1,4}|e\d{1,4}|\d{1,4})*$").unwrap());
    let key = names::match_key(title);
    let series_key = title_key(series_title);
    let rest = if series_key.is_empty() { key.as_str() } else { key.strip_prefix(series_key.as_str()).unwrap_or(&key) };
    NUMBERING.is_match(rest)
}

fn fill_episodes(series_title: &str, episodes: Vec<Episode>, from_tmdb: &[TmdbEpisode]) -> Vec<Episode> {
    let by_number: HashMap<(i64, i64), &TmdbEpisode> = from_tmdb.iter().map(|episode| ((episode.season, episode.number), episode)).collect();
    episodes
        .into_iter()
        .map(|mut episode| {
            let Some(found) = by_number.get(&(episode.season, episode.number)) else { return episode };
            if episode.plot.as_deref().is_none_or(|plot| plot.trim().is_empty()) {
                episode.plot = found.overview.clone().or(episode.plot);
            }
            if episode.still_url.is_none() {
                episode.still_url = found.still_url.clone();
            }
            if episode.duration_seconds.is_none_or(|seconds| seconds <= 0) {
                episode.duration_seconds = found.runtime_minutes.map(|minutes| minutes * 60).or(episode.duration_seconds);
            }
            if episode.air_date.is_none() {
                episode.air_date = found.air_date.as_deref().and_then(air_date);
            }
            if let Some(name) = &found.name
                && renamed_by_tmdb(&episode.title, series_title, name)
            {
                episode.title = name.clone();
            }
            episode
        })
        .collect()
}

/// Midday UTC on the day TMDB gives, so it reads as that day everywhere.
fn air_date(date: &str) -> Option<i64> {
    let day = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some(day.and_hms_opt(12, 0, 0)?.and_utc().timestamp())
}
