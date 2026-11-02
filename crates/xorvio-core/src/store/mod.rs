//! Storage. Provider data (`catalog.sqlite`) can always be rebuilt by
//! refreshing, so it's kept apart from what people make themselves
//! (`library.sqlite`), which survives any catalog rebuild.

pub(crate) mod catalog;
pub(crate) mod config;
pub(crate) mod library;

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::Connection;

/// Opens a connection with the settings every Xorvio database uses: WAL, so
/// readers keep seeing the last committed state during a long write, and a
/// generous wait when another connection is writing.
pub(crate) fn open_connection(path: &Path) -> anyhow::Result<Connection> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(30))?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", "OFF")?;
    Ok(connection)
}

/// A lock that keeps working after a panic elsewhere: the data behind it is a
/// database connection, whose own transactions keep it consistent.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// JSON array text, for binding a list to `json_each(?)`.
pub(crate) fn json_list<S: AsRef<str>>(values: &[S]) -> String {
    serde_json::to_string(&values.iter().map(AsRef::as_ref).collect::<Vec<&str>>()).unwrap_or_else(|_| "[]".into())
}

/// SQL `LIMIT` for an optional count (SQLite reads -1 as "no limit").
pub(crate) fn sql_limit(limit: Option<usize>) -> i64 {
    limit.map_or(-1, |limit| i64::try_from(limit).unwrap_or(i64::MAX))
}

/// A guide channel someone chose for a channel. Kept in the library, so it
/// survives refreshes; the catalog applies it each time it links channels to
/// guides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuideOverride {
    pub channel_id: String,
    /// A source's id, or an added guide's.
    pub guide_id: String,
    pub epg_id: String,
}
