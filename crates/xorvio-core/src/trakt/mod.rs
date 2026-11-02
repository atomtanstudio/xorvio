//! Trakt: connecting an account (a device code entered at trakt.tv/activate),
//! telling Trakt what plays as it plays (scrobbling), and keeping what's been
//! watched, places left part-way and the watchlist the same both ways.
//!
//! Going out, changes people make here (marking watched or unwatched, taking
//! a card off Continue Watching, saving to the watchlist) are sent in the
//! background. Coming in, a sync (sync.rs) keeps what Trakt says in the
//! library's Trakt tables and applies it to Xorvio's own progress and
//! watchlist directly, so nothing taken in is ever sent back. Nothing here
//! deletes Xorvio's own history because of Trakt.

mod api;
mod ids;
mod lists;
mod sync;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;

use serde_json::{Value, json};
use xorvio_model::{
    DEMO_SOURCE_ID, MediaKind, PlaybackProgress, TraktOptions, TraktScrobbleAction, TraktSignInStart, TraktStatus,
    WatchlistEntry,
};

pub use api::{TraktEvent, TraktListener};
pub use lists::AddonReach;
pub(crate) use api::{APP_KEY, CONNECTION_KEY, Trakt};
pub(crate) use ids::TraktIds;

use api::TraktError;
use ids::{Subject, Title};

use crate::sources::ids as item_ids;
use crate::{Core, Result, json as loose, redact};

impl Core {
    /// Hears about Trakt's status changing and syncs changing the library,
    /// for the interface's `trakt-changed` and `library-changed` events.
    pub fn set_trakt_listener(&self, listener: TraktListener) {
        self.trakt.set_listener(listener);
    }

    pub fn trakt_status(&self) -> TraktStatus {
        self.trakt.status()
    }

    /// Starts connecting: a code to enter at Trakt's verification address.
    /// The device code it stands for stays here.
    pub async fn trakt_start_sign_in(&self) -> Result<TraktSignInStart> {
        Ok(self.trakt.start_sign_in().await?)
    }

    /// True once the code has been entered and Xorvio allowed (the caller
    /// then starts the first sync); false while waiting. Fails when the code
    /// expired or was refused.
    pub async fn trakt_check_sign_in(&self) -> Result<bool> {
        self.trakt.check_sign_in().await
    }

    /// Revokes the connection and forgets it, with what Trakt said about the
    /// account. Xorvio's own progress and watchlist stay as they are.
    pub async fn trakt_disconnect(&self) -> Result<()> {
        self.trakt.disconnect().await
    }

    pub fn trakt_set_options(&self, options: TraktOptions) -> Result<()> {
        self.trakt.set_options(options)
    }

    /// Uses this Trakt app instead of the built-in one (kept in the
    /// keychain); empty strings go back to the built-in one. The redirect
    /// address is the one the app was registered with; empty is the default.
    pub async fn trakt_set_app(&self, client_id: &str, redirect_uri: &str) -> Result<()> {
        self.trakt.set_app(client_id, redirect_uri).await
    }

    #[cfg(test)]
    pub(crate) fn use_trakt_test_server(&self, base: &str, built_in: Option<&str>, write_spacing: std::time::Duration) {
        self.trakt.use_test_server(base, built_in, write_spacing);
    }

    /// Tells Trakt what's playing: a film or episode starting, pausing or
    /// stopping at `progress` percent. Trakt marks it watched itself when it
    /// stops at 80% or more. Does nothing when Trakt isn't connected,
    /// scrobbling is off or the title isn't one Trakt knows, and never fails:
    /// a problem is only logged.
    pub async fn trakt_scrobble(&self, item_id: &str, kind: MediaKind, action: TraktScrobbleAction, progress: f64) {
        if !matches!(kind, MediaKind::Movie | MediaKind::Episode) || !self.trakt.options().scrobble || !self.trakt.is_connected() {
            return;
        }
        let Some(subject) = self.trakt_subject(item_id, kind) else {
            tracing::debug!("nothing to scrobble: the title has nothing Trakt knows it by");
            return;
        };
        let trakt = &self.trakt;
        let progress = if progress.is_finite() { progress.clamp(0.0, 100.0) } else { 0.0 };
        let body = match &subject {
            Subject::Movie(title) => {
                let Some(ids) = trakt.resolve(title).await else { return };
                json!({"movie": {"ids": ids.to_json()}, "progress": progress})
            }
            Subject::Episode { show, season, number } => {
                let Some(show_ids) = trakt.resolve(show).await else { return };
                match trakt.episode_ids(&show_ids, *season, *number).await {
                    Ok(ids) => json!({"episode": {"ids": ids.to_json()}, "progress": progress}),
                    Err(error) => {
                        tracing::debug!("the episode playing wasn't found on Trakt: {error}");
                        return;
                    }
                }
            }
        };
        match trakt.post(&format!("/scrobble/{}", action.as_str()), &body).await {
            // 409: stopped moments ago already, which Trakt counted.
            Ok(_) | Err(TraktError::Http(409)) => {}
            // Below 1% Trakt ignores a stop (422); a title it doesn't have is 404.
            Err(error @ (TraktError::Http(422) | TraktError::NotFound)) => tracing::debug!("Trakt didn't take a scrobble: {error}"),
            Err(error) => {
                tracing::warn!("Trakt couldn't be told what's playing: {}", redact::redact(&error.to_string(), &trakt.hidden()));
            }
        }
    }

    // MARK: Changes made here

    /// Whether history goes both ways with Trakt right now.
    pub(crate) fn trakt_history_on(&self) -> bool {
        self.trakt.options().sync_history && self.trakt.is_connected()
    }

    fn trakt_watchlist_on(&self) -> bool {
        self.trakt.options().sync_watchlist && self.trakt.is_connected()
    }

    /// Marking watched or unwatched by hand, sent to Trakt's history in the
    /// background. (Playback reaching the end isn't sent: scrobbling is how
    /// Trakt hears of that.) Unwatched also forgets Trakt's place in it.
    pub(crate) fn tell_trakt_watched(&self, items: &[PlaybackProgress], watched: bool) {
        if !self.trakt_history_on() {
            return;
        }
        let subjects: Vec<Subject> = items.iter().filter_map(|item| self.trakt_subject(&item.item_id, item.kind)).collect();
        if subjects.is_empty() {
            return;
        }
        if !watched {
            // Or the next sync would bring them back before Trakt has heard.
            self.forget_trakt_rows(&subjects, true);
        }
        let trakt = self.trakt.clone();
        let at = watched.then(|| sync::iso_time(crate::now()));
        spawn_trakt("history", trakt.clone(), async move {
            let named = name_all(&trakt, &subjects).await;
            if named.is_empty() {
                return Ok(());
            }
            let body = history_body(&named, at.as_deref());
            trakt.post(if watched { "/sync/history" } else { "/sync/history/remove" }, &body).await?;
            if !watched {
                forget_places_on_trakt(&trakt, &named).await?;
            }
            Ok(())
        });
    }

    /// A card taken off Continue Watching: Trakt forgets its place in that
    /// title too, if it has one, in the background.
    pub(crate) fn tell_trakt_place_forgotten(&self, item_id: &str, kind: MediaKind) {
        if !self.trakt_history_on() {
            return;
        }
        let Some(subject) = self.trakt_subject(item_id, kind) else { return };
        let subjects = vec![subject];
        self.forget_trakt_rows(&subjects, false);
        let trakt = self.trakt.clone();
        spawn_trakt("places", trakt.clone(), async move {
            let named = name_all(&trakt, &subjects).await;
            if named.is_empty() { Ok(()) } else { forget_places_on_trakt(&trakt, &named).await }
        });
    }

    /// A film or series saved to or taken off the watchlist here, sent to
    /// Trakt's watchlist in the background.
    pub(crate) fn tell_trakt_watchlist(&self, entry: &WatchlistEntry, saved: bool) {
        if !matches!(entry.kind, MediaKind::Movie | MediaKind::Series) || !self.trakt_watchlist_on() {
            return;
        }
        let Some(title) = self.trakt_title_of(entry) else { return };
        let trakt = self.trakt.clone();
        spawn_trakt("watchlist", trakt.clone(), async move {
            let Some(ids) = trakt.resolve(&title).await else { return Ok(()) };
            let body = sync::list_body(&[(title.show, ids)]);
            trakt.post(if saved { "/sync/watchlist" } else { "/sync/watchlist/remove" }, &body).await.map(|_| ())
        });
    }

    /// Forgets what the Trakt tables say about these titles' places (and,
    /// with `watched`, that they were watched), so the next sync doesn't put
    /// back what was just undone here.
    fn forget_trakt_rows(&self, subjects: &[Subject], watched: bool) {
        let library = &self.trakt.library;
        let result = (|| -> Result<()> {
            let paused = library.trakt_paused()?;
            let (films, episodes) =
                if watched { (library.trakt_watched(false)?, library.trakt_watched(true)?) } else { (Vec::new(), Vec::new()) };
            for subject in subjects {
                let (title, season, number) = match subject {
                    Subject::Movie(title) => (title, 0, 0),
                    Subject::Episode { show, season, number } => (show, *season, *number),
                };
                let title = self.known_title(title);
                for place in paused.iter().filter(|place| place.title.is(&title) && (place.season, place.number) == (season, number)) {
                    library.forget_trakt_paused(place.playback_id)?;
                }
                let rows = if title.show { &episodes } else { &films };
                for row in rows.iter().filter(|row| row.title.is(&title) && (row.season, row.number) == (season, number)) {
                    if let Some(trakt_id) = row.title.ids.trakt {
                        library.forget_trakt_watched(title.show, trakt_id, season, number)?;
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!("what Trakt said about a title couldn't be cleared: {error}");
        }
    }

    /// A title with the ids an earlier lookup found, when it has none of its own.
    fn known_title(&self, title: &Title) -> Title {
        if !title.ids.is_empty() {
            return title.clone();
        }
        match self.trakt.library.trakt_lookup(title.show, &title.key(), title.year) {
            Ok(Some(Some(ids))) => Title { ids, ..title.clone() },
            _ => title.clone(),
        }
    }

    // MARK: Naming titles to Trakt

    /// What a film or episode is on Trakt, from the catalog or an add-on id.
    fn trakt_subject(&self, item_id: &str, kind: MediaKind) -> Option<Subject> {
        if item_id.starts_with("addon|") {
            return ids::addon_subject(item_id);
        }
        if item_ids::source_of(item_id) == DEMO_SOURCE_ID {
            return None;
        }
        match kind {
            MediaKind::Movie => {
                let movie = self.catalog.movies_by_ids(&[item_id.to_string()]).ok()?.pop()?;
                Some(Subject::Movie(Title::of_movie(&movie)))
            }
            MediaKind::Episode => {
                let episode = self.catalog.episodes_by_ids(&[item_id.to_string()]).ok()?.pop()?;
                let series = self.catalog.series_by_ids(std::slice::from_ref(&episode.series_id)).ok()?.pop()?;
                Some(Subject::Episode { show: Title::of_series(&series), season: episode.season, number: episode.number })
            }
            _ => None,
        }
    }

    /// What a watchlist film or series is on Trakt. One whose source has
    /// gone is named by the entry's own title and year.
    fn trakt_title_of(&self, entry: &WatchlistEntry) -> Option<Title> {
        if entry.id.starts_with("addon|") {
            return ids::addon_title(&entry.id);
        }
        if entry.source_id == DEMO_SOURCE_ID {
            return None;
        }
        let id = [entry.id.clone()];
        let found = match entry.kind {
            MediaKind::Movie => self.catalog.movies_by_ids(&id).ok()?.pop().map(|movie| Title::of_movie(&movie)),
            MediaKind::Series => self.catalog.series_by_ids(&id).ok()?.pop().map(|series| Title::of_series(&series)),
            _ => return None,
        };
        found.or_else(|| {
            Some(Title {
                show: entry.kind == MediaKind::Series,
                ids: TraktIds::default(),
                name: crate::names::media_title(&entry.title),
                year: entry.subtitle.as_deref().and_then(|year| year.trim().parse().ok()),
            })
        })
    }
}

/// A film or episode as Trakt's ids name it.
#[derive(Debug, Clone, PartialEq)]
enum Named {
    Movie(TraktIds),
    Episode { show: TraktIds, season: i64, number: i64 },
}

/// Each subject's Trakt ids, looking up those without; ones Trakt doesn't
/// know are left out.
async fn name_all(trakt: &Trakt, subjects: &[Subject]) -> Vec<Named> {
    let mut named = Vec::new();
    for subject in subjects {
        let found = match subject {
            Subject::Movie(title) => trakt.resolve(title).await.map(Named::Movie),
            Subject::Episode { show, season, number } => {
                trakt.resolve(show).await.map(|show| Named::Episode { show, season: *season, number: *number })
            }
        };
        named.extend(found);
    }
    named
}

/// `/sync/history` (or its remove): films by their ids, episodes under their
/// show by season and number.
fn history_body(named: &[Named], watched_at: Option<&str>) -> Value {
    let with_time = |mut item: Value| {
        if let (Some(at), Some(object)) = (watched_at, item.as_object_mut()) {
            object.insert("watched_at".into(), json!(at));
        }
        item
    };
    let mut movies = Vec::new();
    let mut shows: Vec<(TraktIds, BTreeMap<i64, Vec<i64>>)> = Vec::new();
    for item in named {
        match item {
            Named::Movie(ids) => movies.push(with_time(json!({"ids": ids.to_json()}))),
            Named::Episode { show, season, number } => {
                let index = match shows.iter().position(|(ids, _)| ids == show) {
                    Some(index) => index,
                    None => {
                        shows.push((show.clone(), BTreeMap::new()));
                        shows.len() - 1
                    }
                };
                shows[index].1.entry(*season).or_default().push(*number);
            }
        }
    }
    let shows: Vec<Value> = shows
        .into_iter()
        .map(|(ids, seasons)| {
            let seasons: Vec<Value> = seasons
                .into_iter()
                .map(|(season, numbers)| {
                    let episodes: Vec<Value> = numbers.into_iter().map(|number| with_time(json!({"number": number}))).collect();
                    json!({"number": season, "episodes": episodes})
                })
                .collect();
            json!({"ids": ids.to_json(), "seasons": seasons})
        })
        .collect();
    json!({"movies": movies, "shows": shows})
}

/// Removes Trakt's places in these titles, looking them up first: a place
/// can be newer than the last sync (scrobbling makes them).
async fn forget_places_on_trakt(trakt: &Trakt, named: &[Named]) -> std::result::Result<(), TraktError> {
    let places = trakt.get("/sync/playback").await?;
    for place in places.as_array().into_iter().flatten() {
        let Some(id) = loose::int(place.get("id")) else { continue };
        if !named.iter().any(|item| is_place_of(place, item)) {
            continue;
        }
        match trakt.delete(&format!("/sync/playback/{id}")).await {
            Ok(()) | Err(TraktError::NotFound) => {}
            Err(error) => return Err(error),
        }
        if let Err(error) = trakt.library.forget_trakt_paused(id) {
            tracing::warn!("a Trakt place couldn't be cleared: {error}");
        }
    }
    Ok(())
}

/// Whether a `/sync/playback` item is a place in this film or episode.
fn is_place_of(place: &Value, item: &Named) -> bool {
    let kind = place.get("type").and_then(Value::as_str);
    match item {
        Named::Movie(ids) => kind == Some("movie") && TraktIds::from_json(place.get("movie").and_then(|movie| movie.get("ids"))).shares_id(ids),
        Named::Episode { show, season, number } => {
            let episode = place.get("episode");
            kind == Some("episode")
                && TraktIds::from_json(place.get("show").and_then(|show| show.get("ids"))).shares_id(show)
                && loose::int(episode.and_then(|episode| episode.get("season"))) == Some(*season)
                && loose::int(episode.and_then(|episode| episode.get("number"))) == Some(*number)
        }
    }
}

/// Runs a change for Trakt off to the side: what someone did here has
/// happened whatever Trakt says, so a failure is only logged (without tokens).
fn spawn_trakt(
    what: &'static str,
    trakt: Arc<Trakt>,
    work: impl Future<Output = std::result::Result<(), TraktError>> + Send + 'static,
) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    runtime.spawn(async move {
        if let Err(error) = work.await {
            tracing::warn!("Trakt's {what} couldn't be updated: {}", redact::redact(&error.to_string(), &trakt.hidden()));
        }
    });
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn history_groups_episodes_under_their_show() {
        let show = TraktIds { imdb: Some("tt0000002".into()), ..TraktIds::default() };
        let named = [
            Named::Movie(TraktIds { tmdb: Some(5), ..TraktIds::default() }),
            Named::Episode { show: show.clone(), season: 1, number: 2 },
            Named::Episode { show: show.clone(), season: 1, number: 1 },
            Named::Episode { show, season: 2, number: 1 },
        ];
        let body = history_body(&named, Some("2026-10-30T12:00:00.000Z"));
        assert_eq!(
            body,
            json!({
                "movies": [{"ids": {"tmdb": 5}, "watched_at": "2026-10-30T12:00:00.000Z"}],
                "shows": [{"ids": {"imdb": "tt0000002"}, "seasons": [
                    {"number": 1, "episodes": [{"number": 2, "watched_at": "2026-10-30T12:00:00.000Z"}, {"number": 1, "watched_at": "2026-10-30T12:00:00.000Z"}]},
                    {"number": 2, "episodes": [{"number": 1, "watched_at": "2026-10-30T12:00:00.000Z"}]}
                ]}]
            })
        );
        let removal = history_body(&named[..1], None);
        assert_eq!(removal, json!({"movies": [{"ids": {"tmdb": 5}}], "shows": []}));
    }

    #[test]
    fn places_are_matched_to_their_title() {
        let place = json!({"id": 9, "type": "episode", "progress": 40.0, "episode": {"season": 1, "number": 2},
                           "show": {"ids": {"trakt": 3, "imdb": "tt0000002"}}});
        let show = TraktIds { imdb: Some("tt0000002".into()), ..TraktIds::default() };
        assert!(is_place_of(&place, &Named::Episode { show: show.clone(), season: 1, number: 2 }));
        assert!(!is_place_of(&place, &Named::Episode { show: show.clone(), season: 1, number: 3 }));
        assert!(!is_place_of(&place, &Named::Movie(show)));
    }
}
