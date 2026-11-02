//! A sync with Trakt. `/sync/last_activities` says what changed there since
//! the last one; only that is pulled, into the library's Trakt tables. Then
//! what the tables say is applied to Xorvio's own progress (every sync, so
//! titles a refresh just brought in pick it up too) the way a media server's
//! watched state is: Trakt wins where it knows of something later, Xorvio's
//! own newer progress is kept, and nothing is ever removed because of Trakt.
//! The watchlist is reconciled both ways against what both sides had after
//! the last sync, which tells a removal on one side from a title never synced.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};
use xorvio_model::{DEMO_SOURCE_ID, Episode, MediaKind, PlaybackProgress, Series, Timestamp, TraktStatus, WatchlistEntry};

use super::api::TraktError;
use super::ids::{self, ADDON_SOURCE, CatalogTitle, Candidates, TraktIds};
use crate::store::catalog::title_key;
use crate::store::library::{TraktListed, TraktPaused, TraktTitle, TraktWatched};
use crate::{Core, Result, json as loose, now, redact};

impl Core {
    /// Runs one sync, unless one is already running (then it waits for that
    /// one), and returns the status. A sync that fails says why in the
    /// status's `last_error`; this only fails when Trakt isn't connected.
    pub async fn trakt_sync(&self) -> Result<TraktStatus> {
        let trakt = self.trakt.clone();
        if !trakt.is_connected() {
            return Err(TraktError::NotConnected.into());
        }
        let _turn = match trakt.sync_turn.try_lock() {
            Ok(turn) => turn,
            Err(_) => {
                drop(trakt.sync_turn.lock().await);
                return Ok(trakt.status());
            }
        };
        trakt.set_syncing(true);
        let changed = match self.pull_and_apply().await {
            Ok(changed) => {
                trakt.record_sync(Ok(()));
                changed
            }
            Err(error) => {
                let message = redact::redact(&error.to_string(), &trakt.hidden());
                tracing::warn!("the Trakt sync didn't finish: {message}");
                trakt.record_sync(Err(message));
                false
            }
        };
        trakt.set_syncing(false);
        if changed {
            trakt.notify_library();
        }
        Ok(trakt.status())
    }

    /// Pulls what changed and applies it; true when progress or the
    /// watchlist changed here.
    async fn pull_and_apply(&self) -> Result<bool> {
        let options = self.trakt.options();
        if !options.sync_history && !options.sync_watchlist {
            return Ok(false);
        }
        let activities = self.trakt.get("/sync/last_activities").await?;
        let mut changed = 0;
        if options.sync_history {
            self.pull_history(&activities).await?;
            changed += self.apply_trakt_history()?;
        }
        if options.sync_watchlist {
            changed += self.sync_trakt_watchlist(&activities).await?;
        }
        Ok(changed > 0)
    }

    async fn pull_history(&self, activities: &Value) -> Result<()> {
        let trakt = &self.trakt;
        let films = activity(activities, &[("movies", "watched_at")]);
        if trakt.activity("movies.watched").is_none_or(|seen| seen != films || films.is_empty()) {
            let rows = watched_films(&trakt.get("/sync/watched/movies").await?);
            self.library.replace_trakt_watched(false, &rows)?;
            trakt.set_activity("movies.watched", &films);
        }
        let episodes = activity(activities, &[("episodes", "watched_at")]);
        if trakt.activity("episodes.watched").is_none_or(|seen| seen != episodes || episodes.is_empty()) {
            let rows = watched_episodes(&trakt.get("/sync/watched/shows").await?);
            self.library.replace_trakt_watched(true, &rows)?;
            trakt.set_activity("episodes.watched", &episodes);
        }
        let paused = activity(activities, &[("movies", "paused_at"), ("episodes", "paused_at")]);
        if trakt.activity("paused").is_none_or(|seen| seen != paused || paused.is_empty()) {
            // `extended=full` adds runtimes, for titles whose length Xorvio doesn't know.
            let rows = paused_places(&trakt.get("/sync/playback?extended=full").await?);
            self.library.replace_trakt_paused(&rows)?;
            trakt.set_activity("paused", &paused);
        }
        Ok(())
    }

    /// Active sources whose titles Trakt's can be, in the user's order. The
    /// demo library's titles are made up.
    pub(super) fn trakt_sources(&self) -> Vec<String> {
        self.active_source_ids().into_iter().filter(|id| id != DEMO_SOURCE_ID).collect()
    }

    /// Applies what the Trakt tables say to Xorvio's progress: watched films
    /// and episodes become finished on every matching catalog title, and
    /// places left part-way become resume points on the first matching one
    /// (or, for a title not in the catalog, as the add-on title its IMDb id
    /// names). Returns how many rows changed.
    pub(crate) fn apply_trakt_history(&self) -> Result<usize> {
        let sources = self.trakt_sources();
        let library = &self.library;
        let paused = library.trakt_paused()?;
        let mut updates = Vec::new();

        // Films.
        let watched_films = library.trakt_watched(false)?;
        let lookups = library.trakt_lookups_found(false)?;
        let titles = watched_films.iter().map(|row| &row.title).chain(paused.iter().filter(|place| !place.title.show).map(|place| &place.title));
        let keys = ids::match_keys(titles, &lookups);
        let found = self.catalog.movies_matching(&sources, &keys)?;
        let films = Candidates::new(found.iter().map(CatalogTitle::of_movie).collect(), lookups, &sources);
        for row in &watched_films {
            for film in films.matches(&row.title) {
                updates.push(watched(&film.id, MediaKind::Movie, &film.source_id, None, film.duration_seconds, row.watched_at));
            }
        }

        // Shows, and their episodes in the catalog.
        let watched_episodes = library.trakt_watched(true)?;
        let lookups = library.trakt_lookups_found(true)?;
        let mut shows: HashMap<i64, &TraktTitle> = HashMap::new();
        for title in watched_episodes.iter().map(|row| &row.title).chain(paused.iter().filter(|place| place.title.show).map(|place| &place.title)) {
            if let Some(trakt_id) = title.ids.trakt {
                shows.entry(trakt_id).or_insert(title);
            }
        }
        let keys = ids::match_keys(shows.values().copied(), &lookups);
        let found = self.catalog.series_matching(&sources, &keys)?;
        let series = Candidates::new(found.iter().map(CatalogTitle::of_series).collect(), lookups, &sources);
        let series_of_show: HashMap<i64, Vec<String>> = shows
            .iter()
            .map(|(trakt_id, title)| (*trakt_id, series.matches(title).into_iter().map(|show| show.id.clone()).collect()))
            .collect();
        let mut series_ids: Vec<String> = series_of_show.values().flatten().cloned().collect();
        series_ids.sort();
        series_ids.dedup();
        let all_episodes = self.catalog.episodes_of_series(&series_ids)?;
        let mut episodes: HashMap<&str, HashMap<(i64, i64), &Episode>> = HashMap::new();
        for episode in &all_episodes {
            episodes.entry(episode.series_id.as_str()).or_default().insert((episode.season, episode.number), episode);
        }
        let episode_in = |trakt_id: Option<i64>, season: i64, number: i64| -> Vec<&Episode> {
            let Some(series_ids) = trakt_id.and_then(|id| series_of_show.get(&id)) else { return Vec::new() };
            series_ids.iter().filter_map(|id| episodes.get(id.as_str())?.get(&(season, number)).copied()).collect()
        };
        for row in &watched_episodes {
            for episode in episode_in(row.title.ids.trakt, row.season, row.number) {
                updates.push(watched(
                    &episode.id,
                    MediaKind::Episode,
                    &episode.source_id,
                    Some(&episode.series_id),
                    episode.duration_seconds,
                    row.watched_at,
                ));
            }
        }

        // Places left part-way: one resume point each, so a film two sources
        // carry doesn't make two Continue Watching cards.
        for place in &paused {
            let length = |known: Option<i64>| known.filter(|seconds| *seconds > 0).or(place.runtime_seconds).filter(|seconds| *seconds > 0);
            if !place.title.show {
                match films.matches(&place.title).first() {
                    Some(film) => updates.extend(length(film.duration_seconds).map(|seconds| resume(&film.id, MediaKind::Movie, &film.source_id, None, seconds, place))),
                    None => {
                        if let (Some(imdb), Some(seconds)) = (&place.title.ids.imdb, length(None)) {
                            updates.push(resume(&ids::addon_movie_id(imdb), MediaKind::Movie, ADDON_SOURCE, None, seconds, place));
                        }
                    }
                }
            } else {
                match episode_in(place.title.ids.trakt, place.season, place.number).first() {
                    Some(episode) => updates.extend(length(episode.duration_seconds).map(|seconds| {
                        resume(&episode.id, MediaKind::Episode, &episode.source_id, Some(&episode.series_id), seconds, place)
                    })),
                    None => {
                        if let (Some(imdb), Some(seconds)) = (&place.title.ids.imdb, length(None)) {
                            let id = ids::addon_episode_id(imdb, place.season, place.number);
                            let series_id = ids::addon_series_id(imdb);
                            updates.push(resume(&id, MediaKind::Episode, ADDON_SOURCE, Some(&series_id), seconds, place));
                        }
                    }
                }
            }
        }
        self.merge_trakt_progress(updates)
    }

    /// Applies Trakt's watched episodes to a series' episodes just fetched
    /// from its provider (Xtream sends them only when a series is opened), so
    /// watched marks and Up Next are right there too.
    pub(crate) fn apply_trakt_to_episodes(&self, series: &Series, episodes: &[Episode]) {
        if episodes.is_empty() || series.source_id == DEMO_SOURCE_ID || !self.trakt_history_on() {
            return;
        }
        let result = (|| -> Result<usize> {
            let candidates = Candidates::new(vec![CatalogTitle::of_series(series)], self.library.trakt_lookups_found(true)?, &[]);
            let by_number = ids::by_season_and_number(episodes);
            let mut updates = Vec::new();
            for show in self.library.trakt_watched_shows()? {
                let Some(trakt_id) = show.ids.trakt.filter(|_| !candidates.matches(&show).is_empty()) else { continue };
                for row in self.library.trakt_watched_of_show(trakt_id)? {
                    if let Some(episode) = by_number.get(&(row.season, row.number)) {
                        updates.push(watched(
                            &episode.id,
                            MediaKind::Episode,
                            &episode.source_id,
                            Some(&series.id),
                            episode.duration_seconds,
                            row.watched_at,
                        ));
                    }
                }
            }
            self.merge_trakt_progress(updates)
        })();
        if let Err(error) = result {
            tracing::warn!("what Trakt says was watched couldn't be applied to a series' episodes: {error}");
        }
    }

    /// Writes Trakt's progress where it's later than Xorvio's, straight to
    /// the library (so none of it goes back out). Watched on Trakt wins over
    /// a place here that's no newer; a place on Trakt wins over anything here
    /// older than it. Returns how many rows changed.
    fn merge_trakt_progress(&self, updates: Vec<PlaybackProgress>) -> Result<usize> {
        let item_ids: Vec<String> = updates.iter().map(|update| update.item_id.clone()).collect();
        let local = self.library.progress_many(&item_ids)?;
        let mut taken: Vec<PlaybackProgress> = updates
            .into_iter()
            .filter(|update| match local.get(&update.item_id) {
                None => update.finished || update.position_seconds > 0.0,
                Some(here) if update.finished => !here.finished && here.updated_at <= update.updated_at,
                Some(here) => update.position_seconds > 0.0 && update.updated_at > here.updated_at,
            })
            .collect();
        // Oldest first, so where Trakt says two things of one title the later stays.
        taken.sort_by_key(|update| update.updated_at);
        self.library.save_progress_many(&taken)
    }

    // MARK: Watchlist

    /// Reconciles the watchlist's films and series with Trakt's; returns how
    /// many entries changed here.
    async fn sync_trakt_watchlist(&self, activities: &Value) -> Result<usize> {
        let trakt = &self.trakt;
        let entries: Vec<WatchlistEntry> = self
            .library
            .watchlist()?
            .into_iter()
            .filter(|entry| matches!(entry.kind, MediaKind::Movie | MediaKind::Series))
            .collect();
        let there = activity(activities, &[("movies", "watchlisted_at"), ("shows", "watchlisted_at")]);
        let here = local_key(&entries);
        if !there.is_empty() && trakt.activity("watchlist").as_deref() == Some(there.as_str()) && trakt.activity("watchlist.here").as_deref() == Some(here.as_str()) {
            return Ok(0);
        }

        let mut local = Vec::new();
        for entry in &entries {
            let ids = match self.trakt_title_of(entry) {
                Some(title) => trakt.resolve(&title).await,
                None => None,
            };
            local.push(LocalListed { item_id: entry.id.clone(), show: entry.kind == MediaKind::Series, ids });
        }
        let mut remote = listed(&trakt.get("/sync/watchlist/movies").await?, false);
        remote.extend(listed(&trakt.get("/sync/watchlist/shows").await?, true));
        let plan = reconcile(&local, &remote, &self.library.trakt_listed()?);

        let mut changed = 0;
        for item_id in &plan.remove_here {
            self.library.remove_from_watchlist(item_id)?;
            changed += 1;
        }
        let mut agreed = plan.agreed;
        for item in &plan.add_here {
            let Some(entry) = self.watchlist_entry_for(item)? else { continue };
            self.library.add_to_watchlist(&entry)?;
            agreed.push(TraktListed { title: item.title.clone(), item_ids: vec![entry.id] });
            changed += 1;
        }
        if !plan.add_there.is_empty() {
            trakt.post("/sync/watchlist", &list_body(&plan.add_there)).await?;
        }
        if !plan.remove_there.is_empty() {
            trakt.post("/sync/watchlist/remove", &list_body(&plan.remove_there)).await?;
        }
        self.library.replace_trakt_listed(&agreed)?;
        // Trakt's activity as read before sending anything: the next sync
        // reads again what was just sent, and finds both sides agree.
        trakt.set_activity("watchlist", &there);
        let now_here: Vec<WatchlistEntry> =
            self.library.watchlist()?.into_iter().filter(|entry| matches!(entry.kind, MediaKind::Movie | MediaKind::Series)).collect();
        trakt.set_activity("watchlist.here", &local_key(&now_here));
        Ok(changed)
    }

    /// A watchlist entry for a title on Trakt's watchlist: the catalog's copy
    /// (in the first source that has one), else the add-on title its IMDb id
    /// names, with Trakt's title and year. `None` when it can be neither.
    fn watchlist_entry_for(&self, item: &RemoteListed) -> Result<Option<WatchlistEntry>> {
        let sources = self.trakt_sources();
        let keys = ids::match_keys([&item.title], &[]);
        let titles: Vec<CatalogTitle> = if item.title.show {
            self.catalog.series_matching(&sources, &keys)?.iter().map(CatalogTitle::of_series).collect()
        } else {
            self.catalog.movies_matching(&sources, &keys)?.iter().map(CatalogTitle::of_movie).collect()
        };
        let candidates = Candidates::new(titles, Vec::new(), &sources);
        let kind = if item.title.show { MediaKind::Series } else { MediaKind::Movie };
        let entry = |id: String, source_id: &str, title: &str, year: Option<i64>, artwork_url: Option<String>| WatchlistEntry {
            id,
            kind,
            source_id: source_id.to_string(),
            title: title.to_string(),
            subtitle: year.map(|year| year.to_string()),
            artwork_url,
            added_at: item.listed_at.unwrap_or_else(now),
            channel_id: None,
            start: None,
            end: None,
            remind: false,
        };
        if let Some(found) = candidates.matches(&item.title).first() {
            return Ok(Some(entry(found.id.clone(), &found.source_id, &found.title, found.year, found.artwork_url.clone())));
        }
        let Some(imdb) = &item.title.ids.imdb else { return Ok(None) };
        let id = if item.title.show { ids::addon_series_id(imdb) } else { ids::addon_movie_id(imdb) };
        Ok(Some(entry(id, ADDON_SOURCE, &item.name, item.title.year, None)))
    }
}

/// Finished progress for a title watched on Trakt.
fn watched(item_id: &str, kind: MediaKind, source_id: &str, series_id: Option<&str>, length: Option<i64>, at: Timestamp) -> PlaybackProgress {
    let duration = length.filter(|seconds| *seconds > 0).unwrap_or(0) as f64;
    PlaybackProgress {
        item_id: item_id.to_string(),
        kind,
        source_id: source_id.to_string(),
        series_id: series_id.map(str::to_string),
        position_seconds: duration,
        duration_seconds: duration,
        finished: true,
        updated_at: at,
    }
}

/// A resume point for a place left part-way on Trakt, its percent turned
/// into seconds of `length`.
fn resume(item_id: &str, kind: MediaKind, source_id: &str, series_id: Option<&str>, length: i64, place: &TraktPaused) -> PlaybackProgress {
    let duration = length as f64;
    PlaybackProgress {
        item_id: item_id.to_string(),
        kind,
        source_id: source_id.to_string(),
        series_id: series_id.map(str::to_string),
        position_seconds: (place.progress.clamp(0.0, 100.0) / 100.0 * duration).round(),
        duration_seconds: duration,
        finished: false,
        updated_at: place.paused_at,
    }
}

// MARK: Reading Trakt's replies

/// Parts of `/sync/last_activities`, joined, to compare with the last sync's.
fn activity(activities: &Value, parts: &[(&str, &str)]) -> String {
    parts
        .iter()
        .map(|(group, field)| loose::string(activities.get(group).and_then(|group| group.get(field))).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("|")
        .trim_matches('|')
        .to_string()
}

/// Trakt's times ("2026-10-30T12:00:00.000Z") in Unix seconds.
pub(super) fn time(value: Option<&Value>) -> Option<Timestamp> {
    chrono::DateTime::parse_from_rfc3339(value?.as_str()?).ok().map(|moment| moment.timestamp())
}

/// Unix seconds the way Trakt writes times.
pub(super) fn iso_time(at: Timestamp) -> String {
    chrono::DateTime::from_timestamp(at, 0).unwrap_or_default().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A film or show object (`{"title", "year", "ids"}`), when it has a Trakt id.
pub(super) fn title(value: Option<&Value>, show: bool) -> Option<TraktTitle> {
    let value = value?;
    let ids = TraktIds::from_json(value.get("ids"));
    ids.trakt?;
    Some(TraktTitle {
        show,
        ids,
        title_key: loose::string(value.get("title")).map(|name| title_key(&name)).unwrap_or_default(),
        year: loose::int(value.get("year")).filter(|year| *year > 0),
    })
}

fn items(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

/// `/sync/watched/movies`.
fn watched_films(value: &Value) -> Vec<TraktWatched> {
    items(value)
        .filter_map(|item| {
            Some(TraktWatched {
                title: title(item.get("movie"), false)?,
                season: 0,
                number: 0,
                watched_at: time(item.get("last_watched_at"))?,
            })
        })
        .collect()
}

/// `/sync/watched/shows`: each show's seasons and their episodes.
fn watched_episodes(value: &Value) -> Vec<TraktWatched> {
    let mut rows = Vec::new();
    for item in items(value) {
        let Some(show) = title(item.get("show"), true) else { continue };
        let show_time = time(item.get("last_watched_at"));
        for season in items(item.get("seasons").unwrap_or(&Value::Null)) {
            let Some(season_number) = loose::int(season.get("number")) else { continue };
            for episode in items(season.get("episodes").unwrap_or(&Value::Null)) {
                let (Some(number), Some(watched_at)) = (loose::int(episode.get("number")), time(episode.get("last_watched_at")).or(show_time))
                else {
                    continue;
                };
                rows.push(TraktWatched { title: show.clone(), season: season_number, number, watched_at });
            }
        }
    }
    rows
}

/// `/sync/playback?extended=full`: films, and episodes with their show.
fn paused_places(value: &Value) -> Vec<TraktPaused> {
    items(value)
        .filter_map(|item| {
            let playback_id = loose::int(item.get("id"))?;
            let progress = loose::double(item.get("progress"))?;
            let paused_at = time(item.get("paused_at"))?;
            let runtime = |object: Option<&Value>| loose::int(object?.get("runtime")).filter(|minutes| *minutes > 0).map(|minutes| minutes * 60);
            match item.get("type").and_then(Value::as_str)? {
                "movie" => Some(TraktPaused {
                    playback_id,
                    title: title(item.get("movie"), false)?,
                    season: 0,
                    number: 0,
                    progress,
                    runtime_seconds: runtime(item.get("movie")),
                    paused_at,
                }),
                "episode" => {
                    let episode = item.get("episode")?;
                    Some(TraktPaused {
                        playback_id,
                        title: title(item.get("show"), true)?,
                        season: loose::int(episode.get("season"))?,
                        number: loose::int(episode.get("number"))?,
                        progress,
                        runtime_seconds: runtime(Some(episode)),
                        paused_at,
                    })
                }
                _ => None,
            }
        })
        .collect()
}

/// A title on Trakt's watchlist.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RemoteListed {
    pub title: TraktTitle,
    /// Trakt's title, for an entry made from it.
    pub name: String,
    pub listed_at: Option<Timestamp>,
}

/// `/sync/watchlist/movies` or `/shows`.
fn listed(value: &Value, show: bool) -> Vec<RemoteListed> {
    let key = if show { "show" } else { "movie" };
    items(value)
        .filter_map(|item| {
            Some(RemoteListed {
                title: title(item.get(key), show)?,
                name: loose::string(item.get(key)?.get("title")).unwrap_or_default(),
                listed_at: time(item.get("listed_at")),
            })
        })
        .collect()
}

/// `/sync/watchlist` (or its remove): films and shows by their ids.
pub(super) fn list_body(titles: &[(bool, TraktIds)]) -> Value {
    let pick = |show: bool| titles.iter().filter(|(is_show, _)| *is_show == show).map(|(_, ids)| json!({"ids": ids.to_json()})).collect::<Vec<_>>();
    json!({"movies": pick(false), "shows": pick(true)})
}

/// The watchlist's films and series as one string, to tell whether it changed.
fn local_key(entries: &[WatchlistEntry]) -> String {
    let mut ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
    ids.sort_unstable();
    ids.join("\n")
}

// MARK: Reconciling the watchlist

/// A film or series on Xorvio's watchlist, with its Trakt ids when it has any.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct LocalListed {
    pub item_id: String,
    pub show: bool,
    pub ids: Option<TraktIds>,
}

/// What a watchlist sync does.
#[derive(Debug, Default, PartialEq)]
pub(super) struct WatchlistPlan {
    /// On both sides now: what's kept as in sync.
    pub agreed: Vec<TraktListed>,
    /// Added on Trakt: to add here.
    pub add_here: Vec<RemoteListed>,
    /// Removed on Trakt since the last sync: to remove here.
    pub remove_here: Vec<String>,
    /// Added here: to add on Trakt.
    pub add_there: Vec<(bool, TraktIds)>,
    /// Removed here since the last sync: to remove on Trakt.
    pub remove_there: Vec<(bool, TraktIds)>,
}

/// Compares the two watchlists with what both had after the last sync. A
/// title on one side only was added there, unless both had it last time, in
/// which case it was removed on the other side.
pub(super) fn reconcile(local: &[LocalListed], remote: &[RemoteListed], synced: &[TraktListed]) -> WatchlistPlan {
    let mut plan = WatchlistPlan::default();
    let mut matched: HashSet<usize> = HashSet::new();
    for item in remote {
        let here: Vec<usize> = local
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.show == item.title.show && entry.ids.as_ref().is_some_and(|ids| ids.shares_id(&item.title.ids)))
            .map(|(index, _)| index)
            .collect();
        let before = synced.iter().find(|row| row.title.show == item.title.show && row.title.ids.trakt == item.title.ids.trakt);
        // Still here under an entry that couldn't be named to Trakt this time.
        let kept: Vec<usize> = before
            .map(|row| local.iter().enumerate().filter(|(_, entry)| row.item_ids.contains(&entry.item_id)).map(|(index, _)| index).collect())
            .unwrap_or_default();
        let both: Vec<usize> = here.into_iter().chain(kept).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        if !both.is_empty() {
            matched.extend(both.iter().copied());
            plan.agreed.push(TraktListed { title: item.title.clone(), item_ids: both.iter().map(|index| local[*index].item_id.clone()).collect() });
        } else if before.is_some() {
            plan.remove_there.push((item.title.show, item.title.ids.clone()));
        } else {
            plan.add_here.push(item.clone());
        }
    }
    for (index, entry) in local.iter().enumerate() {
        if matched.contains(&index) {
            continue;
        }
        let before = synced.iter().any(|row| {
            row.title.show == entry.show
                && (row.item_ids.contains(&entry.item_id) || entry.ids.as_ref().is_some_and(|ids| row.title.ids.shares_id(ids)))
        });
        if before {
            plan.remove_here.push(entry.item_id.clone());
        } else if let Some(ids) = &entry.ids
            && !plan.add_there.iter().any(|(show, added)| *show == entry.show && added.shares_id(ids))
        {
            plan.add_there.push((entry.show, ids.clone()));
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(trakt: i64, imdb: &str) -> TraktIds {
        TraktIds { trakt: Some(trakt), imdb: Some(imdb.into()), tmdb: None }
    }

    fn remote(show: bool, trakt: i64, imdb: &str) -> RemoteListed {
        RemoteListed {
            title: TraktTitle { show, ids: ids(trakt, imdb), title_key: format!("title{trakt}"), year: Some(2020) },
            name: format!("Title {trakt}"),
            listed_at: Some(1_700_000_000),
        }
    }

    fn here(item_id: &str, show: bool, imdb: Option<&str>) -> LocalListed {
        LocalListed { item_id: item_id.into(), show, ids: imdb.map(|imdb| TraktIds { imdb: Some(imdb.into()), ..TraktIds::default() }) }
    }

    fn synced(show: bool, trakt: i64, imdb: &str, item_ids: &[&str]) -> TraktListed {
        TraktListed { title: remote(show, trakt, imdb).title, item_ids: item_ids.iter().map(|id| id.to_string()).collect() }
    }

    #[test]
    fn the_first_sync_puts_both_watchlists_together() {
        let local = [here("s|mv|1", false, Some("tt0000001")), here("s|se|2", true, Some("tt0000002")), here("s|mv|3", false, None)];
        let remote = [remote(false, 1, "tt0000001"), remote(true, 4, "tt0000004")];
        let plan = reconcile(&local, &remote, &[]);
        assert_eq!(plan.agreed, [synced(false, 1, "tt0000001", &["s|mv|1"])]);
        assert_eq!(plan.add_here.iter().map(|item| item.title.ids.trakt).collect::<Vec<_>>(), [Some(4)]);
        assert_eq!(plan.add_there, [(true, TraktIds { imdb: Some("tt0000002".into()), ..TraktIds::default() })]);
        assert!(plan.remove_here.is_empty() && plan.remove_there.is_empty(), "nothing is removed on a first sync");
    }

    #[test]
    fn removals_on_either_side_are_told_from_titles_never_synced() {
        let last_time = [
            synced(false, 1, "tt0000001", &["s|mv|1"]),
            synced(true, 2, "tt0000002", &["s|se|2"]),
            synced(false, 3, "tt0000003", &["addon|movie|tt0000003"]),
        ];
        // Removed here: 2. Removed on Trakt: 3. Both still have 1. New on each side: 5 there, 6 here.
        let local = [here("s|mv|1", false, Some("tt0000001")), here("addon|movie|tt0000003", false, Some("tt0000003")), here("s|mv|6", false, Some("tt0000006"))];
        let remote = [remote(false, 1, "tt0000001"), remote(true, 2, "tt0000002"), remote(false, 5, "tt0000005")];
        let plan = reconcile(&local, &remote, &last_time);
        assert_eq!(plan.agreed.len(), 1);
        assert_eq!(plan.remove_there, [(true, ids(2, "tt0000002"))]);
        assert_eq!(plan.remove_here, ["addon|movie|tt0000003"]);
        assert_eq!(plan.add_here.iter().map(|item| item.title.ids.trakt).collect::<Vec<_>>(), [Some(5)]);
        assert_eq!(plan.add_there.len(), 1);
    }

    #[test]
    fn an_entry_that_cant_be_named_this_time_isnt_taken_for_removed() {
        let last_time = [synced(false, 1, "tt0000001", &["s|mv|1"])];
        let local = [here("s|mv|1", false, None)];
        let plan = reconcile(&local, &[remote(false, 1, "tt0000001")], &last_time);
        assert_eq!(plan.agreed, last_time);
        assert!(plan.remove_there.is_empty() && plan.add_there.is_empty());
        // Gone from Trakt, it goes from here too, named or not.
        let plan = reconcile(&local, &[], &last_time);
        assert_eq!(plan.remove_here, ["s|mv|1"]);
    }

    #[test]
    fn replies_are_read() {
        let shows = json!([{
            "plays": 3, "last_watched_at": "2026-01-02T00:00:00.000Z",
            "show": {"title": "Tidewater", "year": 2025, "ids": {"trakt": 7, "imdb": "tt0000007", "tmdb": 70}},
            "seasons": [{"number": 1, "episodes": [{"number": 1, "plays": 1, "last_watched_at": "2026-01-01T00:00:00.000Z"}, {"number": 2}]}]
        }, {"show": {"title": "No id"}, "seasons": []}]);
        let rows = watched_episodes(&shows);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].season, rows[0].number, rows[0].watched_at), (1, 1, 1_767_225_600));
        assert_eq!(rows[1].watched_at, 1_767_312_000, "the show's time when the episode has none");
        assert_eq!(rows[0].title.title_key, "tidewater");
        let places = paused_places(&json!([
            {"id": 13, "type": "movie", "progress": 25.5, "paused_at": "2026-01-03T00:00:00.000Z",
             "movie": {"title": "Glass Valley", "year": 2018, "ids": {"trakt": 1}, "runtime": 90}},
            {"id": 14, "type": "episode", "progress": 50, "paused_at": "2026-01-03T00:00:00.000Z",
             "episode": {"season": 1, "number": 2, "runtime": 44}, "show": {"title": "Tidewater", "ids": {"trakt": 7}}}
        ]));
        assert_eq!(places.iter().map(|place| (place.playback_id, place.runtime_seconds)).collect::<Vec<_>>(), [(13, Some(5400)), (14, Some(2640))]);
        assert_eq!(iso_time(1_767_225_600), "2026-01-01T00:00:00.000Z");
        let activities = json!({"movies": {"watched_at": "a", "paused_at": "b"}, "episodes": {"paused_at": "c"}});
        assert_eq!(activity(&activities, &[("movies", "paused_at"), ("episodes", "paused_at")]), "b|c");
        assert_eq!(activity(&activities, &[("shows", "watchlisted_at")]), "");
    }
}
