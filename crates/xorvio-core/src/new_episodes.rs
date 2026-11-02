//! New episodes of the series someone follows: those on the watchlist, and
//! those they're caught up on (everything that had aired watched), as Xorvio
//! for Mac's NewEpisodes. Every six hours the app asks for a check, which
//! fetches each series' episodes again from its source (as opening the
//! series does, one series at a time) and compares the newest with the one
//! seen last time. Anything newer is announced (the app sends the
//! notification) and badged "New Episode" until the series is opened. The
//! first look at a series only notes where it's up to.
//!
//! What was seen is kept in the library's preferences, as JSON, so it lasts
//! across launches without a schema change.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use xorvio_model::{Episode, MediaKind, Series, SourceKind, Timestamp};

use crate::continue_watching::episode_subtitle;
use crate::store::lock;
use crate::{Core, Result, names};

/// How often the app looks.
pub const CHECK_INTERVAL_SECONDS: i64 = 6 * 3600;

/// Recently watched series looked at besides the watchlist's.
const WATCHED_SERIES: usize = 25;

/// Between one provider request and the next, on top of the provider's own
/// spacing, so a check never looks like a burst.
#[cfg(not(test))]
const PAUSE: Duration = Duration::from_millis(1500);
#[cfg(test)]
const PAUSE: Duration = Duration::ZERO;

/// The newest episode seen of each series, as season and number.
const LATEST_KEY: &str = "new_episodes_latest";
/// Series with new episodes not looked at yet.
const UNSEEN_KEY: &str = "new_episodes_unseen";
/// When the last check ran.
const CHECKED_KEY: &str = "new_episodes_checked_at";

/// An episode's place in its series: season, then number.
type Marker = (i64, i64);

/// New episodes of a series, waiting to be looked at.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Unseen {
    count: i64,
    /// When they were found, which orders Home's New Episodes row.
    found_at: Timestamp,
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

/// Providers date episodes by the day (kept as midday UTC), so one dated
/// today counts from the morning.
const DAY_SECONDS: i64 = 86_400;

fn counts(episode: &Episode, now: Timestamp) -> bool {
    episode.season > 0 && episode.air_date.is_none_or(|aired| aired <= now + DAY_SECONDS)
}

fn marker(episode: &Episode) -> Marker {
    (episode.season, episode.number)
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
    /// Whether six hours have passed since the last check.
    pub fn new_episodes_due(&self, now: Timestamp) -> Result<bool> {
        let last = self.library.preference(CHECKED_KEY)?.and_then(|value| value.parse::<Timestamp>().ok()).unwrap_or(0);
        Ok(now - last >= CHECK_INTERVAL_SECONDS)
    }

    /// Looks for new episodes of the series someone follows, and returns
    /// what's new since the last look, to announce. Series hidden, locked or
    /// left out by the profile are skipped (and looked at once they're back).
    pub async fn check_new_episodes(&self, now: Timestamp) -> Result<Vec<NewEpisodes>> {
        self.library.set_preference(CHECKED_KEY, Some(&now.to_string()))?;
        let saved: Vec<String> = self
            .library
            .watchlist()?
            .into_iter()
            .filter(|entry| entry.kind == MediaKind::Series)
            .map(|entry| entry.id)
            .collect();
        let saved_set: HashSet<&str> = saved.iter().map(String::as_str).collect();
        // Series with something watched to the end recently; they count only while caught up.
        let mut watched: Vec<String> = Vec::new();
        for entry in self.library.finished(400)? {
            if let Some(series) = entry.series_id
                && !saved_set.contains(series.as_str())
                && !watched.contains(&series)
            {
                watched.push(series);
            }
        }
        watched.truncate(WATCHED_SERIES);
        let followed: Vec<String> = saved.iter().cloned().chain(watched).collect();

        let active: HashSet<String> = self.active_source_ids().into_iter().collect();
        let excluded = self.excluded(false)?;
        let shows: Vec<Series> = self
            .catalog
            .series_by_ids(&followed)?
            .into_iter()
            .filter(|show| active.contains(&show.source_id) && !excluded.series(show))
            .collect();

        let mut latest: HashMap<String, Marker> = self.stored(LATEST_KEY)?;
        let mut unseen: HashMap<String, Unseen> = self.stored(UNSEEN_KEY)?;
        let mut found = Vec::new();
        let mut asked = false;
        for show in &shows {
            if self.fetches_episodes(show) {
                if asked {
                    tokio::time::sleep(PAUSE).await;
                }
                asked = true;
            }
            let episodes = self.current_episodes(show).await;
            let Some(newest) = newest(&episodes, now) else { continue };
            let Some(previous) = latest.insert(show.id.clone(), newest).filter(|previous| *previous < newest) else { continue };
            // A series that isn't saved is followed only while someone's caught
            // up: the newest one there was last time has been watched.
            if !saved_set.contains(show.id.as_str()) {
                let Some(last) = episodes.iter().find(|episode| marker(episode) == previous) else { continue };
                if !self.library.progress(&last.id)?.is_some_and(|progress| progress.finished) {
                    continue;
                }
            }
            let fresh = after(&episodes, previous, now);
            let Some(announced) = announcement(show, &fresh) else { continue };
            let waiting = unseen.entry(show.id.clone()).or_default();
            waiting.count += fresh.len() as i64;
            waiting.found_at = now;
            found.push(announced);
        }
        // Badges for series no longer followed have nowhere to show.
        unseen.retain(|id, _| followed.contains(id));
        self.store(LATEST_KEY, &latest)?;
        self.store(UNSEEN_KEY, &unseen)?;
        Ok(found)
    }

    /// Series with new episodes not looked at yet, and how many.
    pub fn new_episode_counts(&self) -> Result<HashMap<String, i64>> {
        let unseen: HashMap<String, Unseen> = self.stored(UNSEEN_KEY)?;
        Ok(unseen.into_iter().map(|(id, waiting)| (id, waiting.count)).collect())
    }

    /// Opening a series clears its badge. True when there was one.
    pub fn mark_episodes_seen(&self, series_id: &str) -> Result<bool> {
        let mut unseen: HashMap<String, Unseen> = self.stored(UNSEEN_KEY)?;
        if unseen.remove(series_id).is_none() {
            return Ok(false);
        }
        self.store(UNSEEN_KEY, &unseen)?;
        Ok(true)
    }

    /// Series with unseen new episodes, the latest found first.
    pub(crate) fn series_with_unseen_episodes(&self) -> Result<Vec<String>> {
        let unseen: HashMap<String, Unseen> = self.stored(UNSEEN_KEY)?;
        let mut ids: Vec<(String, Timestamp)> = unseen.into_iter().map(|(id, waiting)| (id, waiting.found_at)).collect();
        ids.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Ok(ids.into_iter().map(|(id, _)| id).collect())
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
