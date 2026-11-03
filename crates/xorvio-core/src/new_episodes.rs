//! New episodes of the series someone follows: those on the watchlist, and
//! those they're caught up on (everything that had aired watched), as Xorvio
//! for Mac's NewEpisodes. Every hour the app asks for a check, which fetches
//! each series' episodes again from its source (as opening the series does,
//! one series at a time; add-on series come from their add-on, fetched by the
//! app) and compares the newest with the one seen last time. A series that
//! hasn't aired anything for two months is looked at once a day. Anything
//! newer is announced (the app sends the notification), badged "New Episode"
//! until the series is opened, and listed in Home's New Episodes row until
//! it's watched. The first look at a series only notes where it's up to.
//!
//! What was seen is kept in the library's preferences, as JSON, so it lasts
//! across launches without a schema change.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use xorvio_model::{Episode, MediaKind, Series, SourceKind, Timestamp};

use crate::continue_watching::episode_subtitle;
use crate::next_episode::AIRING_WINDOW_SECONDS;
use crate::store::catalog::title_key;
use crate::store::lock;
use crate::{Core, Result, names, now};

/// How often the app looks.
pub const CHECK_INTERVAL_SECONDS: i64 = 3600;

/// How often a series that has stopped airing is looked at.
const QUIET_INTERVAL_SECONDS: i64 = 24 * 3600;

/// Recently watched series looked at besides the watchlist's.
const WATCHED_SERIES: usize = 25;

/// How long after it airs an episode someone's caught up to counts as new
/// for Home's row without a check having found it (the same two weeks as
/// Continue Watching's "New Episode").
const RECENT_SECONDS: i64 = 14 * 86_400;

/// Add-on titles' ids start with this; the catalog doesn't hold them.
const ADDON_PREFIX: &str = "addon|";

/// Between one provider request and the next, on top of the provider's own
/// spacing, so a check never looks like a burst.
#[cfg(not(test))]
const PAUSE: Duration = Duration::from_millis(1500);
#[cfg(test)]
const PAUSE: Duration = Duration::ZERO;

/// The newest episode seen of each series, as season and number.
const LATEST_KEY: &str = "new_episodes_latest";
/// When each series was last looked at.
const LOOKED_KEY: &str = "new_episodes_looked";
/// Series with new episodes not looked at yet.
const UNSEEN_KEY: &str = "new_episodes_unseen";
/// Series with new episodes not watched yet.
const PENDING_KEY: &str = "new_episodes_pending";
/// When the last check ran.
const CHECKED_KEY: &str = "new_episodes_checked_at";

/// An episode's place in its series: season, then number.
type Marker = (i64, i64);

/// New episodes of a series, waiting to be looked at.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Unseen {
    count: i64,
}

/// New episodes of a series, waiting to be watched: every episode after `since`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Pending {
    since: Marker,
    /// When the latest of them were found, which orders Home's New Episodes row.
    found_at: Timestamp,
}

/// The last look at a series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Looked {
    at: Timestamp,
    /// When its newest episode aired, when the source says.
    aired: Option<Timestamp>,
}

/// New episodes found by a check, ready for a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEpisodes {
    pub series_id: String,
    /// The first of them, which the notification's Play button plays.
    pub episode_id: String,
    /// "New episode of Tidewater", "3 new episodes of Tidewater"
    pub title: String,
    /// "S02 E04 · The Long Reach", "Starting with S02 E04 · The Long Reach"
    pub body: String,
    pub count: usize,
}

/// The newest regular episode that has aired (specials don't count, and an
/// episode without a date counts as aired). A source lists episodes it
/// can play, so a date ahead of today is rare: TMDB's are never among them.
pub(crate) fn newest(episodes: &[Episode], now: Timestamp) -> Option<Marker> {
    episodes.iter().filter(|episode| counts(episode, now)).map(marker).max()
}

/// Regular episodes after `since` that have aired, in order.
pub(crate) fn after(episodes: &[Episode], since: Marker, now: Timestamp) -> Vec<&Episode> {
    let mut fresh: Vec<&Episode> = episodes.iter().filter(|episode| counts(episode, now) && marker(episode) > since).collect();
    fresh.sort_by_key(|episode| marker(episode));
    fresh
}

/// Whether any episode after `since` has aired and isn't watched.
fn waiting(episodes: &[Episode], since: Marker, finished: &HashSet<String>, now: Timestamp) -> bool {
    after(episodes, since, now).iter().any(|episode| !finished.contains(&episode.id))
}

/// Whether the episode after the furthest one watched aired in the last two weeks.
fn next_just_aired(episodes: &[Episode], finished: &HashSet<String>, now: Timestamp) -> bool {
    let mut regular: Vec<&Episode> = episodes.iter().filter(|episode| episode.season > 0).collect();
    regular.sort_by_key(|episode| marker(episode));
    let Some(furthest) = regular.iter().rposition(|episode| finished.contains(&episode.id)) else { return false };
    regular
        .get(furthest + 1)
        .is_some_and(|next| counts(next, now) && next.air_date.is_some_and(|aired| now - aired <= RECENT_SECONDS))
}

/// Whether a series is due a look: each check, unless the newest episode
/// aired over two months ago, and then once a day.
fn due(looked: Option<&Looked>, now: Timestamp) -> bool {
    let Some(looked) = looked else { return true };
    let quiet = looked.aired.is_some_and(|aired| now - aired > AIRING_WINDOW_SECONDS);
    !quiet || now - looked.at >= QUIET_INTERVAL_SECONDS
}

/// When the newest regular episode aired, if its source says.
fn newest_aired_on(episodes: &[Episode], now: Timestamp) -> Option<Timestamp> {
    episodes.iter().filter(|episode| counts(episode, now)).max_by_key(|episode| marker(episode)).and_then(|episode| episode.air_date)
}

/// Providers date episodes by the day (kept as midday UTC), so one dated
/// today counts from the morning.
const DAY_SECONDS: i64 = 86_400;

fn counts(episode: &Episode, now: Timestamp) -> bool {
    episode.season > 0 && episode.air_date.is_none_or(|aired| aired <= now + DAY_SECONDS)
}

fn marker(episode: &Episode) -> Marker {
    (episode.season, episode.number)
}

/// One card per show: two sources can both carry it.
fn show_key(show: &Series) -> String {
    match show.tmdb_id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => format!("tmdb-{id}"),
        None => title_key(&show.title),
    }
}

/// What a notification says about `fresh` new episodes of `show`.
fn announcement(show: &Series, fresh: &[&Episode]) -> Option<NewEpisodes> {
    let first = fresh.first()?;
    let title = names::media_title(&show.title);
    let episode = episode_subtitle(first);
    Some(NewEpisodes {
        series_id: show.id.clone(),
        episode_id: first.id.clone(),
        title: match fresh.len() {
            1 => format!("New episode of {title}"),
            count => format!("{count} new episodes of {title}"),
        },
        body: if fresh.len() == 1 { episode } else { format!("Starting with {episode}") },
        count: fresh.len(),
    })
}

impl Core {
    /// Whether an hour has passed since the last check.
    pub fn new_episodes_due(&self, now: Timestamp) -> Result<bool> {
        let last = self.library.preference(CHECKED_KEY)?.and_then(|value| value.parse::<Timestamp>().ok()).unwrap_or(0);
        Ok(now - last >= CHECK_INTERVAL_SECONDS)
    }

    /// Followed add-on series due a look, for the app to fetch from their
    /// add-on and hand to [`Core::check_new_episodes`].
    pub fn followed_addon_series(&self, now: Timestamp) -> Result<Vec<String>> {
        let (followed, _) = self.followed()?;
        let looked: HashMap<String, Looked> = self.stored(LOOKED_KEY)?;
        Ok(followed.into_iter().filter(|id| id.starts_with(ADDON_PREFIX) && due(looked.get(id), now)).collect())
    }

    /// Add-on series with new episodes not watched yet, whose episodes the
    /// app hands to [`Core::new_episode_series`].
    pub fn pending_addon_series(&self) -> Result<Vec<String>> {
        let pending: HashMap<String, Pending> = self.stored(PENDING_KEY)?;
        Ok(pending.into_keys().filter(|id| id.starts_with(ADDON_PREFIX)).collect())
    }

    /// Looks for new episodes of the series someone follows, and returns
    /// what's new since the last look, to announce. `addon_shows` are the
    /// followed add-on series with their episodes as their add-on has them
    /// now. Series hidden, locked or left out by the profile are skipped
    /// (and looked at once they're back).
    pub async fn check_new_episodes(&self, now: Timestamp, addon_shows: Vec<(Series, Vec<Episode>)>) -> Result<Vec<NewEpisodes>> {
        self.library.set_preference(CHECKED_KEY, Some(&now.to_string()))?;
        let (followed, saved) = self.followed()?;
        let active: HashSet<String> = self.active_source_ids().into_iter().collect();
        let excluded = self.excluded(false)?;
        let looked: HashMap<String, Looked> = self.stored(LOOKED_KEY)?;
        let catalog_shows = self
            .catalog
            .series_by_ids(&followed)?
            .into_iter()
            .filter(|show| active.contains(&show.source_id) && !excluded.series(show) && due(looked.get(&show.id), now));
        let addon_shows = addon_shows.into_iter().filter(|(show, _)| followed.contains(&show.id) && !excluded.series(show));
        let shows: Vec<(Series, Option<Vec<Episode>>)> =
            catalog_shows.map(|show| (show, None)).chain(addon_shows.map(|(show, episodes)| (show, Some(episodes)))).collect();

        let latest: HashMap<String, Marker> = self.stored(LATEST_KEY)?;
        let mut newest_seen: HashMap<String, Marker> = HashMap::new();
        let mut looks: HashMap<String, Looked> = HashMap::new();
        // Series with something new: since which episode, and how many.
        let mut news: Vec<(String, Marker, i64)> = Vec::new();
        let mut found = Vec::new();
        let mut asked = false;
        for (show, given) in shows {
            let episodes = match given {
                Some(episodes) => episodes,
                None => {
                    if self.fetches_episodes(&show) {
                        if asked {
                            tokio::time::sleep(PAUSE).await;
                        }
                        asked = true;
                    }
                    self.current_episodes(&show).await
                }
            };
            looks.insert(show.id.clone(), Looked { at: now, aired: newest_aired_on(&episodes, now) });
            let Some(newest) = newest(&episodes, now) else { continue };
            newest_seen.insert(show.id.clone(), newest);
            let Some(previous) = latest.get(&show.id).copied().filter(|previous| *previous < newest) else { continue };
            // A series that isn't saved is followed only while someone's caught
            // up: the newest one there was last time has been watched.
            if !saved.contains(&show.id) {
                let Some(last) = episodes.iter().find(|episode| marker(episode) == previous) else { continue };
                if !self.library.progress(&last.id)?.is_some_and(|progress| progress.finished) {
                    continue;
                }
            }
            let fresh = after(&episodes, previous, now);
            let Some(announced) = announcement(&show, &fresh) else { continue };
            news.push((show.id.clone(), previous, fresh.len() as i64));
            found.push(announced);
        }

        // Saved over what's there now rather than what was there when the
        // check began, so a series opened (or episodes marked watched) while
        // it ran isn't undone.
        let mut latest: HashMap<String, Marker> = self.stored(LATEST_KEY)?;
        latest.extend(newest_seen);
        let mut looked: HashMap<String, Looked> = self.stored(LOOKED_KEY)?;
        looked.extend(looks);
        let mut unseen: HashMap<String, Unseen> = self.stored(UNSEEN_KEY)?;
        let mut pending: HashMap<String, Pending> = self.stored(PENDING_KEY)?;
        for (id, since, count) in news {
            unseen.entry(id.clone()).or_default().count += count;
            // Earlier ones still unwatched keep their place: the row lasts until all are watched.
            pending.entry(id).and_modify(|waiting| waiting.found_at = now).or_insert(Pending { since, found_at: now });
        }
        // Badges and rows for series no longer followed have nowhere to show.
        let followed: HashSet<&str> = followed.iter().map(String::as_str).collect();
        looked.retain(|id, _| followed.contains(id.as_str()));
        unseen.retain(|id, _| followed.contains(id.as_str()));
        pending.retain(|id, _| followed.contains(id.as_str()));
        self.store(LATEST_KEY, &latest)?;
        self.store(LOOKED_KEY, &looked)?;
        self.store(UNSEEN_KEY, &unseen)?;
        self.store(PENDING_KEY, &pending)?;
        Ok(found)
    }

    /// Series with new episodes not looked at yet, and how many.
    pub fn new_episode_counts(&self) -> Result<HashMap<String, i64>> {
        let unseen: HashMap<String, Unseen> = self.stored(UNSEEN_KEY)?;
        Ok(unseen.into_iter().map(|(id, waiting)| (id, waiting.count)).collect())
    }

    /// Opening a series clears its badge. True when there was one. It stays
    /// in Home's New Episodes row until the new episodes are watched.
    pub fn mark_episodes_seen(&self, series_id: &str) -> Result<bool> {
        let mut unseen: HashMap<String, Unseen> = self.stored(UNSEEN_KEY)?;
        if unseen.remove(series_id).is_none() {
            return Ok(false);
        }
        self.store(UNSEEN_KEY, &unseen)?;
        Ok(true)
    }

    /// Home's New Episodes: series someone follows with an episode that's
    /// aired and that they haven't watched. First those a check found, the
    /// latest found first, until their new episodes are watched; then
    /// followed series whose next episode aired in the last two weeks, as far
    /// as the catalog knows their episodes (dates come from the source). One
    /// card per show. `addon_shows` holds the add-on series from
    /// [`Core::pending_addon_series`] with their episodes.
    pub fn new_episode_series(&self, limit: usize, addon_shows: &HashMap<String, (Series, Vec<Episode>)>) -> Result<Vec<Series>> {
        let at = now();
        let sources: HashSet<String> = self.active_source_ids().into_iter().collect();
        let excluded = self.excluded(false)?;
        let pending: HashMap<String, Pending> = self.stored(PENDING_KEY)?;
        let mut order: Vec<(&String, &Pending)> = pending.iter().collect();
        order.sort_by(|a, b| b.1.found_at.cmp(&a.1.found_at).then_with(|| a.0.cmp(b.0)));
        let ids: Vec<String> = order.iter().map(|(id, _)| (*id).clone()).collect();
        let catalog: HashMap<String, Series> = self.catalog.series_by_ids(&ids)?.into_iter().map(|show| (show.id.clone(), show)).collect();

        let mut listed: Vec<Series> = Vec::new();
        let mut watched = Vec::new();
        for (id, entry) in order {
            let (show, episodes) = match (addon_shows.get(id), catalog.get(id)) {
                (Some((show, episodes)), _) => (show.clone(), episodes.clone()),
                (None, Some(show)) if sources.contains(&show.source_id) => (show.clone(), self.catalog.episodes(id)?),
                _ => continue,
            };
            let finished: HashSet<String> = self.library.finished_in_series(id)?.into_iter().collect();
            if !waiting(&episodes, entry.since, &finished, at) {
                // All watched (an empty list is one not fetched yet, and waits).
                if !episodes.is_empty() {
                    watched.push(id.clone());
                }
                continue;
            }
            if !excluded.series(&show) {
                listed.push(show);
            }
        }

        let (followed, _) = self.followed()?;
        for show in self.catalog.series_by_ids(&followed)? {
            if pending.contains_key(&show.id) || !sources.contains(&show.source_id) || excluded.series(&show) {
                continue;
            }
            let episodes = self.catalog.episodes(&show.id)?;
            let finished: HashSet<String> = self.library.finished_in_series(&show.id)?.into_iter().collect();
            if next_just_aired(&episodes, &finished, at) {
                listed.push(show);
            }
        }

        if !watched.is_empty() {
            let mut pending: HashMap<String, Pending> = self.stored(PENDING_KEY)?;
            for id in watched {
                pending.remove(&id);
            }
            self.store(PENDING_KEY, &pending)?;
        }
        let mut shows = HashSet::new();
        Ok(listed.into_iter().filter(|show| shows.insert(show_key(show))).take(limit).map(Self::present_series).collect())
    }

    /// The series someone follows, the watchlist's first, and which of them
    /// are saved: besides the watchlist, the latest series with something
    /// watched to the end, which count only while caught up.
    fn followed(&self) -> Result<(Vec<String>, HashSet<String>)> {
        let mut followed: Vec<String> = self
            .library
            .watchlist()?
            .into_iter()
            .filter(|entry| entry.kind == MediaKind::Series)
            .map(|entry| entry.id)
            .collect();
        let saved: HashSet<String> = followed.iter().cloned().collect();
        let mut watched = 0;
        for entry in self.library.finished(400)? {
            if watched == WATCHED_SERIES {
                break;
            }
            if let Some(series) = entry.series_id
                && !followed.contains(&series)
            {
                followed.push(series);
                watched += 1;
            }
        }
        Ok((followed, saved))
    }

    /// Someone marked episodes watched: a series now caught up (every
    /// episode that's aired watched) is followed from its newest episode on,
    /// without waiting for the next check to notice.
    pub(crate) fn follow_caught_up(&self, series_ids: &[String], now: Timestamp) -> Result<()> {
        let mut latest: HashMap<String, Marker> = self.stored(LATEST_KEY)?;
        let mut changed = false;
        for series_id in series_ids {
            let episodes = self.catalog.episodes(series_id)?;
            let Some(newest) = newest(&episodes, now) else { continue };
            let finished: HashSet<String> = self.library.finished_in_series(series_id)?.into_iter().collect();
            let caught_up = episodes.iter().filter(|episode| counts(episode, now)).all(|episode| finished.contains(&episode.id));
            if caught_up && latest.get(series_id).is_none_or(|seen| *seen < newest) {
                latest.insert(series_id.clone(), newest);
                changed = true;
            }
        }
        if changed {
            self.store(LATEST_KEY, &latest)?;
        }
        Ok(())
    }

    /// Whether this series' episodes come from asking its provider (Xtream
    /// sends them only on request); other sources' come with each refresh.
    fn fetches_episodes(&self, show: &Series) -> bool {
        show.remote_id.is_some() && self.config(&show.source_id).is_ok_and(|config| config.kind == SourceKind::Xtream)
    }

    /// A series' episodes as its source has them now. Opening a series asks
    /// an Xtream provider once a session; a check asks again, and keeps what
    /// was there when the provider can't be reached.
    async fn current_episodes(&self, show: &Series) -> Vec<Episode> {
        lock(&self.episodes_fetched).remove(&show.id);
        match self.source_episodes(show).await {
            Ok(episodes) => episodes,
            Err(error) => {
                tracing::warn!("new episodes of a series couldn't be looked for: {}", crate::error::describe(&error));
                self.catalog.episodes(&show.id).unwrap_or_default()
            }
        }
    }

    fn stored<T: DeserializeOwned + Default>(&self, key: &str) -> Result<T> {
        Ok(self.library.preference(key)?.and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default())
    }

    fn store<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.library.set_preference(key, Some(&serde_json::to_string(value)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xorvio_model::StreamLocator;

    const NOW: Timestamp = 1_800_000_000;

    /// (season, number, days ago it aired; negative is still to come)
    fn episodes(list: &[(i64, i64, Option<i64>)]) -> Vec<Episode> {
        list.iter()
            .map(|&(season, number, days_ago)| Episode {
                id: format!("s|se|1|ep|{season}x{number}"),
                series_id: "s|se|1".into(),
                source_id: "s".into(),
                season,
                number,
                title: format!("Chapter {number}"),
                plot: None,
                duration_seconds: None,
                still_url: None,
                stream: StreamLocator::Url { url: format!("https://example.com/{season}/{number}") },
                air_date: days_ago.map(|days| NOW - days * 86_400),
            })
            .collect()
    }

    #[test]
    fn the_newest_aired_regular_episode_marks_where_a_series_is() {
        let list = episodes(&[(0, 1, Some(2)), (1, 1, Some(30)), (1, 2, Some(23)), (2, 1, None), (2, 2, Some(-6))]);
        assert_eq!(newest(&list, NOW), Some((2, 1)), "specials and episodes still to come don't count; no date counts as aired");
        assert_eq!(newest(&episodes(&[(0, 1, Some(2))]), NOW), None);
    }

    #[test]
    fn what_came_after_the_last_look() {
        let list = episodes(&[(1, 2, None), (2, 1, None), (1, 1, None), (1, 3, None), (2, 2, Some(-3))]);
        let fresh: Vec<Marker> = after(&list, (1, 2), NOW).into_iter().map(marker).collect();
        assert_eq!(fresh, [(1, 3), (2, 1)], "in order, and not one that hasn't aired");
        assert!(after(&list, (2, 1), NOW).is_empty());
    }

    #[test]
    fn series_that_stopped_airing_are_looked_at_daily() {
        assert!(due(None, NOW), "never looked at");
        let airing = Looked { at: NOW - 3600, aired: Some(NOW - 5 * 86_400) };
        assert!(due(Some(&airing), NOW));
        let undated = Looked { at: NOW - 60, aired: None };
        assert!(due(Some(&undated), NOW), "no date: as if airing");
        let quiet = Looked { at: NOW - 3600, aired: Some(NOW - 90 * 86_400) };
        assert!(!due(Some(&quiet), NOW));
        assert!(due(Some(&quiet), NOW - 3600 + QUIET_INTERVAL_SECONDS));
        let list = episodes(&[(1, 1, Some(90)), (1, 2, None)]);
        assert_eq!(newest_aired_on(&list, NOW), None, "the newest episode's date, not an older one's");
    }

    #[test]
    fn new_episodes_wait_until_every_one_is_watched() {
        let list = episodes(&[(1, 1, None), (1, 2, None), (1, 3, None), (1, 4, Some(-7))]);
        let finished = |done: &[&Episode]| done.iter().map(|episode| episode.id.clone()).collect::<HashSet<_>>();
        assert!(waiting(&list, (1, 1), &finished(&[]), NOW));
        assert!(waiting(&list, (1, 1), &finished(&[&list[1]]), NOW));
        assert!(!waiting(&list, (1, 1), &finished(&[&list[1], &list[2]]), NOW), "one still to air doesn't count");
    }

    #[test]
    fn notifications_name_the_first_new_episode() {
        let show = Series {
            id: "s|se|1".into(),
            source_id: "s".into(),
            title: "EN - Tidewater".into(),
            year: None,
            poster_url: None,
            backdrop_url: None,
            category_id: None,
            rating: None,
            added_at: None,
            plot: None,
            genre: None,
            tmdb_id: None,
            imdb_id: None,
            remote_id: None,
        };
        let list = episodes(&[(2, 4, None), (2, 5, None)]);
        let one = announcement(&show, &[&list[0]]).unwrap();
        assert_eq!((one.title.as_str(), one.body.as_str(), one.count), ("New episode of Tidewater", "S02 E04 · Chapter 4", 1));
        let two = announcement(&show, &[&list[0], &list[1]]).unwrap();
        assert_eq!((two.title.as_str(), two.body.as_str()), ("2 new episodes of Tidewater", "Starting with S02 E04 · Chapter 4"));
        assert_eq!(two.episode_id, list[0].id);
        assert!(announcement(&show, &[]).is_none());
    }
}
