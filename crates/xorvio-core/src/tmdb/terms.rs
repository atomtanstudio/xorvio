//! What search finds films and series by, from TMDB, for titles whose
//! source names no one in them: most IPTV films, and playlists' and media
//! folders' videos. The app asks for a batch at a time in the background
//! (`Core::fill_search_terms`), the most recently added titles first. Each
//! is matched as its page would be (its TMDB id, else its IMDb id, else its
//! name and year, which is all that's sent) and then asked about once, with
//! its credits, in a reply much smaller than its page's. Copies of a title
//! share the answer, and a title whose page was opened needs no request at
//! all. One TMDB has nothing for is asked about again after a while.
//! Settings › Details can switch this off, and switching TMDB off stops it.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::api::{Client, Key, Kind};
use super::{RETRY_MISSES_AFTER, TitleLookup, TmdbTitle};
use crate::error::SourceError;
use crate::store::catalog::{SearchKind, TitleWantingTerms, title_key};
use crate::terms::SearchTerms;
use crate::{Core, Result, now};

/// Titles taken from the catalog at a time.
const BATCH: usize = 200;
/// A pause between titles, so looking a library up never crowds out what's
/// being opened.
const BETWEEN: Duration = Duration::from_millis(150);
/// Titles TMDB couldn't be asked about in a row before a batch stops: one
/// it always fails on is passed over, but TMDB being down stops them all.
const MOST_FAILURES: usize = 3;

impl Core {
    /// Settings › Details: whether every title is looked up for search.
    pub fn set_search_lookups(&self, on: bool) {
        self.tmdb.search_lookups.store(on, Ordering::Relaxed);
    }

    /// How many films and series are still to be looked up for search, or
    /// `None` when they aren't being looked up (switched off, or no key).
    pub fn search_lookups_left(&self) -> Result<Option<usize>> {
        if !self.tmdb.search_lookups.load(Ordering::Relaxed) || self.tmdb.key().is_none() {
            return Ok(None);
        }
        let (sources, folders) = self.term_sources();
        Ok(Some(self.catalog.count_titles_wanting_terms(&sources, &folders, now() - RETRY_MISSES_AFTER)?))
    }

    /// Looks a batch of titles up on TMDB for search, returning how many it
    /// settled: none when there's nothing to look up, it's switched off, or
    /// TMDB can't be asked just now.
    pub async fn fill_search_terms(&self) -> Result<usize> {
        if !self.tmdb.search_lookups.load(Ordering::Relaxed) || self.tmdb.key().is_none() {
            return Ok(0);
        }
        let Ok(_batch) = self.tmdb.filling_terms.try_lock() else { return Ok(0) };
        let (sources, folders) = self.term_sources();
        let wanted = self.catalog.titles_wanting_terms(&sources, &folders, now() - RETRY_MISSES_AFTER, BATCH)?;
        let (mut settled, mut failures) = (0, 0);
        for Copies { kind, lookup, copies } in copies_together(wanted) {
            // Switched off, or the key turned down, meanwhile.
            if !self.tmdb.search_lookups.load(Ordering::Relaxed) {
                break;
            }
            let Some(key) = self.tmdb.key() else { break };
            // TMDB couldn't be asked: the title waits for another time.
            let Some(found) = self.tmdb_search_terms(&key, &lookup).await else {
                failures += 1;
                if failures == MOST_FAILURES {
                    break;
                }
                continue;
            };
            failures = 0;
            self.catalog.save_tmdb_terms(&copies, kind, found.as_ref().map(|(id, terms)| (*id, terms)), now())?;
            settled += copies.len();
            tokio::time::sleep(BETWEEN).await;
        }
        Ok(settled)
    }

    /// The sources whose titles are looked up, and which of them are media folders.
    fn term_sources(&self) -> (Vec<String>, Vec<String>) {
        let sources = self.tmdb_sources();
        let folders = sources.iter().filter(|id| self.is_folder_source(id)).cloned().collect();
        (sources, folders)
    }

    /// TMDB's terms for a title, with its id there: `Some(None)` when TMDB
    /// has no such title, `None` when it couldn't be asked.
    async fn tmdb_search_terms(&self, key: &Key, lookup: &TitleLookup) -> Option<Option<(i64, SearchTerms)>> {
        let Some(id) = self.tmdb_id_lookup(key, lookup).await? else { return Some(None) };
        let kind = lookup.kind.path();
        // A copy looked up before, or the title's page opened before.
        if let Ok(Some(terms)) = self.catalog.tmdb_terms_of(search_kind(lookup.kind), id) {
            return Some(Some((id, terms)));
        }
        if let Ok(Some((text, _))) = self.catalog.tmdb_cached(kind, id)
            && let Ok(title) = serde_json::from_str::<TmdbTitle>(&text)
            && !title.is_empty()
        {
            return Some(Some((id, SearchTerms::from_details(&title.details))));
        }
        let what = format!("terms:{kind}/{id}");
        if self.tmdb.failed_lately(&what) {
            return None;
        }
        let base = self.tmdb.base();
        let client = Client { http: &self.http, base: &base, key };
        let asked = {
            let _turn = self.tmdb.turns.wait().await;
            client.search_details(lookup.kind, id).await
        };
        match asked {
            Ok(details) => Some(Some((id, SearchTerms::from_details(&details)))),
            Err(SourceError::Http(404)) => Some(None),
            Err(error) => {
                self.tmdb.note_failure(&what, &error);
                None
            }
        }
    }
}

fn search_kind(kind: Kind) -> SearchKind {
    match kind {
        Kind::Movie => SearchKind::Movie,
        Kind::Tv => SearchKind::Series,
    }
}

/// Copies of one title: how it's looked up, and each copy as (id, source id).
struct Copies {
    kind: SearchKind,
    lookup: TitleLookup,
    copies: Vec<(String, String)>,
}

/// The titles to look up, copies of one (by TMDB id, else IMDb id, else
/// name and year) together, in the order they came.
fn copies_together(wanted: Vec<TitleWantingTerms>) -> Vec<Copies> {
    let mut groups: Vec<Copies> = Vec::new();
    let mut by_identity: HashMap<String, usize> = HashMap::new();
    for title in wanted {
        let kind = if title.kind == SearchKind::Series { Kind::Tv } else { Kind::Movie };
        let some = |id: &Option<String>| id.as_deref().map(str::trim).filter(|id| !id.is_empty()).map(str::to_string);
        let identity = match (some(&title.tmdb_id), some(&title.imdb_id)) {
            (Some(id), _) => format!("{}|tmdb|{id}", kind.path()),
            (None, Some(id)) => format!("{}|imdb|{id}", kind.path()),
            (None, None) => format!("{}|{}|{}", kind.path(), title_key(&title.title), title.year.unwrap_or(0)),
        };
        let copy = (title.item_id.clone(), title.source_id.clone());
        match by_identity.get(&identity) {
            Some(&at) => groups[at].copies.push(copy),
            None => {
                by_identity.insert(identity, groups.len());
                let lookup = TitleLookup {
                    kind,
                    item_id: title.item_id,
                    source_id: title.source_id,
                    tmdb_id: title.tmdb_id,
                    imdb_id: title.imdb_id,
                    title: title.title,
                    year: title.year,
                };
                groups.push(Copies { kind: title.kind, lookup, copies: vec![copy] });
            }
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_of_a_title_are_looked_up_once() {
        let title = |id: &str, kind, title: &str, year, tmdb_id: Option<&str>| TitleWantingTerms {
            kind,
            item_id: id.into(),
            source_id: "s".into(),
            title: title.into(),
            year,
            tmdb_id: tmdb_id.map(str::to_string),
            imdb_id: None,
            added_at: None,
        };
        let groups = copies_together(vec![
            title("a", SearchKind::Movie, "EN - Paper Boats 4K", Some(2024), None),
            title("b", SearchKind::Movie, "The Matrix", Some(1999), Some("603")),
            title("c", SearchKind::Movie, "Paper Boats", Some(2024), None),
            title("d", SearchKind::Series, "Paper Boats", Some(2024), None),
            title("e", SearchKind::Movie, "Matrix", None, Some("603")),
            title("f", SearchKind::Movie, "Paper Boats", Some(1960), None),
        ]);
        let ids: Vec<Vec<&str>> = groups.iter().map(|group| group.copies.iter().map(|(id, _)| id.as_str()).collect()).collect();
        assert_eq!(ids, [vec!["a", "c"], vec!["b", "e"], vec!["d"], vec!["f"]]);
        assert_eq!(groups[2].lookup.kind, Kind::Tv);
    }
}
