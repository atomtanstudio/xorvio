//! Continue Watching: what's part-way through, and carrying on past the end
//! of an episode, the next episode of each series someone's watching or has
//! on the watchlist.

use std::collections::{HashMap, HashSet};

use xorvio_model::{ContinueItem, Episode, MediaKind, Movie, PlaybackProgress, Series, Timestamp, UpNextReason};

use crate::details::DetailsRecord;
use crate::edits::Excluded;
use crate::next_episode::{self, Reason};
use crate::sources::ids;
use crate::store::catalog::title_key;
use crate::store::library::{fraction, is_resumable};
use crate::{Core, Result, names, now};

/// How many recently watched series are looked at for a next episode.
const WATCHED_SERIES_LIMIT: usize = 30;

/// How long after it airs an episode's card says "New Episode".
const NEW_EPISODE_SECONDS: i64 = 14 * 86_400;

impl Core {
    /// Items part-way through plus next episodes up, most recent first, one
    /// card per show.
    /// Progress on titles that came from add-ons (`source_id` "addon"): the
    /// latest entry for each film or show, part-watched or finished, newest
    /// first. The catalog doesn't hold these titles, so the app turns them
    /// into Continue Watching cards with the add-ons' details.
    pub fn addon_progress(&self, limit: usize) -> Result<Vec<PlaybackProgress>> {
        let mut entries: Vec<PlaybackProgress> = self
            .library
            .resumable(limit.saturating_mul(4).max(20))?
            .into_iter()
            .chain(self.library.finished(limit.saturating_mul(4).max(20))?)
            .filter(|entry| entry.source_id == "addon")
            .collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.updated_at));
        let mut seen = HashSet::new();
        Ok(entries
            .into_iter()
            .filter(|entry| seen.insert(entry.series_id.clone().unwrap_or_else(|| entry.item_id.clone())))
            .take(limit)
            .collect())
    }

    /// Takes a card off Continue Watching. A film or episode left part-way
    /// forgets where it got to (here, on its media server, and on Trakt); an
    /// episode's series stays off until something newer happens in it
    /// (another episode watched, or a new one airing).
    pub fn remove_from_continue_watching(&self, item_id: &str) -> Result<()> {
        if let Some(entry) = self.library.progress(item_id)?.filter(|entry| !entry.finished) {
            self.library.remove_progress(item_id)?;
            self.tell_trakt_place_forgotten(item_id, entry.kind);
            // A media server forgets the place too, as its own apps do, or
            // the next refresh would bring it back.
            self.send_progress_to_server(&PlaybackProgress { position_seconds: 0.0, ..entry });
        }
        if let Some(series_id) = ids::series_of_episode(item_id) {
            self.library.dismiss_show(series_id, now())?;
        }
        Ok(())
    }

    /// Drops cards for shows taken off Continue Watching since their date.
    pub fn without_dismissed(&self, items: &mut Vec<ContinueItem>) -> Result<()> {
        let dismissed = self.library.dismissed_shows()?;
        if dismissed.is_empty() {
            return Ok(());
        }
        items.retain(|item| {
            let show = ids::series_of_episode(&item.item_id);
            show.and_then(|show| dismissed.get(show)).is_none_or(|at| item.updated_at > *at)
        });
        Ok(())
    }

    /// Episode ids of a series watched to the end.
    pub fn finished_in_series(&self, series_id: &str) -> Result<Vec<String>> {
        self.library.finished_in_series(series_id)
    }

    /// Films and episodes part-way through, and next episodes up, most
    /// recent first, one card per show. Titles hidden, locked or left out by
    /// the profile stay off the row, and are back when they're shown again.
    pub fn continue_watching(&self, limit: usize) -> Result<Vec<ContinueItem>> {
        let now = now();
        let active: HashSet<String> = self.active_source_ids().into_iter().collect();
        let excluded = self.excluded(false)?;

        // When each series last had an episode watched to the end, most recent first.
        let mut last_finished: HashMap<String, Timestamp> = HashMap::new();
        let mut watched_series: Vec<String> = Vec::new();
        for entry in self.library.finished(500)? {
            let Some(series) = entry.series_id.filter(|_| active.contains(&entry.source_id)) else { continue };
            if !last_finished.contains_key(&series) {
                last_finished.insert(series.clone(), entry.updated_at);
                watched_series.push(series);
            }
        }

        // An episode left part-way gives way to the next one once a later episode is finished.
        let progress: Vec<PlaybackProgress> = self
            .library
            .resumable(limit.max(1))?
            .into_iter()
            .filter(|entry| active.contains(&entry.source_id) && is_resumable(entry))
            .filter(|entry| {
                let finished = entry.series_id.as_ref().and_then(|series| last_finished.get(series));
                finished.is_none_or(|finished| entry.updated_at >= *finished)
            })
            .collect();
        let mut items = self.resuming_items(&progress, &excluded)?;

        let resuming_series: HashSet<String> = progress.iter().filter_map(|entry| entry.series_id.clone()).collect();
        items.extend(self.up_next_items(&watched_series, &last_finished, &resuming_series, &active, &excluded, now)?);

        // One card per show, the most recent: two sources can both carry it.
        items.sort_by_key(|(date, _, _)| std::cmp::Reverse(*date));
        let mut shows = HashSet::new();
        Ok(items
            .into_iter()
            .filter(|(_, show, _)| show.as_ref().is_none_or(|key| shows.insert(key.clone())))
            .map(|(_, _, item)| item)
            .take(limit)
            .collect())
    }

    /// Cards for films and episodes left part-way, with the date that places
    /// them and the show they belong to.
    fn resuming_items(
        &self,
        progress: &[PlaybackProgress],
        excluded: &Excluded,
    ) -> Result<Vec<(Timestamp, Option<String>, ContinueItem)>> {
        let ids_of = |kind: MediaKind| -> Vec<String> {
            progress.iter().filter(|entry| entry.kind == kind).map(|entry| entry.item_id.clone()).collect()
        };
        let movies: HashMap<String, Movie> =
            self.catalog.movies_by_ids(&ids_of(MediaKind::Movie))?.into_iter().map(|movie| (movie.id.clone(), movie)).collect();
        let episodes: HashMap<String, Episode> = self
            .catalog
            .episodes_by_ids(&ids_of(MediaKind::Episode))?
            .into_iter()
            .map(|episode| (episode.id.clone(), episode))
            .collect();
        let mut series_ids: Vec<String> = episodes.values().map(|episode| episode.series_id.clone()).collect();
        series_ids.sort();
        series_ids.dedup();
        let series: HashMap<String, Series> =
            self.catalog.series_by_ids(&series_ids)?.into_iter().map(|show| (show.id.clone(), show)).collect();
        let detail_ids: Vec<String> = movies.keys().chain(series.keys()).cloned().collect();
        let details = self.catalog.details_many(&detail_ids)?;

        let mut items = Vec::new();
        for entry in progress {
            let remaining = (entry.duration_seconds > 0.0).then(|| (entry.duration_seconds - entry.position_seconds).max(0.0));
            let card = |title: String, subtitle: Option<String>, artwork_url: Option<String>| ContinueItem {
                item_id: entry.item_id.clone(),
                kind: entry.kind,
                source_id: entry.source_id.clone(),
                title,
                subtitle,
                artwork_url,
                progress: fraction(entry),
                remaining_seconds: remaining,
                up_next: false,
                reason: None,
                updated_at: entry.updated_at,
            };
            if let Some(movie) = movies.get(&entry.item_id) {
                if excluded.movie(movie) {
                    continue;
                }
                let artwork = backdrop(&details, &movie.id).or_else(|| movie.backdrop_url.clone()).or_else(|| movie.poster_url.clone());
                items.push((entry.updated_at, None, card(names::media_title(&movie.title), None, artwork)));
            } else if let Some(episode) = episodes.get(&entry.item_id)
                && let Some(show) = series.get(&episode.series_id)
                && !excluded.series(show)
            {
                let artwork = episode.still_url.clone().or_else(|| series_artwork(&details, show));
                let item = card(names::media_title(&show.title), Some(episode_subtitle(episode)), artwork);
                items.push((entry.updated_at, Some(title_key(&show.title)), item));
            }
        }
        Ok(items)
    }

    /// The next episode of every watchlist series, and of the recently watched
    /// series that have one after the furthest watched. Series already in the
    /// row with an episode part-way are skipped.
    fn up_next_items(
        &self,
        watched_series: &[String],
        last_finished: &HashMap<String, Timestamp>,
        skipping: &HashSet<String>,
        active: &HashSet<String>,
        excluded: &Excluded,
        now: Timestamp,
    ) -> Result<Vec<(Timestamp, Option<String>, ContinueItem)>> {
        let saved_at: HashMap<String, Timestamp> = self
            .library
            .watchlist()?
            .into_iter()
            .filter(|entry| entry.kind == MediaKind::Series)
            .map(|entry| (entry.id, entry.added_at))
            .collect();
        let mut ids: Vec<String> = saved_at.keys().cloned().collect();
        ids.sort();
        ids.extend(watched_series.iter().filter(|id| !saved_at.contains_key(*id)).take(WATCHED_SERIES_LIMIT).cloned());
        ids.retain(|id| !skipping.contains(id));
        let shows: Vec<Series> =
            self.catalog.series_by_ids(&ids)?.into_iter().filter(|show| active.contains(&show.source_id) && !excluded.series(show)).collect();
        let details = self.catalog.details_many(&shows.iter().map(|show| show.id.clone()).collect::<Vec<_>>())?;

        let mut items = Vec::new();
        for show in shows {
            let episodes = self.catalog.episodes(&show.id)?;
            if episodes.is_empty() {
                continue;
            }
            let finished: HashSet<String> = self.library.finished_in_series(&show.id)?.into_iter().collect();
            let Some((episode, reason)) = next_episode::find(&episodes, &finished, now) else { continue };
            // A series that isn't saved is here only to carry on from what was watched.
            let saved = saved_at.get(&show.id).copied();
            if saved.is_none() && reason != Reason::Next {
                continue;
            }
            let since = last_finished.get(&show.id).copied().or(saved).unwrap_or(0);
            // A new episode moves its series to the front.
            let aired = if reason == Reason::Start { None } else { episode.air_date.filter(|date| *date <= now) };
            let date = since.max(aired.unwrap_or(0));
            items.push((
                date,
                Some(title_key(&show.title)),
                ContinueItem {
                    item_id: episode.id.clone(),
                    kind: MediaKind::Episode,
                    source_id: show.source_id.clone(),
                    title: names::media_title(&show.title),
                    subtitle: Some(episode_subtitle(episode)),
                    artwork_url: episode.still_url.clone().or_else(|| series_artwork(&details, &show)),
                    progress: 0.0,
                    remaining_seconds: episode.duration_seconds.map(|seconds| seconds as f64),
                    up_next: true,
                    reason: Some(up_next_reason(reason, episode.air_date, now)),
                    updated_at: date,
                },
            ));
        }
        Ok(items)
    }
}

/// What a next episode's card says, as Xorvio for Mac chose: "New Episode"
/// for one that aired in the last two weeks (unless the series starts from
/// the beginning, where the first episode is never new), and otherwise why
/// it's offered.
pub(crate) fn up_next_reason(reason: Reason, air_date: Option<Timestamp>, now: Timestamp) -> UpNextReason {
    let recent = air_date.is_some_and(|aired| aired <= now && now - aired < NEW_EPISODE_SECONDS);
    match reason {
        Reason::Next | Reason::Latest if recent => UpNextReason::NewEpisode,
        Reason::Next => UpNextReason::Next,
        Reason::Latest => UpNextReason::Latest,
        Reason::Start => UpNextReason::Start,
    }
}

/// The label for the episode after one finished, for add-on titles, which
/// Continue Watching works out apart from the catalog's.
pub fn next_episode_reason(air_date: Option<Timestamp>, now: Timestamp) -> UpNextReason {
    up_next_reason(Reason::Next, air_date, now)
}

/// "S01 E03 · What the Nets Bring", or just the code when the title only
/// repeats it.
pub(crate) fn episode_subtitle(episode: &Episode) -> String {
    let code = names::episode_code(episode.season, episode.number);
    let title = episode.title.trim();
    if title.is_empty() || names::episode_info(title).is_some() || title == format!("Episode {}", episode.number) {
        code
    } else {
        format!("{code} · {title}")
    }
}

fn backdrop(details: &HashMap<String, DetailsRecord>, id: &str) -> Option<String> {
    details.get(id).and_then(|record| record.backdrop_urls.first().cloned())
}

/// Landscape artwork for a series: a backdrop, or the poster as a last resort.
fn series_artwork(details: &HashMap<String, DetailsRecord>, show: &Series) -> Option<String> {
    backdrop(details, &show.id).or_else(|| show.backdrop_url.clone()).or_else(|| show.poster_url.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Timestamp = 1_800_000_000;
    const DAY: i64 = 86_400;

    #[test]
    fn cards_say_new_episode_for_two_weeks_after_it_airs() {
        assert_eq!(up_next_reason(Reason::Next, Some(NOW - 3 * DAY), NOW), UpNextReason::NewEpisode);
        assert_eq!(up_next_reason(Reason::Latest, Some(NOW - 13 * DAY), NOW), UpNextReason::NewEpisode);
        assert_eq!(up_next_reason(Reason::Next, Some(NOW - 15 * DAY), NOW), UpNextReason::Next, "older: just the next one");
        assert_eq!(up_next_reason(Reason::Latest, Some(NOW - 40 * DAY), NOW), UpNextReason::Latest);
        assert_eq!(up_next_reason(Reason::Next, None, NOW), UpNextReason::Next, "no air date: nothing to say it's new");
        assert_eq!(up_next_reason(Reason::Start, Some(NOW - DAY), NOW), UpNextReason::Start, "a series started from the beginning");
        assert_eq!(up_next_reason(Reason::Next, Some(NOW + DAY), NOW), UpNextReason::Next, "a date still to come isn't new yet");
        assert_eq!(next_episode_reason(Some(NOW - DAY), NOW), UpNextReason::NewEpisode);
    }
}
