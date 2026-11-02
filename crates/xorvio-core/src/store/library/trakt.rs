//! What Trakt says, kept in the library between syncs: films and episodes
//! watched there, places left part-way, the watchlist as both sides had it
//! after the last sync, and titles looked up by name. Xorvio's own progress
//! and watchlist stay in their own tables; these only feed them.

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use xorvio_model::Timestamp;

use super::LibraryStore;
use crate::trakt::TraktIds;

/// A film or show as Trakt names it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TraktTitle {
    /// A show (whose episodes these are) rather than a film.
    pub show: bool,
    /// Always has the Trakt id when it came from Trakt.
    pub ids: TraktIds,
    /// Trakt's title as a title key, with its year, for catalog titles that
    /// carry no ids.
    pub title_key: String,
    pub year: Option<i64>,
}

/// A film, or one episode of a show, watched on Trakt.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TraktWatched {
    pub title: TraktTitle,
    /// 0 for films.
    pub season: i64,
    pub number: i64,
    pub watched_at: Timestamp,
}

/// A place left part-way on Trakt.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TraktPaused {
    /// Trakt's own id for it, which removing it needs.
    pub playback_id: i64,
    pub title: TraktTitle,
    pub season: i64,
    pub number: i64,
    /// How far, in percent.
    pub progress: f64,
    /// Trakt's runtime, for titles whose length Xorvio doesn't know.
    pub runtime_seconds: Option<i64>,
    pub paused_at: Timestamp,
}

/// A watchlist title both sides had after the last sync, and the Xorvio
/// items it was.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TraktListed {
    pub title: TraktTitle,
    pub item_ids: Vec<String>,
}

fn kind_text(show: bool) -> &'static str {
    if show { "show" } else { "movie" }
}

const TITLE_COLUMNS: &str = "kind, trakt_id, imdb_id, tmdb_id, title_key, year";

/// The six title columns, starting at `at`.
fn title_at(row: &Row<'_>, at: usize) -> rusqlite::Result<TraktTitle> {
    Ok(TraktTitle {
        show: row.get::<_, String>(at)? == "show",
        ids: TraktIds { trakt: row.get(at + 1)?, imdb: row.get(at + 2)?, tmdb: row.get(at + 3)? },
        title_key: row.get(at + 4)?,
        year: row.get(at + 5)?,
    })
}

fn watched_row(row: &Row<'_>) -> rusqlite::Result<TraktWatched> {
    Ok(TraktWatched { title: title_at(row, 0)?, season: row.get(6)?, number: row.get(7)?, watched_at: row.get(8)? })
}

fn paused_row(row: &Row<'_>) -> rusqlite::Result<TraktPaused> {
    Ok(TraktPaused {
        playback_id: row.get(0)?,
        title: title_at(row, 1)?,
        season: row.get(7)?,
        number: row.get(8)?,
        progress: row.get(9)?,
        runtime_seconds: row.get(10)?,
        paused_at: row.get(11)?,
    })
}

impl LibraryStore {
    // MARK: Watched

    /// Replaces what Trakt says was watched, for films or for shows' episodes.
    pub fn replace_trakt_watched(&self, show: bool, rows: &[TraktWatched]) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("DELETE FROM trakt_watched WHERE kind = ?1", [kind_text(show)])?;
            {
                let mut insert = tx.prepare_cached(&format!(
                    "INSERT OR REPLACE INTO trakt_watched ({TITLE_COLUMNS}, season, number, watched_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
                ))?;
                for row in rows.iter().filter(|row| row.title.show == show && row.title.ids.trakt.is_some()) {
                    let title = &row.title;
                    insert.execute(params![
                        kind_text(show),
                        title.ids.trakt,
                        title.ids.imdb,
                        title.ids.tmdb,
                        title.title_key,
                        title.year,
                        row.season,
                        row.number,
                        row.watched_at
                    ])?;
                }
            }
            tx.commit()
        })
    }

    /// Every film, or every show's episode, watched on Trakt.
    pub fn trakt_watched(&self, show: bool) -> anyhow::Result<Vec<TraktWatched>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {TITLE_COLUMNS}, season, number, watched_at FROM trakt_watched WHERE kind = ?1"
            ))?;
            statement.query_map([kind_text(show)], watched_row)?.collect()
        })
    }

    /// The shows with episodes watched on Trakt, each once.
    pub fn trakt_watched_shows(&self) -> anyhow::Result<Vec<TraktTitle>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {TITLE_COLUMNS} FROM trakt_watched WHERE kind = 'show' GROUP BY trakt_id"
            ))?;
            statement.query_map([], |row| title_at(row, 0))?.collect()
        })
    }

    /// One show's episodes watched on Trakt.
    pub fn trakt_watched_of_show(&self, trakt_id: i64) -> anyhow::Result<Vec<TraktWatched>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {TITLE_COLUMNS}, season, number, watched_at FROM trakt_watched WHERE kind = 'show' AND trakt_id = ?1"
            ))?;
            statement.query_map([trakt_id], watched_row)?.collect()
        })
    }

    /// Forgets that Trakt said a film (season and number 0) or an episode was
    /// watched, after it was marked unwatched here, so the next sync doesn't
    /// mark it again before Trakt has heard.
    pub fn forget_trakt_watched(&self, show: bool, trakt_id: i64, season: i64, number: i64) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute(
                "DELETE FROM trakt_watched WHERE kind = ?1 AND trakt_id = ?2 AND season = ?3 AND number = ?4",
                params![kind_text(show), trakt_id, season, number],
            )?;
            Ok(())
        })
    }

    // MARK: Paused

    /// Replaces the places left part-way on Trakt.
    pub fn replace_trakt_paused(&self, rows: &[TraktPaused]) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("DELETE FROM trakt_playback", [])?;
            {
                let mut insert = tx.prepare_cached(&format!(
                    "INSERT OR REPLACE INTO trakt_playback (playback_id, {TITLE_COLUMNS}, season, number, progress, runtime, paused_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
                ))?;
                for row in rows.iter().filter(|row| row.title.ids.trakt.is_some()) {
                    let title = &row.title;
                    insert.execute(params![
                        row.playback_id,
                        kind_text(title.show),
                        title.ids.trakt,
                        title.ids.imdb,
                        title.ids.tmdb,
                        title.title_key,
                        title.year,
                        row.season,
                        row.number,
                        row.progress,
                        row.runtime_seconds,
                        row.paused_at
                    ])?;
                }
            }
            tx.commit()
        })
    }

    pub fn trakt_paused(&self) -> anyhow::Result<Vec<TraktPaused>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT playback_id, {TITLE_COLUMNS}, season, number, progress, runtime, paused_at FROM trakt_playback
                 ORDER BY paused_at DESC"
            ))?;
            statement.query_map([], paused_row)?.collect()
        })
    }

    pub fn forget_trakt_paused(&self, playback_id: i64) -> anyhow::Result<()> {
        self.with(|db| db.execute("DELETE FROM trakt_playback WHERE playback_id = ?1", [playback_id]).map(|_| ()))
    }

    // MARK: Watchlist

    /// The watchlist titles both sides had after the last sync.
    pub fn trakt_listed(&self) -> anyhow::Result<Vec<TraktListed>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(&format!("SELECT {TITLE_COLUMNS}, item_ids FROM trakt_watchlist"))?;
            statement
                .query_map([], |row| {
                    let item_ids: String = row.get(6)?;
                    Ok(TraktListed { title: title_at(row, 0)?, item_ids: serde_json::from_str(&item_ids).unwrap_or_default() })
                })?
                .collect()
        })
    }

    pub fn replace_trakt_listed(&self, rows: &[TraktListed]) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("DELETE FROM trakt_watchlist", [])?;
            {
                let mut insert = tx.prepare_cached(&format!(
                    "INSERT OR REPLACE INTO trakt_watchlist ({TITLE_COLUMNS}, item_ids) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
                ))?;
                for row in rows.iter().filter(|row| row.title.ids.trakt.is_some()) {
                    let title = &row.title;
                    insert.execute(params![
                        kind_text(title.show),
                        title.ids.trakt,
                        title.ids.imdb,
                        title.ids.tmdb,
                        title.title_key,
                        title.year,
                        super::json_list(&row.item_ids)
                    ])?;
                }
            }
            tx.commit()
        })
    }

    // MARK: Lookups

    /// What looking a title up by name found: `None` when it's never been
    /// looked up, `Some(None)` when nothing matched confidently.
    pub fn trakt_lookup(&self, show: bool, title_key: &str, year: Option<i64>) -> anyhow::Result<Option<Option<TraktIds>>> {
        self.with(|db| {
            db.query_row(
                "SELECT trakt_id, imdb_id, tmdb_id FROM trakt_lookups WHERE kind = ?1 AND title_key = ?2 AND year = ?3",
                params![kind_text(show), title_key, year.unwrap_or(0)],
                |row| {
                    let ids = TraktIds { trakt: row.get(0)?, imdb: row.get(1)?, tmdb: row.get(2)? };
                    Ok((!ids.is_empty()).then_some(ids))
                },
            )
            .optional()
        })
    }

    pub fn save_trakt_lookup(
        &self,
        show: bool,
        title_key: &str,
        year: Option<i64>,
        found: Option<&TraktIds>,
        now: Timestamp,
    ) -> anyhow::Result<()> {
        let none = TraktIds::default();
        let ids = found.unwrap_or(&none);
        self.with(|db| {
            db.execute(
                "INSERT OR REPLACE INTO trakt_lookups (kind, title_key, year, trakt_id, imdb_id, tmdb_id, looked_up_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![kind_text(show), title_key, year.unwrap_or(0), ids.trakt, ids.imdb, ids.tmdb, now],
            )?;
            Ok(())
        })
    }

    /// Lookups that found a title, as (title key, year or 0, what was found).
    pub fn trakt_lookups_found(&self, show: bool) -> anyhow::Result<Vec<(String, i64, TraktIds)>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(
                "SELECT title_key, year, trakt_id, imdb_id, tmdb_id FROM trakt_lookups
                 WHERE kind = ?1 AND (trakt_id IS NOT NULL OR imdb_id IS NOT NULL OR tmdb_id IS NOT NULL)",
            )?;
            statement
                .query_map([kind_text(show)], |row| {
                    Ok((row.get(0)?, row.get(1)?, TraktIds { trakt: row.get(2)?, imdb: row.get(3)?, tmdb: row.get(4)? }))
                })?
                .collect()
        })
    }

    /// Forgets everything Trakt said about the account that was connected.
    /// Lookups stay: they're about titles, not anyone's account.
    pub fn forget_trakt_account(&self) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for table in ["trakt_watched", "trakt_playback", "trakt_watchlist"] {
                tx.execute(&format!("DELETE FROM {table}"), [])?;
            }
            tx.execute("DELETE FROM preferences WHERE key LIKE 'trakt_activity:%'", [])?;
            tx.commit()
        })
    }
}

/// Takes a removed source's titles out of the synced watchlist. A title left
/// with none of its items is forgotten, so the next sync sees it as on Trakt
/// and not yet here (and adds it back as another copy), rather than as
/// removed here (which would remove it from Trakt).
pub(super) fn forget_source_items(tx: &Connection, prefix: &str) -> rusqlite::Result<()> {
    let rows: Vec<(String, i64, String)> = {
        let mut statement = tx.prepare("SELECT kind, trakt_id, item_ids FROM trakt_watchlist")?;
        statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect::<rusqlite::Result<_>>()?
    };
    for (kind, trakt_id, item_ids) in rows {
        let ids: Vec<String> = serde_json::from_str(&item_ids).unwrap_or_default();
        let kept: Vec<&String> = ids.iter().filter(|id| !id.starts_with(prefix)).collect();
        if kept.len() == ids.len() {
            continue;
        }
        if kept.is_empty() {
            tx.execute("DELETE FROM trakt_watchlist WHERE kind = ?1 AND trakt_id = ?2", params![kind, trakt_id])?;
        } else {
            tx.execute(
                "UPDATE trakt_watchlist SET item_ids = ?3 WHERE kind = ?1 AND trakt_id = ?2",
                params![kind, trakt_id, super::json_list(&kept)],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(show: bool, trakt: i64) -> TraktTitle {
        TraktTitle { show, ids: TraktIds { trakt: Some(trakt), ..TraktIds::default() }, title_key: format!("title{trakt}"), year: Some(2020) }
    }

    #[test]
    fn libraries_from_version_6_gain_the_trakt_tables() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("library.sqlite");
        {
            let sixth = super::super::SCHEMA.split("-- What Trakt says").next().unwrap();
            let db = Connection::open(&path).unwrap();
            db.execute_batch(sixth).unwrap();
            db.execute("INSERT INTO preferences (key, value) VALUES ('device_id', 'kept')", []).unwrap();
            db.pragma_update(None, "user_version", 6).unwrap();
            assert!(db.prepare("SELECT 1 FROM trakt_watched").is_err(), "the test starts from the old tables");
        }
        let store = LibraryStore::open(&path).unwrap();
        assert_eq!(store.preference("device_id").unwrap().as_deref(), Some("kept"), "what was there stays");
        let watched = TraktWatched { title: title(true, 3), season: 1, number: 2, watched_at: 100 };
        store.replace_trakt_watched(true, std::slice::from_ref(&watched)).unwrap();
        assert_eq!(store.trakt_watched(true).unwrap(), std::slice::from_ref(&watched));
        assert_eq!(store.trakt_watched_shows().unwrap(), std::slice::from_ref(&watched.title));
        assert!(store.trakt_watched(false).unwrap().is_empty(), "films and shows are kept apart");
        store.forget_trakt_watched(true, 3, 1, 2).unwrap();
        assert!(store.trakt_watched_of_show(3).unwrap().is_empty());

        assert_eq!(store.trakt_lookup(false, "paperboats", Some(2021)).unwrap(), None, "never looked up");
        store.save_trakt_lookup(false, "paperboats", Some(2021), None, 5).unwrap();
        assert_eq!(store.trakt_lookup(false, "paperboats", Some(2021)).unwrap(), Some(None), "looked up, nothing found");
        let found = TraktIds { trakt: Some(2), ..TraktIds::default() };
        store.save_trakt_lookup(true, "tidewater", None, Some(&found), 5).unwrap();
        assert_eq!(store.trakt_lookup(true, "tidewater", None).unwrap(), Some(Some(found.clone())));
        assert_eq!(store.trakt_lookups_found(true).unwrap(), [("tidewater".to_string(), 0, found)]);
        assert!(store.trakt_lookups_found(false).unwrap().is_empty());
    }

    #[test]
    fn a_removed_sources_titles_leave_the_synced_watchlist() {
        let folder = tempfile::tempdir().unwrap();
        let store = LibraryStore::open(&folder.path().join("library.sqlite")).unwrap();
        store
            .replace_trakt_listed(&[
                TraktListed { title: title(false, 1), item_ids: vec!["gone|mv|1".into()] },
                TraktListed { title: title(false, 2), item_ids: vec!["gone|mv|2".into(), "kept|mv|2".into()] },
                TraktListed { title: title(true, 3), item_ids: vec!["addon|series|tt0000003".into()] },
            ])
            .unwrap();
        store.remove_source("gone").unwrap();
        let left: Vec<(Option<i64>, Vec<String>)> =
            store.trakt_listed().unwrap().into_iter().map(|row| (row.title.ids.trakt, row.item_ids)).collect();
        assert_eq!(left.len(), 2);
        assert!(left.contains(&(Some(2), vec!["kept|mv|2".to_string()])));
        assert!(left.contains(&(Some(3), vec!["addon|series|tt0000003".to_string()])));

        store.forget_trakt_account().unwrap();
        assert!(store.trakt_listed().unwrap().is_empty());
    }
}
