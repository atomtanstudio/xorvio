//! Posters for lists from TMDB, for films and series whose source has none
//! (common with media folders, and some providers' titles). They're looked
//! for only for titles a list is showing, a batch at a time and one request
//! after another, and kept by catalog id, so lists show them at once from
//! then on (see `Core::present_movies`). TMDB's own reply for a title
//! already kept for its page is used when it has the poster. A media
//! folder's video with no year and no ids isn't a film TMDB would know (a
//! home video, a clip, a music video): it's never looked for, and a frame of
//! it is its picture instead (`Core::videos_without_pictures`).

use xorvio_model::DEMO_SOURCE_ID;

use super::api::{Client, Key};
use super::{RETRY_MISSES_AFTER, TitleLookup, TmdbTitle};
use crate::error::SourceError;
use crate::{Core, Result, now};

/// The most titles looked for in one call; a list asks again for more.
const MOST_AT_ONCE: usize = 24;

impl Core {
    /// TMDB's posters for these films or series, those that have none of
    /// their own, as (id, poster) for those found. A title looked for
    /// before isn't asked about again for a while, whatever was found; the
    /// demo library never is. Without a key nothing is asked.
    pub async fn fill_posters(&self, ids: &[String]) -> Result<Vec<(String, String)>> {
        if self.tmdb.key().is_none() || ids.is_empty() {
            return Ok(Vec::new());
        }
        // One batch at a time, so lists scrolled quickly never pile requests up.
        let _batch = self.tmdb.filling_posters.lock().await;
        let ids: Vec<String> = ids.iter().take(MOST_AT_ONCE).cloned().collect();
        let lookups: Vec<TitleLookup> = self
            .catalog
            .movies_by_ids(&ids)?
            .iter()
            .filter(|movie| movie.poster_url.is_none())
            .map(TitleLookup::of_movie)
            .chain(self.catalog.series_by_ids(&ids)?.iter().filter(|series| series.poster_url.is_none()).map(TitleLookup::of_series))
            .filter(|lookup| lookup.source_id != DEMO_SOURCE_ID)
            .filter(|lookup| lookup.year.is_some() || lookup.tmdb_id.is_some() || lookup.imdb_id.is_some() || !self.is_folder_source(&lookup.source_id))
            .collect();
        let mut found = Vec::new();
        for lookup in lookups {
            match self.catalog.poster_lookup(&lookup.item_id)? {
                Some((Some(poster), _)) => {
                    found.push((lookup.item_id.clone(), poster));
                    continue;
                }
                Some((None, at)) if now() - at < RETRY_MISSES_AFTER => continue,
                _ => {}
            }
            // A key turned down meanwhile stops the batch.
            let Some(key) = self.tmdb.key() else { break };
            // TMDB couldn't be asked: the rest wait for another time too.
            let Some(poster) = self.tmdb_poster(&key, &lookup).await else { break };
            self.catalog.save_poster_lookup(&lookup.item_id, &lookup.source_id, poster.as_deref(), now())?;
            if let Some(poster) = poster {
                found.push((lookup.item_id.clone(), poster));
            }
        }
        Ok(found)
    }

    /// The title's poster on TMDB, `Some(None)` when TMDB has none (or no
    /// such title), `None` when TMDB couldn't be asked.
    async fn tmdb_poster(&self, key: &Key, lookup: &TitleLookup) -> Option<Option<String>> {
        let Some(id) = self.tmdb_id_lookup(key, lookup).await? else { return Some(None) };
        let kind = lookup.kind.path();
        // A title opened before has TMDB's whole reply kept, poster and all.
        if let Ok(Some((text, _))) = self.catalog.tmdb_cached(kind, id)
            && let Ok(title) = serde_json::from_str::<TmdbTitle>(&text)
            && let Some(poster) = title.poster_url
        {
            return Some(Some(poster));
        }
        let what = format!("poster:{kind}/{id}");
        if self.tmdb.failed_lately(&what) {
            return None;
        }
        let base = self.tmdb.base();
        let client = Client { http: &self.http, base: &base, key };
        let asked = {
            let _turn = self.tmdb.turns.wait().await;
            client.poster(lookup.kind, id).await
        };
        match asked {
            Ok(poster) => Some(poster),
            Err(SourceError::Http(404)) => Some(None),
            Err(error) => {
                self.tmdb.note_failure(&what, &error);
                None
            }
        }
    }
}
