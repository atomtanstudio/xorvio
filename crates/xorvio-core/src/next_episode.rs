//! The episode of a series to watch next, for Continue Watching.

use std::collections::HashSet;

use xorvio_model::{Episode, Timestamp};

/// Why an episode is offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The one after the furthest watched.
    Next,
    /// Nothing watched yet and the series is airing: its newest episode.
    Latest,
    /// Nothing watched yet and the series isn't airing: the first episode.
    Start,
}

/// How long after its newest episode a series still counts as airing.
pub const AIRING_WINDOW_SECONDS: i64 = 60 * 86_400;

/// The episode after the furthest one watched, once it has aired; `None` when
/// someone is caught up. With nothing watched, the newest episode while the
/// series is airing, and otherwise the first. Specials (season 0) are left out
/// unless they're all there is. An episode without an air date counts as aired.
pub fn find<'a>(episodes: &'a [Episode], finished: &HashSet<String>, now: Timestamp) -> Option<(&'a Episode, Reason)> {
    let regular: Vec<&Episode> = episodes.iter().filter(|episode| episode.season > 0).collect();
    let mut ordered: Vec<&Episode> = if regular.is_empty() { episodes.iter().collect() } else { regular };
    ordered.sort_by_key(|episode| (episode.season, episode.number));
    let aired = |episode: &Episode| episode.air_date.is_none_or(|date| date <= now);

    if let Some(furthest) = ordered.iter().rposition(|episode| finished.contains(&episode.id)) {
        let following = *ordered.get(furthest + 1)?;
        return aired(following).then_some((following, Reason::Next));
    }
    let latest = *ordered.iter().rev().find(|episode| aired(episode))?;
    if let Some(air_date) = latest.air_date
        && now - air_date <= AIRING_WINDOW_SECONDS
    {
        return Some((latest, Reason::Latest));
    }
    ordered.first().map(|first| (*first, Reason::Start))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xorvio_model::StreamLocator;

    const NOW: Timestamp = 1_800_000_000;

    /// (season, number, days ago it aired; negative is still to come, `None` unknown)
    fn episodes(list: &[(i64, i64, Option<i64>)]) -> Vec<Episode> {
        list.iter()
            .map(|&(season, number, days_ago)| Episode {
                id: format!("{season}x{number}"),
                series_id: "series".into(),
                source_id: "source".into(),
                season,
                number,
                title: format!("E{number}"),
                plot: None,
                duration_seconds: None,
                still_url: None,
                stream: StreamLocator::Url { url: format!("https://example.com/{season}/{number}") },
                air_date: days_ago.map(|days| NOW - days * 86_400),
            })
            .collect()
    }

    fn finished(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    fn found(result: Option<(&Episode, Reason)>) -> Option<(String, Reason)> {
        result.map(|(episode, reason)| (episode.id.clone(), reason))
    }

    #[test]
    fn follows_the_furthest_watched() {
        let list = episodes(&[(1, 1, Some(400)), (1, 2, Some(393)), (2, 1, Some(30)), (2, 2, Some(23))]);
        assert_eq!(found(find(&list, &finished(&["1x1", "1x2"]), NOW)), Some(("2x1".into(), Reason::Next)));
        // Watching out of order still carries on after the furthest.
        assert_eq!(found(find(&list, &finished(&["2x1"]), NOW)), Some(("2x2".into(), Reason::Next)));
    }

    #[test]
    fn caught_up_waits_for_the_next_to_air() {
        let list = episodes(&[(1, 1, Some(14)), (1, 2, Some(7)), (1, 3, Some(-7))]);
        assert_eq!(find(&list, &finished(&["1x1", "1x2"]), NOW), None);
        let later = NOW + 8 * 86_400;
        assert_eq!(found(find(&list, &finished(&["1x1", "1x2"]), later)).map(|f| f.0), Some("1x3".into()));
    }

    #[test]
    fn nothing_watched_offers_the_newest_while_airing() {
        let list = episodes(&[(0, 1, Some(500)), (1, 1, Some(400)), (2, 1, Some(20)), (2, 2, Some(13)), (2, 3, Some(-1))]);
        assert_eq!(found(find(&list, &finished(&[]), NOW)), Some(("2x2".into(), Reason::Latest)));
    }

    #[test]
    fn nothing_watched_of_a_finished_series_starts_at_the_beginning() {
        // Specials come first in a season list but aren't where a series starts.
        let ended = episodes(&[(0, 1, Some(900)), (1, 1, Some(800)), (1, 2, Some(793))]);
        assert_eq!(found(find(&ended, &finished(&[]), NOW)), Some(("1x1".into(), Reason::Start)));
        // Without air dates there's no telling it's airing, so it starts at the beginning too.
        let undated = episodes(&[(1, 1, None), (1, 2, None)]);
        assert_eq!(found(find(&undated, &finished(&[]), NOW)), Some(("1x1".into(), Reason::Start)));
    }

    #[test]
    fn all_watched_has_nothing_next() {
        let list = episodes(&[(1, 1, Some(30)), (1, 2, None)]);
        assert_eq!(find(&list, &finished(&["1x1", "1x2"]), NOW), None);
        assert_eq!(find(&[], &finished(&[]), NOW), None);
    }
}
