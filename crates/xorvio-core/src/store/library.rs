//! What people make themselves: the watchlist, favorite channels and their
//! own channel lists, playback progress, what they've watched, their changes
//! to channels and categories (names, hiding, locks, catch-up offsets, the
//! version of a channel to play), added guides and guide channels chosen by
//! hand, what Trakt says (trakt.rs), and library profiles with Home's layout
//! for each. Kept in its own database so it survives catalog rebuilds and
//! syncs between computers on its own (sync.rs).

mod sync;
mod trakt;

pub(crate) use sync::JournalTables;
pub(crate) use trakt::{TraktListed, TraktPaused, TraktTitle, TraktWatched};

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use xorvio_model::{GuideSource, MediaKind, PlaybackProgress, Timestamp, WatchlistEntry};

use super::{GuideOverride, json_list, lock, open_connection, sql_limit};

/// Every version so far only added tables, which [`SCHEMA`] makes when
/// they're missing; existing ones keep their rows. Version 8 adds history
/// titles, 9 library profiles and Home's layouts, 10 syncing (change times
/// and tombstones).
pub(crate) const SCHEMA_VERSION: i64 = 10;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS watchlist (
    item_id TEXT PRIMARY KEY, kind TEXT NOT NULL, source_id TEXT NOT NULL, title TEXT NOT NULL, subtitle TEXT,
    artwork TEXT, added_at INTEGER NOT NULL, channel_id TEXT, start INTEGER, end INTEGER, remind INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS watchlist_added ON watchlist(added_at DESC);

CREATE TABLE IF NOT EXISTS favorites (
    item_id TEXT NOT NULL, kind TEXT NOT NULL, list_id TEXT NOT NULL DEFAULT 'default',
    sort_index INTEGER NOT NULL, added_at INTEGER NOT NULL, PRIMARY KEY (item_id, list_id)
);

CREATE TABLE IF NOT EXISTS progress (
    item_id TEXT PRIMARY KEY, kind TEXT NOT NULL, source_id TEXT NOT NULL, series_id TEXT,
    position REAL NOT NULL, duration REAL NOT NULL, finished INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS progress_updated ON progress(updated_at DESC);
CREATE INDEX IF NOT EXISTS progress_series ON progress(series_id);

CREATE TABLE IF NOT EXISTS history (
    id INTEGER PRIMARY KEY AUTOINCREMENT, item_id TEXT NOT NULL, kind TEXT NOT NULL, source_id TEXT NOT NULL,
    watched_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS history_date ON history(watched_at DESC);
-- What each watched title was called when it played (schema 8), for Viewing
-- History when the catalog no longer has it (add-on titles never are).
CREATE TABLE IF NOT EXISTS history_titles (item_id TEXT PRIMARY KEY, title TEXT NOT NULL, subtitle TEXT);
CREATE TABLE IF NOT EXISTS dismissed_shows (show_id TEXT PRIMARY KEY, dismissed_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS reminders_sent (item_id TEXT PRIMARY KEY, sent_at INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS guide_sources (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, display_host TEXT, offset_minutes INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL, sort_index INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS guide_overrides (
    channel_id TEXT PRIMARY KEY, guide_id TEXT NOT NULL, epg_id TEXT NOT NULL, set_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS favorite_lists (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, sort_index INTEGER NOT NULL, created_at INTEGER NOT NULL
);

-- A NULL lock leaves it to the name: categories that look adult lock
-- themselves once there's a PIN, and 0 is someone unlocking one of those.
CREATE TABLE IF NOT EXISTS item_edits (
    item_id TEXT PRIMARY KEY, display_name TEXT, hidden INTEGER NOT NULL DEFAULT 0, locked INTEGER
);

-- Minutes a channel's replays are moved by, for providers whose replays
-- start early or late. Only channels with an offset have a row.
CREATE TABLE IF NOT EXISTS catchup_offsets (channel_id TEXT PRIMARY KEY, minutes INTEGER NOT NULL);

-- The version chosen to play for a channel the provider lists several times,
-- by the key of its group (see grouping.rs), which starts with the source id.
CREATE TABLE IF NOT EXISTS preferred_versions (group_key TEXT PRIMARY KEY, channel_id TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS preferences (key TEXT PRIMARY KEY, value TEXT NOT NULL);

-- Library profiles (see profiles.rs), each a LibraryProfile as JSON, in the
-- order they were made. The one in use is the 'active_profile' preference.
CREATE TABLE IF NOT EXISTS profiles (id TEXT PRIMARY KEY, sort_index INTEGER NOT NULL, json TEXT NOT NULL);
-- Home's rows for each profile as the interface arranges them (JSON), one
-- layout for when the sources lead Home ('sources') and one for add-ons ('addons').
CREATE TABLE IF NOT EXISTS home_layouts (
    profile_id TEXT NOT NULL, mode TEXT NOT NULL, json TEXT NOT NULL, PRIMARY KEY (profile_id, mode)
);

-- What Trakt says, as of the last sync (see trakt.rs). Titles are Trakt's:
-- kind 'movie' or 'show', its Trakt id, IMDb and TMDB ids, and its title as a
-- title key with its year, for catalog titles that have no ids.
-- Films, and episodes (a show's rows, by season and number), watched on Trakt.
CREATE TABLE IF NOT EXISTS trakt_watched (
    kind TEXT NOT NULL, trakt_id INTEGER NOT NULL, imdb_id TEXT, tmdb_id INTEGER, title_key TEXT NOT NULL, year INTEGER,
    season INTEGER NOT NULL DEFAULT 0, number INTEGER NOT NULL DEFAULT 0, watched_at INTEGER NOT NULL,
    PRIMARY KEY (kind, trakt_id, season, number)
);
-- Places left part-way, by Trakt's playback id, with the progress in percent.
CREATE TABLE IF NOT EXISTS trakt_playback (
    playback_id INTEGER PRIMARY KEY, kind TEXT NOT NULL, trakt_id INTEGER NOT NULL, imdb_id TEXT, tmdb_id INTEGER,
    title_key TEXT NOT NULL, year INTEGER, season INTEGER NOT NULL DEFAULT 0, number INTEGER NOT NULL DEFAULT 0,
    progress REAL NOT NULL, runtime INTEGER, paused_at INTEGER NOT NULL
);
-- Watchlist titles both sides had after the last sync, with the Xorvio items
-- they were (a JSON list), so a removal on one side is told from a title
-- that was never synced.
CREATE TABLE IF NOT EXISTS trakt_watchlist (
    kind TEXT NOT NULL, trakt_id INTEGER NOT NULL, imdb_id TEXT, tmdb_id INTEGER, title_key TEXT NOT NULL, year INTEGER,
    item_ids TEXT NOT NULL, PRIMARY KEY (kind, trakt_id)
);
-- Titles without ids looked up on Trakt by title and year (0: no year), once
-- each: what was found, or no ids when nothing matched confidently.
CREATE TABLE IF NOT EXISTS trakt_lookups (
    kind TEXT NOT NULL, title_key TEXT NOT NULL, year INTEGER NOT NULL DEFAULT 0,
    trakt_id INTEGER, imdb_id TEXT, tmdb_id INTEGER, looked_up_at INTEGER NOT NULL,
    PRIMARY KEY (kind, title_key, year)
);

-- Syncing between computers (schema 10; see library/sync.rs). The tables it
-- names also gain a changed_at column and triggers each time the library
-- opens. Deleted rows, by table and key (a JSON array of the key's values):
CREATE TABLE IF NOT EXISTS sync_tombstones (
    table_name TEXT NOT NULL, row_key TEXT NOT NULL, deleted_at INTEGER NOT NULL, PRIMARY KEY (table_name, row_key)
);
-- How many synced rows this computer changed itself, so the app knows to sync.
CREATE TABLE IF NOT EXISTS sync_meta (id INTEGER PRIMARY KEY CHECK (id = 1), local_changes INTEGER NOT NULL DEFAULT 0);
INSERT OR IGNORE INTO sync_meta (id, local_changes) VALUES (1, 0);
-- Holds a row only inside a transaction whose writes aren't this computer's
-- own changes (a merge, removing a source), so the triggers stand aside.
CREATE TABLE IF NOT EXISTS sync_untracked (busy INTEGER NOT NULL);
-- History rows are named by item and time when they sync.
CREATE INDEX IF NOT EXISTS history_item ON history(item_id, watched_at);
";

/// The built-in Favorites list. Lists people name themselves have random ids.
pub(crate) const DEFAULT_LIST: &str = "default";

pub(crate) fn kind_name(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Channel => "channel",
        MediaKind::Movie => "movie",
        MediaKind::Series => "series",
        MediaKind::Episode => "episode",
        MediaKind::Program => "program",
        MediaKind::Recording => "recording",
    }
}

fn kind_from(text: &str) -> MediaKind {
    match text {
        "channel" => MediaKind::Channel,
        "series" => MediaKind::Series,
        "episode" => MediaKind::Episode,
        "program" => MediaKind::Program,
        "recording" => MediaKind::Recording,
        _ => MediaKind::Movie,
    }
}

const WATCHLIST_COLUMNS: &str = "item_id, kind, source_id, title, subtitle, artwork, added_at, channel_id, start, end, remind";

fn watchlist_row(row: &Row<'_>) -> rusqlite::Result<WatchlistEntry> {
    Ok(WatchlistEntry {
        id: row.get(0)?,
        kind: kind_from(&row.get::<_, String>(1)?),
        source_id: row.get(2)?,
        title: row.get(3)?,
        subtitle: row.get(4)?,
        artwork_url: row.get(5)?,
        added_at: row.get(6)?,
        channel_id: row.get(7)?,
        start: row.get(8)?,
        end: row.get(9)?,
        remind: row.get::<_, i64>(10)? != 0,
    })
}

const PROGRESS_COLUMNS: &str = "item_id, kind, source_id, series_id, position, duration, finished, updated_at";

fn progress_row(row: &Row<'_>) -> rusqlite::Result<PlaybackProgress> {
    Ok(PlaybackProgress {
        item_id: row.get(0)?,
        kind: kind_from(&row.get::<_, String>(1)?),
        source_id: row.get(2)?,
        series_id: row.get(3)?,
        position_seconds: row.get(4)?,
        duration_seconds: row.get(5)?,
        finished: row.get::<_, i64>(6)? != 0,
        updated_at: row.get(7)?,
    })
}

/// Worth offering "Resume": past the opening and not effectively finished.
pub(crate) fn is_resumable(progress: &PlaybackProgress) -> bool {
    !progress.finished && progress.position_seconds > 30.0 && fraction(progress) < 0.95
}

/// How far through, 0…1.
pub(crate) fn fraction(progress: &PlaybackProgress) -> f64 {
    if progress.duration_seconds > 0.0 && progress.position_seconds.is_finite() {
        (progress.position_seconds / progress.duration_seconds).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Someone's changes to one channel or category, as stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EditRow {
    pub item_id: String,
    /// Their own name for it; the catalog keeps the provider's.
    pub display_name: Option<String>,
    pub hidden: bool,
    /// `None` leaves it to the name (see the schema).
    pub locked: Option<bool>,
}

impl EditRow {
    fn is_empty(&self) -> bool {
        self.display_name.is_none() && !self.hidden && self.locked.is_none()
    }
}

fn edit_row(row: &Row<'_>) -> rusqlite::Result<EditRow> {
    Ok(EditRow {
        item_id: row.get(0)?,
        display_name: row.get(1)?,
        hidden: row.get::<_, i64>(2)? != 0,
        locked: row.get::<_, Option<i64>>(3)?.map(|locked| locked != 0),
    })
}

/// A line of the history, as stored.
pub(crate) struct HistoryRow {
    pub item_id: String,
    pub kind: MediaKind,
    pub source_id: String,
    pub watched_at: Timestamp,
    pub title: Option<String>,
    pub subtitle: Option<String>,
}

pub(crate) struct LibraryStore {
    db: Mutex<Connection>,
}

impl LibraryStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let mut db = open_connection(path)?;
        let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < SCHEMA_VERSION {
            db.execute_batch(SCHEMA)?;
            db.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        sync::prepare(&mut db)?;
        Ok(Self { db: Mutex::new(db) })
    }

    /// A consistent copy of the whole library at `path` (a new file), made
    /// by SQLite while the library stays open, for a backup.
    pub fn snapshot_into(&self, path: &Path) -> anyhow::Result<()> {
        let target = path.to_str().ok_or_else(|| anyhow::anyhow!("The backup's working folder has an unusual name."))?;
        self.with(|db| db.execute("VACUUM INTO ?1", [target]).map(|_| ()))
    }

    fn with<T>(&self, work: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> anyhow::Result<T> {
        Ok(work(&mut lock(&self.db))?)
    }

    /// Titles that are now called by other ids (a media folder's video moved
    /// or renamed) keep their watchlist place, progress, history and edits.
    pub fn rekey_items(&self, moves: &[(String, String)]) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction()?;
            for (from, to) in moves {
                for table in ["watchlist", "progress", "history_titles", "item_edits"] {
                    tx.execute(&format!("UPDATE OR REPLACE {table} SET item_id = ?2 WHERE item_id = ?1"), params![from, to])?;
                }
                tx.execute("UPDATE history SET item_id = ?2 WHERE item_id = ?1", params![from, to])?;
            }
            tx.commit()
        })
    }

    // MARK: Watchlist

    /// Everything on the watchlist, most recently added first.
    pub fn watchlist(&self) -> anyhow::Result<Vec<WatchlistEntry>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(&format!("SELECT {WATCHLIST_COLUMNS} FROM watchlist ORDER BY added_at DESC"))?;
            statement.query_map([], watchlist_row)?.collect()
        })
    }

    /// Adds an entry, or updates its details while keeping when it was first added.
    pub fn add_to_watchlist(&self, entry: &WatchlistEntry) -> anyhow::Result<()> {
        self.with(|db| upsert_watchlist(db, entry))
    }

    pub fn is_watchlisted(&self, item_id: &str) -> anyhow::Result<bool> {
        self.with(|db| {
            db.query_row("SELECT 1 FROM watchlist WHERE item_id = ?1", [item_id], |_| Ok(()))
                .optional()
                .map(|found| found.is_some())
        })
    }

    pub fn remove_from_watchlist(&self, item_id: &str) -> anyhow::Result<()> {
        self.with(|db| db.execute("DELETE FROM watchlist WHERE item_id = ?1", [item_id]).map(|_| ()))
    }

    /// Swaps a programme for its rescheduled listing (whose id carries the new start time).
    pub fn replace_watchlist_entry(&self, old_id: &str, entry: &WatchlistEntry) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("DELETE FROM watchlist WHERE item_id = ?1", [old_id])?;
            upsert_watchlist(&tx, entry)?;
            tx.commit()
        })
    }

    /// Removes programmes that have finished airing and returns their ids.
    pub fn remove_ended_programs(&self, now: Timestamp) -> anyhow::Result<Vec<String>> {
        self.with(|db| {
            let ended: Vec<String> = {
                let mut statement =
                    db.prepare_cached("SELECT item_id FROM watchlist WHERE kind = 'program' AND end IS NOT NULL AND end <= ?1")?;
                statement.query_map([now], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?
            };
            if !ended.is_empty() {
                db.execute("DELETE FROM watchlist WHERE kind = 'program' AND end IS NOT NULL AND end <= ?1", [now])?;
            }
            Ok(ended)
        })
    }

    // MARK: Reminders

    /// Reminded programmes whose reminder hasn't gone out yet. A programme the
    /// guide moved has a new id (it carries the start), so it's reminded again
    /// for its new time.
    pub fn unsent_reminders(&self) -> anyhow::Result<Vec<WatchlistEntry>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {WATCHLIST_COLUMNS} FROM watchlist
                 WHERE kind = 'program' AND remind = 1 AND start IS NOT NULL
                    AND item_id NOT IN (SELECT item_id FROM reminders_sent)"
            ))?;
            statement.query_map([], watchlist_row)?.collect()
        })
    }

    /// Notes reminders went out at `now`. A sent mark only matters until its
    /// programme starts, so marks older than a day are cleared on the way.
    pub fn mark_reminders_sent(&self, item_ids: &[String], now: Timestamp) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for item_id in item_ids {
                tx.execute("INSERT OR REPLACE INTO reminders_sent (item_id, sent_at) VALUES (?1, ?2)", params![item_id, now])?;
            }
            tx.execute("DELETE FROM reminders_sent WHERE sent_at < ?1", [now - 86_400])?;
            tx.commit()
        })
    }

    // MARK: Favorites and channel lists

    /// A list's items in its order: Favorites ([`DEFAULT_LIST`]) or a named list.
    pub fn list_items(&self, list_id: &str) -> anyhow::Result<Vec<String>> {
        self.with(|db| {
            let mut statement = db.prepare_cached("SELECT item_id FROM favorites WHERE list_id = ?1 ORDER BY sort_index")?;
            statement.query_map([list_id], |row| row.get(0))?.collect()
        })
    }

    /// Adds an item to the end of a list, or takes it out.
    pub fn set_in_list(&self, item_id: &str, kind: MediaKind, list_id: &str, member: bool, now: Timestamp) -> anyhow::Result<()> {
        self.with(|db| {
            if member {
                db.execute(
                    "INSERT OR IGNORE INTO favorites (item_id, kind, list_id, sort_index, added_at)
                     VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(sort_index), -1) + 1 FROM favorites WHERE list_id = ?3), ?4)",
                    params![item_id, kind_name(kind), list_id, now],
                )?;
            } else {
                db.execute("DELETE FROM favorites WHERE item_id = ?1 AND list_id = ?2", params![item_id, list_id])?;
            }
            Ok(())
        })
    }

    /// Puts a list's items in a new order. Items left out keep their places,
    /// so reordering the channels shown never moves ones that are hidden or
    /// from a paused source.
    pub fn reorder_list(&self, list_id: &str, ordered: &[String]) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current: Vec<String> = {
                let mut statement = tx.prepare_cached("SELECT item_id FROM favorites WHERE list_id = ?1 ORDER BY sort_index")?;
                statement.query_map([list_id], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?
            };
            let present: HashSet<&str> = current.iter().map(String::as_str).collect();
            let mut named = HashSet::new();
            let moved: Vec<&str> =
                ordered.iter().map(String::as_str).filter(|id| present.contains(id) && named.insert(*id)).collect();
            // The named items take the places they held between them, in their new order.
            let mut next = moved.into_iter();
            for (index, id) in current.iter().enumerate() {
                let item = if named.contains(id.as_str()) { next.next().unwrap_or(id.as_str()) } else { id.as_str() };
                tx.execute(
                    "UPDATE favorites SET sort_index = ?1 WHERE list_id = ?2 AND item_id = ?3",
                    params![index as i64, list_id, item],
                )?;
            }
            tx.commit()
        })
    }

    /// Lists people named themselves, as (id, name), in the order they were made.
    pub fn lists(&self) -> anyhow::Result<Vec<(String, String)>> {
        self.with(|db| {
            let mut statement = db.prepare_cached("SELECT id, name FROM favorite_lists ORDER BY sort_index")?;
            statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
        })
    }

    pub fn create_list(&self, id: &str, name: &str, now: Timestamp) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO favorite_lists (id, name, sort_index, created_at)
                 VALUES (?1, ?2, (SELECT COALESCE(MAX(sort_index), -1) + 1 FROM favorite_lists), ?3)",
                params![id, name, now],
            )
            .map(|_| ())
        })
    }

    /// Renames a list; false when there's no such list.
    pub fn rename_list(&self, id: &str, name: &str) -> anyhow::Result<bool> {
        self.with(|db| Ok(db.execute("UPDATE favorite_lists SET name = ?2 WHERE id = ?1", params![id, name])? > 0))
    }

    /// Deletes a list and its entries; the channels themselves stay.
    pub fn delete_list(&self, id: &str) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("DELETE FROM favorites WHERE list_id = ?1", [id])?;
            tx.execute("DELETE FROM favorite_lists WHERE id = ?1", [id])?;
            tx.commit()
        })
    }

    // MARK: Progress

    /// Stores progress. Should two devices ever sync, the most recent write wins.
    pub fn save_progress(&self, progress: &PlaybackProgress) -> anyhow::Result<()> {
        self.with(|db| upsert_progress(db, progress).map(|_| ()))
    }

    /// Stores several at once, each only over older progress, and says how
    /// many rows changed.
    pub fn save_progress_many(&self, rows: &[PlaybackProgress]) -> anyhow::Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut changed = 0;
            for progress in rows {
                changed += upsert_progress(&tx, progress)?;
            }
            tx.commit()?;
            Ok(changed)
        })
    }

    /// Progress for each of these items that has any, by item id.
    pub fn progress_many(&self, item_ids: &[String]) -> anyhow::Result<HashMap<String, PlaybackProgress>> {
        if item_ids.is_empty() {
            return Ok(HashMap::new());
        }
        self.with(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {PROGRESS_COLUMNS} FROM progress WHERE item_id IN (SELECT value FROM json_each(?1))"
            ))?;
            statement
                .query_map([json_list(item_ids)], progress_row)?
                .map(|row| row.map(|progress| (progress.item_id.clone(), progress)))
                .collect()
        })
    }

    pub fn progress(&self, item_id: &str) -> anyhow::Result<Option<PlaybackProgress>> {
        self.with(|db| {
            db.query_row(&format!("SELECT {PROGRESS_COLUMNS} FROM progress WHERE item_id = ?1"), [item_id], progress_row)
                .optional()
        })
    }

    /// Resumable items, most recent first, one per series.
    pub fn resumable(&self, limit: usize) -> anyhow::Result<Vec<PlaybackProgress>> {
        let rows: Vec<PlaybackProgress> = self.with(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {PROGRESS_COLUMNS} FROM progress WHERE finished = 0 AND position > 30 ORDER BY updated_at DESC LIMIT ?1"
            ))?;
            statement.query_map([sql_limit(Some(limit.saturating_mul(3)))], progress_row)?.collect()
        })?;
        let mut seen_series = std::collections::HashSet::new();
        Ok(rows
            .into_iter()
            .filter(is_resumable)
            .filter(|row| row.series_id.as_ref().is_none_or(|series| seen_series.insert(series.clone())))
            .take(limit)
            .collect())
    }

    /// Films and episodes watched to the end (or marked watched), most recent first.
    pub fn finished(&self, limit: usize) -> anyhow::Result<Vec<PlaybackProgress>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {PROGRESS_COLUMNS} FROM progress WHERE finished = 1 ORDER BY updated_at DESC LIMIT ?1"
            ))?;
            statement.query_map([sql_limit(Some(limit))], progress_row)?.collect()
        })
    }

    /// Forgets where someone got to in a film or episode (marking it unwatched).
    pub fn remove_progress(&self, item_id: &str) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute("DELETE FROM progress WHERE item_id = ?1", [item_id])?;
            Ok(())
        })
    }

    /// Notes a series was taken off Continue Watching at `at`.
    pub fn dismiss_show(&self, show_id: &str, at: Timestamp) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO dismissed_shows (show_id, dismissed_at) VALUES (?1, ?2)
                 ON CONFLICT(show_id) DO UPDATE SET dismissed_at = excluded.dismissed_at",
                params![show_id, at],
            )?;
            Ok(())
        })
    }

    /// Series taken off Continue Watching, with when.
    pub fn dismissed_shows(&self) -> anyhow::Result<std::collections::HashMap<String, Timestamp>> {
        self.with(|db| {
            let mut statement = db.prepare_cached("SELECT show_id, dismissed_at FROM dismissed_shows")?;
            statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
        })
    }

    /// Every film's (or episode's) progress, for filtering lists by what's
    /// been watched. Bounded by what someone has played or marked.
    pub fn progress_of_kind(&self, kind: MediaKind) -> anyhow::Result<Vec<PlaybackProgress>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(&format!("SELECT {PROGRESS_COLUMNS} FROM progress WHERE kind = ?1"))?;
            statement.query_map([kind_name(kind)], progress_row)?.collect()
        })
    }

    /// The progress of every episode of these series that has any.
    pub fn progress_of_series(&self, series_ids: &[String]) -> anyhow::Result<Vec<PlaybackProgress>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {PROGRESS_COLUMNS} FROM progress WHERE series_id IN (SELECT value FROM json_each(?1))"
            ))?;
            statement.query_map([json_list(series_ids)], progress_row)?.collect()
        })
    }

    /// Episode ids of a series watched to the end.
    pub fn finished_in_series(&self, series_id: &str) -> anyhow::Result<Vec<String>> {
        self.with(|db| {
            let mut statement = db.prepare_cached("SELECT item_id FROM progress WHERE series_id = ?1 AND finished = 1")?;
            statement.query_map([series_id], |row| row.get(0))?.collect()
        })
    }

    // MARK: Item edits

    /// Every channel and category someone renamed, hid or set a lock on.
    pub fn item_edits(&self) -> anyhow::Result<Vec<EditRow>> {
        self.with(|db| {
            let mut statement = db.prepare_cached("SELECT item_id, display_name, hidden, locked FROM item_edits")?;
            statement.query_map([], edit_row)?.collect()
        })
    }

    /// Changes one item's edits. A row left with nothing set is removed, so
    /// the item is back to just as the provider sends it.
    pub fn edit_item(&self, item_id: &str, change: impl FnOnce(&mut EditRow)) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut row = tx
                .query_row("SELECT item_id, display_name, hidden, locked FROM item_edits WHERE item_id = ?1", [item_id], edit_row)
                .optional()?
                .unwrap_or_else(|| EditRow { item_id: item_id.to_string(), ..EditRow::default() });
            change(&mut row);
            if row.is_empty() {
                tx.execute("DELETE FROM item_edits WHERE item_id = ?1", [item_id])?;
            } else {
                tx.execute(
                    "INSERT INTO item_edits (item_id, display_name, hidden, locked) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(item_id) DO UPDATE SET display_name = excluded.display_name, hidden = excluded.hidden,
                        locked = excluded.locked",
                    params![item_id, row.display_name, row.hidden, row.locked],
                )?;
            }
            tx.commit()
        })
    }

    /// Shows everything hidden again, keeping names and locks.
    pub fn show_all_hidden(&self) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("UPDATE item_edits SET hidden = 0", [])?;
            tx.execute("DELETE FROM item_edits WHERE display_name IS NULL AND hidden = 0 AND locked IS NULL", [])?;
            tx.commit()
        })
    }

    // MARK: Catch-up offsets and versions

    /// Channels whose replays are moved, with the minutes.
    pub fn catchup_offsets(&self) -> anyhow::Result<Vec<(String, i64)>> {
        self.with(|db| {
            let mut statement = db.prepare_cached("SELECT channel_id, minutes FROM catchup_offsets")?;
            statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
        })
    }

    /// Sets a channel's catch-up offset; 0 removes it.
    pub fn set_catchup_offset(&self, channel_id: &str, minutes: i64) -> anyhow::Result<()> {
        self.with(|db| {
            if minutes == 0 {
                db.execute("DELETE FROM catchup_offsets WHERE channel_id = ?1", [channel_id])?;
            } else {
                db.execute(
                    "INSERT INTO catchup_offsets (channel_id, minutes) VALUES (?1, ?2)
                     ON CONFLICT(channel_id) DO UPDATE SET minutes = excluded.minutes",
                    params![channel_id, minutes],
                )?;
            }
            Ok(())
        })
    }

    /// Versions chosen to play, as (group key, channel id).
    pub fn preferred_versions(&self) -> anyhow::Result<Vec<(String, String)>> {
        self.with(|db| {
            let mut statement = db.prepare_cached("SELECT group_key, channel_id FROM preferred_versions")?;
            statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
        })
    }

    /// Remembers the version a group plays, or forgets the choice (`None`).
    pub fn set_preferred_version(&self, group_key: &str, channel_id: Option<&str>) -> anyhow::Result<()> {
        self.with(|db| {
            match channel_id {
                Some(channel_id) => db.execute(
                    "INSERT INTO preferred_versions (group_key, channel_id) VALUES (?1, ?2)
                     ON CONFLICT(group_key) DO UPDATE SET channel_id = excluded.channel_id",
                    params![group_key, channel_id],
                )?,
                None => db.execute("DELETE FROM preferred_versions WHERE group_key = ?1", [group_key])?,
            };
            Ok(())
        })
    }

    // MARK: Preferences

    pub fn preference(&self, key: &str) -> anyhow::Result<Option<String>> {
        self.with(|db| db.query_row("SELECT value FROM preferences WHERE key = ?1", [key], |row| row.get(0)).optional())
    }

    /// Stores a value, or removes it when `None`.
    pub fn set_preference(&self, key: &str, value: Option<&str>) -> anyhow::Result<()> {
        self.with(|db| {
            match value {
                Some(value) => db.execute(
                    "INSERT INTO preferences (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )?,
                None => db.execute("DELETE FROM preferences WHERE key = ?1", [key])?,
            };
            Ok(())
        })
    }

    // MARK: Profiles and Home

    /// Profiles as (id, JSON), in the order they were made.
    pub fn profiles(&self) -> anyhow::Result<Vec<(String, String)>> {
        self.with(|db| {
            let mut statement = db.prepare_cached("SELECT id, json FROM profiles ORDER BY sort_index")?;
            statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
        })
    }

    /// Adds a profile at the end, or saves one in its place.
    pub fn save_profile(&self, id: &str, json: &str) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO profiles (id, sort_index, json)
                 VALUES (?1, (SELECT COALESCE(MAX(sort_index), -1) + 1 FROM profiles), ?2)
                 ON CONFLICT(id) DO UPDATE SET json = excluded.json",
                params![id, json],
            )
            .map(|_| ())
        })
    }

    /// Deletes a profile with its Home layouts.
    pub fn delete_profile(&self, id: &str) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("DELETE FROM profiles WHERE id = ?1", [id])?;
            tx.execute("DELETE FROM home_layouts WHERE profile_id = ?1", [id])?;
            tx.commit()
        })
    }

    pub fn home_layout(&self, profile_id: &str, mode: &str) -> anyhow::Result<Option<String>> {
        self.with(|db| {
            db.query_row("SELECT json FROM home_layouts WHERE profile_id = ?1 AND mode = ?2", [profile_id, mode], |row| row.get(0))
                .optional()
        })
    }

    /// Saves a profile's Home layout, or forgets it (`None`) so Home is back to the standard one.
    pub fn set_home_layout(&self, profile_id: &str, mode: &str, json: Option<&str>) -> anyhow::Result<()> {
        self.with(|db| {
            match json {
                Some(json) => db.execute(
                    "INSERT INTO home_layouts (profile_id, mode, json) VALUES (?1, ?2, ?3)
                     ON CONFLICT(profile_id, mode) DO UPDATE SET json = excluded.json",
                    params![profile_id, mode, json],
                )?,
                None => db.execute("DELETE FROM home_layouts WHERE profile_id = ?1 AND mode = ?2", [profile_id, mode])?,
            };
            Ok(())
        })
    }

    // MARK: History

    /// Notes something was watched. Repeated plays within an hour count once.
    pub fn add_history(&self, item_id: &str, kind: MediaKind, source_id: &str, now: Timestamp) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("DELETE FROM history WHERE item_id = ?1 AND watched_at > ?2", params![item_id, now - 3600])?;
            tx.execute(
                "INSERT INTO history (item_id, kind, source_id, watched_at) VALUES (?1, ?2, ?3, ?4)",
                params![item_id, kind_name(kind), source_id, now],
            )?;
            tx.commit()
        })
    }

    /// Keeps what a watched title was called, for Viewing History.
    pub fn note_history_title(&self, item_id: &str, title: &str, subtitle: Option<&str>) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO history_titles (item_id, title, subtitle) VALUES (?1, ?2, ?3)
                 ON CONFLICT(item_id) DO UPDATE SET title = excluded.title, subtitle = excluded.subtitle",
                params![item_id, title, subtitle],
            )?;
            Ok(())
        })
    }

    /// Everything watched, newest first, with the names kept when it played.
    pub fn history(&self, limit: usize) -> anyhow::Result<Vec<HistoryRow>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(
                "SELECT h.item_id, h.kind, h.source_id, h.watched_at, t.title, t.subtitle
                 FROM history h LEFT JOIN history_titles t ON t.item_id = h.item_id
                 ORDER BY h.watched_at DESC, h.id DESC LIMIT ?1",
            )?;
            statement
                .query_map([sql_limit(Some(limit))], |row| {
                    Ok(HistoryRow {
                        item_id: row.get(0)?,
                        kind: kind_from(&row.get::<_, String>(1)?),
                        source_id: row.get(2)?,
                        watched_at: row.get(3)?,
                        title: row.get(4)?,
                        subtitle: row.get(5)?,
                    })
                })?
                .collect()
        })
    }

    /// Forgets the history (and so Recent Channels); progress and Continue
    /// Watching stay.
    pub fn clear_history(&self) -> anyhow::Result<()> {
        self.with(|db| db.execute_batch("DELETE FROM history; DELETE FROM history_titles;"))
    }

    pub fn recent_channel_ids(&self, limit: usize) -> anyhow::Result<Vec<String>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(
                "SELECT item_id FROM history WHERE kind = 'channel' GROUP BY item_id ORDER BY MAX(watched_at) DESC LIMIT ?1",
            )?;
            statement.query_map([sql_limit(Some(limit))], |row| row.get(0))?.collect()
        })
    }

    // MARK: Guides

    /// Added guides in the user's order.
    pub fn guide_sources(&self) -> anyhow::Result<Vec<GuideSource>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(
                "SELECT id, name, display_host, offset_minutes, created_at, sort_index FROM guide_sources ORDER BY sort_index",
            )?;
            statement
                .query_map([], |row| {
                    Ok(GuideSource {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        display_host: row.get(2)?,
                        offset_minutes: row.get(3)?,
                        created_at: row.get(4)?,
                        sort_index: row.get(5)?,
                    })
                })?
                .collect()
        })
    }

    /// Adds a guide, or saves its edited settings.
    pub fn save_guide_source(&self, guide: &GuideSource) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO guide_sources (id, name, display_host, offset_minutes, created_at, sort_index)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name, display_host = excluded.display_host,
                    offset_minutes = excluded.offset_minutes, sort_index = excluded.sort_index",
                params![guide.id, guide.name, guide.display_host, guide.offset_minutes, guide.created_at, guide.sort_index],
            )?;
            Ok(())
        })
    }

    /// Removes an added guide and every guide channel chosen from it. Added
    /// guides don't sync, so this is tidying up here, not a deletion for the
    /// other computers to copy.
    pub fn remove_guide_source(&self, id: &str) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            sync::untracked(&tx)?;
            tx.execute("DELETE FROM guide_sources WHERE id = ?1", [id])?;
            tx.execute("DELETE FROM guide_overrides WHERE guide_id = ?1", [id])?;
            sync::track_again(&tx)?;
            tx.commit()
        })
    }

    /// Guide channels chosen by hand.
    pub fn guide_overrides(&self) -> anyhow::Result<Vec<GuideOverride>> {
        self.with(|db| {
            let mut statement = db.prepare_cached("SELECT channel_id, guide_id, epg_id FROM guide_overrides")?;
            statement
                .query_map([], |row| Ok(GuideOverride { channel_id: row.get(0)?, guide_id: row.get(1)?, epg_id: row.get(2)? }))?
                .collect()
        })
    }

    pub fn set_guide_override(&self, choice: &GuideOverride, now: Timestamp) -> anyhow::Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO guide_overrides (channel_id, guide_id, epg_id, set_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(channel_id) DO UPDATE SET guide_id = excluded.guide_id, epg_id = excluded.epg_id,
                    set_at = excluded.set_at",
                params![choice.channel_id, choice.guide_id, choice.epg_id, now],
            )?;
            Ok(())
        })
    }

    pub fn remove_guide_override(&self, channel_id: &str) -> anyhow::Result<()> {
        self.with(|db| db.execute("DELETE FROM guide_overrides WHERE channel_id = ?1", [channel_id]).map(|_| ()))
    }

    /// Removes personal state tied to a deleted source, including guide
    /// channels chosen for its channels or from its guide. Sources don't
    /// sync, so this is tidying up here: another computer with the same
    /// source keeps its watchlist and progress for it.
    pub fn remove_source(&self, source_id: &str) -> anyhow::Result<()> {
        let prefix = format!("{source_id}|");
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            sync::untracked(&tx)?;
            for (table, column) in
                [("favorites", "item_id"), ("item_edits", "item_id"), ("catchup_offsets", "channel_id"), ("preferred_versions", "group_key")]
            {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE substr({column}, 1, ?1) = ?2"),
                    params![prefix.len() as i64, prefix],
                )?;
            }
            tx.execute(
                "DELETE FROM guide_overrides WHERE substr(channel_id, 1, ?1) = ?2 OR guide_id = ?3",
                params![prefix.len() as i64, prefix, source_id],
            )?;
            for table in ["watchlist", "progress", "history"] {
                tx.execute(&format!("DELETE FROM {table} WHERE source_id = ?1"), [source_id])?;
            }
            tx.execute("DELETE FROM history_titles WHERE item_id NOT IN (SELECT item_id FROM history)", [])?;
            trakt::forget_source_items(&tx, &prefix)?;
            sync::track_again(&tx)?;
            tx.commit()
        })
    }
}

/// Writes progress over what's stored unless what's stored is newer, and
/// says how many rows changed (none when it was newer).
fn upsert_progress(db: &Connection, progress: &PlaybackProgress) -> rusqlite::Result<usize> {
    let position = if progress.position_seconds.is_finite() { progress.position_seconds.max(0.0) } else { 0.0 };
    let duration = if progress.duration_seconds.is_finite() { progress.duration_seconds.max(0.0) } else { 0.0 };
    db.prepare_cached(&format!(
        "INSERT INTO progress ({PROGRESS_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(item_id) DO UPDATE SET position = excluded.position, duration = excluded.duration,
            finished = excluded.finished, updated_at = excluded.updated_at,
            series_id = COALESCE(excluded.series_id, series_id)
         WHERE excluded.updated_at >= progress.updated_at"
    ))?
    .execute(params![
        progress.item_id,
        kind_name(progress.kind),
        progress.source_id,
        progress.series_id,
        position,
        duration,
        progress.finished,
        progress.updated_at
    ])
}

fn upsert_watchlist(db: &Connection, entry: &WatchlistEntry) -> rusqlite::Result<()> {
    db.execute(
        &format!(
            "INSERT INTO watchlist ({WATCHLIST_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(item_id) DO UPDATE SET title = excluded.title, subtitle = excluded.subtitle,
                artwork = COALESCE(excluded.artwork, artwork), channel_id = excluded.channel_id,
                start = excluded.start, end = excluded.end, remind = excluded.remind"
        ),
        params![
            entry.id,
            kind_name(entry.kind),
            entry.source_id,
            entry.title,
            entry.subtitle,
            entry.artwork_url,
            entry.added_at,
            entry.channel_id,
            entry.start,
            entry.end,
            entry.remind
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(store: &LibraryStore) -> i64 {
        store.with(|db| db.pragma_query_value(None, "user_version", |row| row.get(0))).unwrap()
    }

    #[test]
    fn older_libraries_gain_profiles_and_keep_everything_else() {
        for older in [7, 8] {
            an_older_library_gains_profiles(older);
        }
    }

    fn an_older_library_gains_profiles(older: i64) {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("library.sqlite");
        {
            // An older library: everything but the profile tables.
            let db = Connection::open(&path).unwrap();
            db.execute_batch(SCHEMA).unwrap();
            db.execute_batch("DROP TABLE profiles; DROP TABLE home_layouts;").unwrap();
            db.execute(
                "INSERT INTO watchlist (item_id, kind, source_id, title, added_at) VALUES ('s|mv|1', 'movie', 's', 'Paper Moons', 1)",
                [],
            )
            .unwrap();
            db.execute("INSERT INTO item_edits (item_id, display_name, hidden) VALUES ('s|livecat|x', 'Mine', 1)", []).unwrap();
            db.execute("INSERT INTO preferences (key, value) VALUES ('parental_pin', 'v1:aa:bb')", []).unwrap();
            db.execute("INSERT INTO favorites (item_id, kind, sort_index, added_at) VALUES ('s|ch|1', 'channel', 0, 1)", []).unwrap();
            db.pragma_update(None, "user_version", older).unwrap();
        }

        let store = LibraryStore::open(&path).unwrap();
        assert_eq!(version(&store), SCHEMA_VERSION);
        assert_eq!(store.watchlist().unwrap()[0].title, "Paper Moons");
        let edit = &store.item_edits().unwrap()[0];
        assert_eq!((edit.display_name.as_deref(), edit.hidden), (Some("Mine"), true));
        assert_eq!(store.preference("parental_pin").unwrap().as_deref(), Some("v1:aa:bb"));
        assert_eq!(store.list_items(DEFAULT_LIST).unwrap(), ["s|ch|1"]);

        assert!(store.profiles().unwrap().is_empty(), "Everything is implied until a profile is saved");
        store.save_profile("a", "{\"name\":\"A\"}").unwrap();
        store.save_profile("b", "{\"name\":\"B\"}").unwrap();
        store.save_profile("a", "{\"name\":\"A again\"}").unwrap();
        assert_eq!(store.profiles().unwrap(), [("a".into(), "{\"name\":\"A again\"}".into()), ("b".into(), "{\"name\":\"B\"}".into())]);
        store.set_home_layout("a", "sources", Some("{}")).unwrap();
        assert_eq!(store.home_layout("a", "sources").unwrap().as_deref(), Some("{}"));

        // Opening again changes nothing.
        drop(store);
        let store = LibraryStore::open(&path).unwrap();
        assert_eq!(store.profiles().unwrap().len(), 2);
        store.delete_profile("a").unwrap();
        assert_eq!(store.home_layout("a", "sources").unwrap(), None);
        assert_eq!(store.watchlist().unwrap().len(), 1);
    }
}
