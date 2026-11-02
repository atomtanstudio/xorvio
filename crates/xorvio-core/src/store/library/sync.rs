//! The library's half of syncing between computers through a shared folder
//! (crate::sync has the folder's half). Every synced table keeps a change
//! time per row, and its deletions are kept as tombstones; triggers write
//! both, so no write path in library.rs can forget to. A computer's rows and
//! tombstones (a journal) then merge into another's, newest wins per row.
//!
//! Which tables sync is the list below: a table, the columns that name its
//! rows on every computer, and the column holding when each row changed.
//! Adding a table is a line there; opening the library gives it the column
//! and triggers.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, Transaction, TransactionBehavior, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use xorvio_model::Timestamp;

use super::LibraryStore;

/// A library table that syncs between computers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SyncedTable {
    pub name: &'static str,
    /// The columns that name a row on every computer.
    pub key: &'static [&'static str],
    /// The column holding when the row last changed, in Unix seconds.
    pub changed: &'static str,
    /// The triggers keep `changed` up to date (a `changed_at` column sync
    /// adds), rather than the app (progress's `updated_at`, which already
    /// keeps the newer position).
    pub stamped: bool,
    /// Columns that mean something on this computer only (history's row id).
    pub local_only: &'static [&'static str],
    /// The column naming a row's source. Rows for sources this computer
    /// doesn't have are left out: they couldn't play here, and a source
    /// removed here shouldn't come back with its watchlist.
    pub source: Option<&'static str>,
}

impl SyncedTable {
    const fn new(name: &'static str, key: &'static [&'static str]) -> Self {
        Self { name, key, changed: "changed_at", stamped: true, local_only: &[], source: None }
    }

    const fn changed_by_app(self, column: &'static str) -> Self {
        Self { changed: column, stamped: false, ..self }
    }

    const fn local_only(self, columns: &'static [&'static str]) -> Self {
        Self { local_only: columns, ..self }
    }

    const fn by_source(self, column: &'static str) -> Self {
        Self { source: Some(column), ..self }
    }
}

/// What syncs. A table that isn't in this library yet is passed over, so a
/// table can be listed before the schema that makes it is merged.
pub(crate) const SYNCED_TABLES: &[SyncedTable] = &[
    SyncedTable::new("watchlist", &["item_id"]).by_source("source_id"),
    SyncedTable::new("progress", &["item_id"]).changed_by_app("updated_at").by_source("source_id"),
    SyncedTable::new("history", &["item_id", "watched_at"]).local_only(&["id"]).by_source("source_id"),
    SyncedTable::new("history_titles", &["item_id"]),
    SyncedTable::new("favorites", &["item_id", "list_id"]),
    SyncedTable::new("favorite_lists", &["id"]),
    SyncedTable::new("item_edits", &["item_id"]),
    SyncedTable::new("guide_overrides", &["channel_id"]),
    SyncedTable::new("catchup_offsets", &["channel_id"]),
    SyncedTable::new("preferred_versions", &["group_key"]),
    // Library profiles and their Home layouts (schema 9, merged separately).
    SyncedTable::new("profiles", &["id"]),
    SyncedTable::new("home_layouts", &["profile_id", "mode"]),
];

/// Library tables that stay on this computer: which profile is active and
/// the parental PIN (preferences), Trakt's own record, added guides (their
/// links are in the keychain), reminders already sent, and sync's own
/// bookkeeping. Every table is in this list or the one above; a test checks.
#[cfg_attr(not(test), allow(dead_code, reason = "read by every_library_table_is_synced_or_kept_here"))]
pub(crate) const LOCAL_TABLES: &[&str] = &[
    "preferences",
    "dismissed_shows",
    "reminders_sent",
    "guide_sources",
    "trakt_watched",
    "trakt_playback",
    "trakt_watchlist",
    "trakt_lookups",
    "sync_tombstones",
    "sync_meta",
    "sync_untracked",
];

/// One table of a journal: its rows, each with its change time, and its
/// deletions. Column names travel with the rows, so computers running
/// different versions of Xorvio still line up.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JournalTable {
    pub key: Vec<String>,
    pub changed: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// Deleted rows: their key's values, and when.
    pub deleted: Vec<(Vec<Value>, Timestamp)>,
}

/// A computer's synced library, by table name.
pub(crate) type JournalTables = BTreeMap<String, JournalTable>;

/// What a merge changed.
#[derive(Debug, Default)]
pub(crate) struct MergeReport {
    /// Tables whose rows changed here.
    pub changed_tables: BTreeSet<&'static str>,
    pub rows_changed: usize,
    /// Rows that couldn't be used (from a damaged or very different journal).
    pub skipped: usize,
}

/// Gives every synced table its change-time column and triggers. Runs each
/// time the library opens, so the triggers always match [`SYNCED_TABLES`].
pub(super) fn prepare(db: &mut Connection) -> rusqlite::Result<()> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Only ever set inside a transaction; one left behind would stop all tracking.
    tx.execute("DELETE FROM sync_untracked", [])?;
    let old: Vec<String> = {
        let mut statement = tx.prepare("SELECT name FROM sqlite_master WHERE type = 'trigger' AND name LIKE 'sync!_%' ESCAPE '!'")?;
        statement.query_map([], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    for name in old {
        tx.execute_batch(&format!("DROP TRIGGER IF EXISTS {};", quoted(&name)))?;
    }
    for table in SYNCED_TABLES {
        let columns = table_columns(&tx, table.name)?;
        if columns.is_empty() {
            continue;
        }
        if !columns.iter().any(|column| column == table.changed) {
            if !table.stamped {
                return Err(rusqlite::Error::InvalidColumnName(format!("{}.{}", table.name, table.changed)));
            }
            // Rows from before syncing count as the oldest there are.
            tx.execute_batch(&format!(
                "ALTER TABLE {} ADD COLUMN {} INTEGER NOT NULL DEFAULT 0;",
                quoted(table.name),
                quoted(table.changed)
            ))?;
        }
        tx.execute_batch(&triggers(table))?;
    }
    tx.commit()
}

/// The three triggers that keep a table's change times and tombstones.
fn triggers(table: &SyncedTable) -> String {
    let name = table.name;
    let changed = quoted(table.changed);
    let quiet = "NOT EXISTS (SELECT 1 FROM sync_untracked)";
    let key_of = |row: &str| {
        let values: Vec<String> = table.key.iter().map(|column| format!("{row}.{}", quoted(column))).collect();
        format!("json_array({})", values.join(", "))
    };
    let (old_key, new_key) = (key_of("OLD"), key_of("NEW"));
    let same_row: Vec<String> = table.key.iter().map(|column| format!("{0} IS NEW.{0}", quoted(column))).collect();
    let stamp = if table.stamped {
        format!("UPDATE {} SET {changed} = unixepoch() WHERE {};", quoted(name), same_row.join(" AND "))
    } else {
        String::new()
    };
    let unbury = format!("DELETE FROM sync_tombstones WHERE table_name = '{name}' AND row_key = {new_key};");
    let counted = "UPDATE sync_meta SET local_changes = local_changes + 1 WHERE id = 1;";
    // An update the app makes leaves the change time alone; the stamp
    // itself changes it, and isn't counted again.
    let updated_when = if table.stamped { format!("{quiet} AND NEW.{changed} IS OLD.{changed}") } else { quiet.to_string() };
    format!(
        "CREATE TRIGGER {inserted} AFTER INSERT ON {table} WHEN {quiet} BEGIN
            {stamp} {unbury} {counted}
         END;
         CREATE TRIGGER {updated} AFTER UPDATE ON {table} WHEN {updated_when} BEGIN
            INSERT INTO sync_tombstones (table_name, row_key, deleted_at)
                SELECT '{name}', {old_key}, unixepoch() WHERE {old_key} IS NOT {new_key}
                ON CONFLICT (table_name, row_key) DO UPDATE SET deleted_at = excluded.deleted_at;
            {stamp} {unbury} {counted}
         END;
         CREATE TRIGGER {deleted} AFTER DELETE ON {table} WHEN {quiet} BEGIN
            INSERT INTO sync_tombstones (table_name, row_key, deleted_at) VALUES ('{name}', {old_key}, unixepoch())
                ON CONFLICT (table_name, row_key) DO UPDATE SET deleted_at = excluded.deleted_at;
            {counted}
         END;",
        table = quoted(name),
        inserted = quoted(&format!("sync_{name}_inserted")),
        updated = quoted(&format!("sync_{name}_updated")),
        deleted = quoted(&format!("sync_{name}_deleted")),
    )
}

/// Keeps the triggers out of the rest of this transaction: what follows
/// isn't this computer's own change (a merge, or removing a source, which
/// isn't synced). [`track_again`] ends it before the commit.
pub(super) fn untracked(tx: &Connection) -> rusqlite::Result<()> {
    tx.execute("INSERT INTO sync_untracked (busy) VALUES (1)", []).map(|_| ())
}

pub(super) fn track_again(tx: &Connection) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM sync_untracked", []).map(|_| ())
}

impl LibraryStore {
    /// How many synced rows this computer has changed itself, ever. It moves
    /// when there's something new to tell the other computers.
    pub fn sync_changes(&self) -> anyhow::Result<i64> {
        self.with(|db| db.query_row("SELECT local_changes FROM sync_meta WHERE id = 1", [], |row| row.get(0)))
    }

    /// Every synced row and tombstone, in a steady order (so an unchanged
    /// library writes the same journal).
    pub fn sync_export(&self) -> anyhow::Result<JournalTables> {
        self.with(|db| {
            // One read transaction, so the tables agree with each other.
            let tx = db.transaction()?;
            let mut tables = JournalTables::new();
            for table in SYNCED_TABLES {
                let columns = synced_columns(&tx, table)?;
                if columns.is_empty() {
                    continue;
                }
                let names: Vec<String> = columns.iter().map(|column| quoted(column)).collect();
                let order: Vec<String> = table.key.iter().map(|column| quoted(column)).collect();
                let rows = {
                    let mut statement =
                        tx.prepare(&format!("SELECT {} FROM {} ORDER BY {}", names.join(", "), quoted(table.name), order.join(", ")))?;
                    statement
                        .query_map([], |row| (0..columns.len()).map(|index| row.get_ref(index).map(json_value)).collect())?
                        .collect::<rusqlite::Result<Vec<Vec<Value>>>>()?
                };
                let deleted = {
                    let mut statement =
                        tx.prepare("SELECT row_key, deleted_at FROM sync_tombstones WHERE table_name = ?1 ORDER BY row_key")?;
                    statement
                        .query_map([table.name], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
                        .filter_map(|row| match row {
                            Ok((key, at)) => serde_json::from_str::<Vec<Value>>(&key).ok().map(|key| Ok((key, at))),
                            Err(error) => Some(Err(error)),
                        })
                        .collect::<rusqlite::Result<Vec<_>>>()?
                };
                tables.insert(
                    table.name.to_string(),
                    JournalTable {
                        key: table.key.iter().map(|column| column.to_string()).collect(),
                        changed: table.changed.to_string(),
                        columns,
                        rows,
                        deleted,
                    },
                );
            }
            tx.commit()?;
            Ok(tables)
        })
    }

    /// Merges other computers' journals into this library, newest wins per
    /// row: a row replaces this one when it changed later, a tombstone
    /// deletes it when the deletion was later. A row and a tombstone from
    /// the same second keep the row, and two rows from the same second are
    /// settled by their contents, so every computer ends up the same.
    pub fn sync_merge(&self, journals: &[&JournalTables], known_sources: &HashSet<String>) -> anyhow::Result<MergeReport> {
        self.with(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            untracked(&tx)?;
            let mut report = MergeReport::default();
            for table in SYNCED_TABLES {
                let columns = synced_columns(&tx, table)?;
                if columns.is_empty() {
                    continue;
                }
                let mut current = current_state(&tx, table, &columns)?;
                for journal in journals {
                    if let Some(theirs) = journal.get(table.name) {
                        merge_table(&tx, table, &columns, &mut current, theirs, known_sources, &mut report)?;
                    }
                }
            }
            track_again(&tx)?;
            tx.commit()?;
            Ok(report)
        })
    }

    /// Forgets deletions older than `before`. A computer away longer than
    /// that could bring back a row deleted meanwhile; keeping them forever
    /// would grow every journal without end.
    pub fn prune_tombstones(&self, before: Timestamp) -> anyhow::Result<usize> {
        self.with(|db| db.execute("DELETE FROM sync_tombstones WHERE deleted_at < ?1", [before]))
    }

    /// Moves every change time back by `seconds`, as if this computer's
    /// changes were made that long ago.
    #[cfg(test)]
    pub fn age_changes_for_test(&self, seconds: i64) -> anyhow::Result<()> {
        self.with(|db| {
            let tx = db.transaction()?;
            untracked(&tx)?;
            for table in SYNCED_TABLES {
                if table_columns(&tx, table.name)?.is_empty() {
                    continue;
                }
                tx.execute(&format!("UPDATE {0} SET {1} = {1} - ?1", quoted(table.name), quoted(table.changed)), [seconds])?;
            }
            tx.execute("UPDATE sync_tombstones SET deleted_at = deleted_at - ?1", [seconds])?;
            track_again(&tx)?;
            tx.commit()
        })
    }

    #[cfg(test)]
    pub fn tombstone_keys_for_test(&self, table: &str) -> anyhow::Result<Vec<String>> {
        self.with(|db| {
            let mut statement = db.prepare("SELECT row_key FROM sync_tombstones WHERE table_name = ?1 ORDER BY row_key")?;
            statement.query_map([table], |row| row.get(0))?.collect()
        })
    }

    #[cfg(test)]
    pub fn execute_for_test(&self, sql: &str) -> anyhow::Result<()> {
        self.with(|db| db.execute_batch(sql))
    }

    #[cfg(test)]
    pub fn count_for_test(&self, sql: &str) -> anyhow::Result<i64> {
        self.with(|db| db.query_row(sql, [], |row| row.get(0)))
    }
}

/// What this computer has for a row: the row, with when it changed and its
/// contents (to settle a tie), or when it was deleted.
enum Current {
    Row { at: Timestamp, content: String },
    Deleted { at: Timestamp },
}

/// This library's rows and tombstones of one table, by key.
fn current_state(tx: &Transaction<'_>, table: &SyncedTable, columns: &[String]) -> rusqlite::Result<HashMap<String, Current>> {
    let names: Vec<String> = columns.iter().map(|column| quoted(column)).collect();
    let key_at: Vec<usize> = table.key.iter().filter_map(|key| columns.iter().position(|column| column == key)).collect();
    let changed_at = columns.iter().position(|column| column == table.changed);
    let mut current = HashMap::new();
    {
        let mut statement = tx.prepare(&format!("SELECT {} FROM {}", names.join(", "), quoted(table.name)))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let values: Vec<Value> = (0..columns.len()).map(|index| row.get_ref(index).map(json_value)).collect::<rusqlite::Result<_>>()?;
            let key: Vec<&Value> = key_at.iter().map(|&index| &values[index]).collect();
            let at = changed_at.and_then(|index| values[index].as_i64()).unwrap_or(0);
            let content = content_of(columns.iter().map(String::as_str).zip(values.iter()));
            current.insert(key_text(&key), Current::Row { at, content });
        }
    }
    let mut statement = tx.prepare("SELECT row_key, deleted_at FROM sync_tombstones WHERE table_name = ?1")?;
    let mut rows = statement.query([table.name])?;
    while let Some(row) = rows.next()? {
        let (text, at): (String, Timestamp) = (row.get(0)?, row.get(1)?);
        let Ok(key) = serde_json::from_str::<Vec<Value>>(&text) else { continue };
        let key: Vec<&Value> = key.iter().collect();
        // A row that's here wins over a stale tombstone for it.
        current.entry(key_text(&key)).or_insert(Current::Deleted { at });
    }
    Ok(current)
}

fn merge_table(
    tx: &Transaction<'_>,
    table: &SyncedTable,
    columns: &[String],
    current: &mut HashMap<String, Current>,
    theirs: &JournalTable,
    known_sources: &HashSet<String>,
    report: &mut MergeReport,
) -> rusqlite::Result<()> {
    // A key or change time that differs means a version of Xorvio this one
    // can't line up with; its rows are left for when both are updated.
    let position = |name: &str| theirs.columns.iter().position(|column| column == name);
    let key_at: Option<Vec<usize>> = table.key.iter().map(|key| position(key)).collect();
    let (true, Some(key_at), Some(changed_at)) = (theirs.key == table.key && theirs.changed == table.changed, key_at, position(table.changed))
    else {
        report.skipped += theirs.rows.len() + theirs.deleted.len();
        return Ok(());
    };
    let source_at = match table.source {
        Some(column) => match position(column) {
            Some(index) => Some(index),
            None => {
                report.skipped += theirs.rows.len();
                None
            }
        },
        None => None,
    };
    let rows_usable = table.source.is_none() || source_at.is_some();
    // Their columns this library has too, in their order.
    let shared: Vec<(usize, &str)> = theirs
        .columns
        .iter()
        .enumerate()
        .filter(|(_, column)| columns.contains(column))
        .map(|(index, column)| (index, column.as_str()))
        .collect();
    let key_params: Vec<String> = (1..=table.key.len()).map(|index| format!("?{index}")).collect();
    let by_key: Vec<String> = table.key.iter().zip(&key_params).map(|(column, param)| format!("{} IS {param}", quoted(column))).collect();
    let delete_row = format!("DELETE FROM {} WHERE {}", quoted(table.name), by_key.join(" AND "));
    let insert_row = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quoted(table.name),
        shared.iter().map(|(_, column)| quoted(column)).collect::<Vec<_>>().join(", "),
        (1..=shared.len()).map(|index| format!("?{index}")).collect::<Vec<_>>().join(", ")
    );
    let bury = format!(
        "INSERT INTO sync_tombstones (table_name, row_key, deleted_at) VALUES ('{}', json_array({}), ?{})
         ON CONFLICT (table_name, row_key) DO UPDATE SET deleted_at = MAX(deleted_at, excluded.deleted_at)",
        table.name,
        key_params.join(", "),
        table.key.len() + 1
    );
    let unbury = format!("DELETE FROM sync_tombstones WHERE table_name = '{}' AND row_key = json_array({})", table.name, key_params.join(", "));

    for row in theirs.rows.iter().filter(|_| rows_usable) {
        if row.len() != theirs.columns.len() {
            report.skipped += 1;
            continue;
        }
        let key: Vec<&Value> = key_at.iter().map(|&index| &row[index]).collect();
        if key.iter().any(|value| value.is_null()) {
            report.skipped += 1;
            continue;
        }
        if let Some(index) = source_at
            && !row[index].as_str().is_some_and(|source| known_sources.contains(source))
        {
            continue;
        }
        let at = row[changed_at].as_i64().unwrap_or(0);
        let content = content_of(shared.iter().map(|&(index, column)| (column, &row[index])));
        let text = key_text(&key);
        let newer = match current.get(&text) {
            None => true,
            Some(Current::Deleted { at: deleted }) => at >= *deleted,
            Some(Current::Row { at: local, content: mine }) => at > *local || (at == *local && content > *mine),
        };
        if !newer {
            continue;
        }
        let key_values: Vec<SqlValue> = key.iter().map(|value| sql_value(value)).collect();
        // Each row on its own, so one that doesn't fit (a damaged journal)
        // leaves this computer's row as it was.
        tx.execute_batch("SAVEPOINT sync_row")?;
        let written = tx
            .prepare_cached(&delete_row)
            .and_then(|mut statement| statement.execute(params_from_iter(&key_values)))
            .and_then(|_| tx.prepare_cached(&insert_row))
            .and_then(|mut statement| statement.execute(params_from_iter(shared.iter().map(|&(index, _)| sql_value(&row[index])))));
        match written {
            Ok(_) => tx.execute_batch("RELEASE sync_row")?,
            Err(error) => {
                tracing::debug!("a synced {} row couldn't be used: {error}", table.name);
                tx.execute_batch("ROLLBACK TO sync_row; RELEASE sync_row")?;
                report.skipped += 1;
                continue;
            }
        }
        tx.prepare_cached(&unbury)?.execute(params_from_iter(&key_values))?;
        current.insert(text, Current::Row { at, content });
        report.changed_tables.insert(table.name);
        report.rows_changed += 1;
    }

    for (key, at) in &theirs.deleted {
        if key.len() != table.key.len() || key.iter().any(Value::is_null) {
            report.skipped += 1;
            continue;
        }
        let refs: Vec<&Value> = key.iter().collect();
        let text = key_text(&refs);
        let (delete, record) = match current.get(&text) {
            Some(Current::Row { at: changed, .. }) => (*at > *changed, *at > *changed),
            Some(Current::Deleted { at: deleted }) => (false, *at > *deleted),
            // Kept even with no row here, so the deletion travels on to
            // computers that only see this one's journal later.
            None => (false, true),
        };
        if !record {
            continue;
        }
        let key_values: Vec<SqlValue> = key.iter().map(sql_value).collect();
        if delete {
            tx.prepare_cached(&delete_row)?.execute(params_from_iter(&key_values))?;
            report.changed_tables.insert(table.name);
            report.rows_changed += 1;
        }
        let mut values = key_values;
        values.push(SqlValue::Integer(*at));
        tx.prepare_cached(&bury)?.execute(params_from_iter(&values))?;
        current.insert(text, Current::Deleted { at: *at });
    }
    Ok(())
}

/// A table's columns, in order; none when the table isn't there.
fn table_columns(db: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = db.prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")?;
    statement.query_map([table], |row| row.get(0))?.collect()
}

/// The columns that sync: all but the ones that only mean something here.
fn synced_columns(db: &Connection, table: &SyncedTable) -> rusqlite::Result<Vec<String>> {
    Ok(table_columns(db, table.name)?.into_iter().filter(|column| !table.local_only.contains(&column.as_str())).collect())
}

/// A key as text, the same however its row arrived.
fn key_text(key: &[&Value]) -> String {
    serde_json::to_string(key).unwrap_or_default()
}

/// A row's contents as text with its columns in name order, for settling
/// two changes made in the same second the same way on every computer.
fn content_of<'a>(values: impl Iterator<Item = (&'a str, &'a Value)>) -> String {
    serde_json::to_string(&values.collect::<BTreeMap<&str, &Value>>()).unwrap_or_default()
}

fn json_value(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(number) => number.into(),
        ValueRef::Real(number) => serde_json::Number::from_f64(number).map_or(Value::Null, Value::Number),
        ValueRef::Text(text) => Value::String(String::from_utf8_lossy(text).into_owned()),
        // None of the synced tables hold any.
        ValueRef::Blob(_) => Value::Null,
    }
}

fn sql_value(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Bool(flag) => SqlValue::Integer(i64::from(*flag)),
        Value::Number(number) => number.as_i64().map(SqlValue::Integer).or_else(|| number.as_f64().map(SqlValue::Real)).unwrap_or(SqlValue::Null),
        Value::String(text) => SqlValue::Text(text.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

/// An identifier for SQL. The names come from the lists above and SQLite
/// itself, never from a journal.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Whether a library table syncs, for tests and the docs' table list.
#[cfg(test)]
pub(crate) fn is_synced(table: &str) -> bool {
    SYNCED_TABLES.iter().any(|synced| synced.name == table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xorvio_model::{GuideSource, MediaKind, PlaybackProgress, WatchlistEntry};

    use crate::store::GuideOverride;

    fn open(folder: &tempfile::TempDir, name: &str) -> LibraryStore {
        LibraryStore::open(&folder.path().join(name)).unwrap()
    }

    fn entry(id: &str, source: &str, title: &str) -> WatchlistEntry {
        WatchlistEntry {
            id: id.into(),
            kind: MediaKind::Movie,
            source_id: source.into(),
            title: title.into(),
            subtitle: None,
            artwork_url: None,
            added_at: 100,
            channel_id: None,
            start: None,
            end: None,
            remind: false,
        }
    }

    fn progress(id: &str, position: f64, updated_at: Timestamp) -> PlaybackProgress {
        PlaybackProgress {
            item_id: id.into(),
            kind: MediaKind::Movie,
            source_id: "s".into(),
            series_id: None,
            position_seconds: position,
            duration_seconds: 3600.0,
            finished: false,
            updated_at,
        }
    }

    fn sources() -> HashSet<String> {
        ["s".to_string(), "addon".to_string()].into()
    }

    /// The library as it was at schema 8, before syncing.
    fn eighth_schema() -> &'static str {
        super::super::SCHEMA.split("-- Syncing between computers").next().unwrap()
    }

    #[test]
    fn every_library_table_is_synced_or_kept_here() {
        let folder = tempfile::tempdir().unwrap();
        let store = open(&folder, "library.sqlite");
        let tables: Vec<String> = store
            .with(|db| {
                let mut statement = db.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite!_%' ESCAPE '!'")?;
                statement.query_map([], |row| row.get(0))?.collect()
            })
            .unwrap();
        for table in tables {
            assert!(
                is_synced(&table) || LOCAL_TABLES.contains(&table.as_str()),
                "{table} is in neither SYNCED_TABLES nor LOCAL_TABLES; decide whether it syncs"
            );
        }
    }

    #[test]
    fn every_synced_table_here_has_its_column_and_triggers() {
        let folder = tempfile::tempdir().unwrap();
        let store = open(&folder, "library.sqlite");
        for table in SYNCED_TABLES {
            let columns = store.with(|db| table_columns(db, table.name)).unwrap();
            if columns.is_empty() {
                continue;
            }
            assert!(columns.iter().any(|column| column == table.changed), "{} has no {}", table.name, table.changed);
            for column in table.key {
                assert!(columns.iter().any(|name| name == column), "{} has no key column {column}", table.name);
            }
            for event in ["inserted", "updated", "deleted"] {
                let name = format!("sync_{}_{event}", table.name);
                let found = store.count_for_test(&format!("SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = '{name}'")).unwrap();
                assert_eq!(found, 1, "{name} is missing");
            }
        }
    }

    /// A synced table's key is what names its rows in the schema too, so two
    /// rows (two profiles' progress, say) can never be taken for one.
    #[test]
    fn every_synced_key_is_the_table_s_own() {
        let folder = tempfile::tempdir().unwrap();
        let store = open(&folder, "library.sqlite");
        for table in SYNCED_TABLES {
            let primary: Vec<String> = store
                .with(|db| {
                    let mut statement = db.prepare("SELECT name FROM pragma_table_info(?1) WHERE pk > 0 ORDER BY pk")?;
                    statement.query_map([table.name], |row| row.get(0))?.collect()
                })
                .unwrap();
            if primary.is_empty() {
                continue;
            }
            if primary.iter().all(|column| table.local_only.contains(&column.as_str())) {
                // Keyed here by a row id (history): the sync key needs an index of its own.
                let indexed = store
                    .count_for_test(&format!(
                        "SELECT COUNT(*) FROM pragma_index_list('{0}') l WHERE (SELECT group_concat(name) FROM (SELECT name FROM pragma_index_info(l.name) ORDER BY seqno)) = '{1}'",
                        table.name,
                        table.key.join(",")
                    ))
                    .unwrap();
                assert_eq!(indexed, 1, "{} has no index on its sync key", table.name);
                continue;
            }
            assert_eq!(primary, table.key, "{}'s sync key isn't its primary key", table.name);
        }
    }

    /// Whatever deletes a synced row, even plain SQL no method wraps, leaves
    /// a tombstone, and putting the row back clears it.
    #[test]
    fn any_delete_of_a_synced_row_leaves_a_tombstone() {
        let folder = tempfile::tempdir().unwrap();
        let store = open(&folder, "library.sqlite");
        for table in SYNCED_TABLES {
            let columns = store.with(|db| table_columns(db, table.name)).unwrap();
            if columns.is_empty() {
                continue;
            }
            // A row with every column that has no default filled in.
            let required: Vec<(String, String)> = store
                .with(|db| {
                    let mut statement = db.prepare("SELECT name, type FROM pragma_table_info(?1) WHERE \"notnull\" = 1 AND dflt_value IS NULL AND pk = 0 OR (pk > 0 AND type = 'TEXT')")?;
                    statement.query_map([table.name], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
                })
                .unwrap();
            let mut names: Vec<String> = required.iter().map(|(name, _)| quoted(name)).collect();
            let mut values: Vec<String> = required.iter().map(|(_, kind)| if kind == "TEXT" { "'s|x'".into() } else { "7".into() }).collect();
            for key in table.key {
                if !required.iter().any(|(name, _)| name == key) {
                    names.push(quoted(key));
                    values.push("7".into());
                }
            }
            store.execute_for_test(&format!("INSERT INTO {} ({}) VALUES ({})", quoted(table.name), names.join(", "), values.join(", "))).unwrap();
            assert!(store.tombstone_keys_for_test(table.name).unwrap().is_empty());
            store.execute_for_test(&format!("DELETE FROM {}", quoted(table.name))).unwrap();
            assert_eq!(store.tombstone_keys_for_test(table.name).unwrap().len(), 1, "deleting from {} left no tombstone", table.name);
            store.execute_for_test(&format!("INSERT INTO {} ({}) VALUES ({})", quoted(table.name), names.join(", "), values.join(", "))).unwrap();
            assert!(store.tombstone_keys_for_test(table.name).unwrap().is_empty(), "{} kept a tombstone for a row that's back", table.name);
        }
    }

    /// Each way library.rs deletes synced rows, one by one.
    #[test]
    fn the_library_s_deletions_all_leave_tombstones() {
        let folder = tempfile::tempdir().unwrap();
        let store = open(&folder, "library.sqlite");
        let buried = |table: &str| store.tombstone_keys_for_test(table).unwrap();

        store.add_to_watchlist(&entry("s|mv|1", "s", "One")).unwrap();
        store.remove_from_watchlist("s|mv|1").unwrap();
        assert_eq!(buried("watchlist"), [r#"["s|mv|1"]"#]);
        store.add_to_watchlist(&entry("s|pg|1", "s", "Game")).unwrap();
        store.replace_watchlist_entry("s|pg|1", &entry("s|pg|2", "s", "Game")).unwrap();
        assert!(buried("watchlist").contains(&r#"["s|pg|1"]"#.to_string()));
        let mut ended = entry("s|pg|3", "s", "Ended");
        (ended.kind, ended.end) = (MediaKind::Program, Some(50));
        store.add_to_watchlist(&ended).unwrap();
        store.remove_ended_programs(100).unwrap();
        assert!(buried("watchlist").contains(&r#"["s|pg|3"]"#.to_string()));

        store.set_in_list("s|ch|1", MediaKind::Channel, "default", true, 1).unwrap();
        store.set_in_list("s|ch|1", MediaKind::Channel, "default", false, 1).unwrap();
        assert_eq!(buried("favorites"), [r#"["s|ch|1","default"]"#]);
        store.create_list("list", "Sport", 1).unwrap();
        store.set_in_list("s|ch|2", MediaKind::Channel, "list", true, 1).unwrap();
        store.delete_list("list").unwrap();
        assert_eq!(buried("favorite_lists"), [r#"["list"]"#]);
        assert!(buried("favorites").contains(&r#"["s|ch|2","list"]"#.to_string()));

        store.save_progress(&progress("s|mv|1", 100.0, 10)).unwrap();
        store.remove_progress("s|mv|1").unwrap();
        assert_eq!(buried("progress"), [r#"["s|mv|1"]"#]);

        store.edit_item("s|ch|1", |row| row.display_name = Some("Mine".into())).unwrap();
        store.edit_item("s|ch|1", |row| row.display_name = None).unwrap();
        assert_eq!(buried("item_edits"), [r#"["s|ch|1"]"#]);
        store.edit_item("s|ch|2", |row| row.hidden = true).unwrap();
        store.show_all_hidden().unwrap();
        assert!(buried("item_edits").contains(&r#"["s|ch|2"]"#.to_string()));

        store.set_catchup_offset("s|ch|1", 5).unwrap();
        store.set_catchup_offset("s|ch|1", 0).unwrap();
        assert_eq!(buried("catchup_offsets"), [r#"["s|ch|1"]"#]);
        store.set_preferred_version("s|group", Some("s|ch|1")).unwrap();
        store.set_preferred_version("s|group", None).unwrap();
        assert_eq!(buried("preferred_versions"), [r#"["s|group"]"#]);

        let choice = GuideOverride { channel_id: "s|ch|1".into(), guide_id: "s".into(), epg_id: "one".into() };
        store.set_guide_override(&choice, 1).unwrap();
        store.remove_guide_override("s|ch|1").unwrap();
        assert_eq!(buried("guide_overrides"), [r#"["s|ch|1"]"#]);

        store.add_history("s|ch|1", MediaKind::Channel, "s", 1000).unwrap();
        store.add_history("s|ch|1", MediaKind::Channel, "s", 1500).unwrap();
        assert_eq!(buried("history"), [r#"["s|ch|1",1000]"#], "a repeat within the hour replaces the first");
        store.note_history_title("s|ch|1", "One", None).unwrap();
        store.clear_history().unwrap();
        assert_eq!(buried("history").len(), 2);
        assert_eq!(buried("history_titles"), [r#"["s|ch|1"]"#]);
    }

    /// A removed source or guide isn't synced, so tidying away what was tied
    /// to it here mustn't delete it on the other computers.
    #[test]
    fn removing_a_source_or_guide_leaves_no_tombstones() {
        let folder = tempfile::tempdir().unwrap();
        let store = open(&folder, "library.sqlite");
        store.add_to_watchlist(&entry("s|mv|1", "s", "One")).unwrap();
        store.save_progress(&progress("s|mv|1", 100.0, 10)).unwrap();
        store.set_in_list("s|ch|1", MediaKind::Channel, "default", true, 1).unwrap();
        let guide = GuideSource { id: "guide-1".into(), name: "Guide".into(), display_host: None, offset_minutes: 0, created_at: 1, sort_index: 0 };
        store.save_guide_source(&guide).unwrap();
        store.set_guide_override(&GuideOverride { channel_id: "t|ch|1".into(), guide_id: "guide-1".into(), epg_id: "e".into() }, 1).unwrap();
        let before = store.sync_changes().unwrap();
        store.remove_source("s").unwrap();
        store.remove_guide_source("guide-1").unwrap();
        assert_eq!(store.count_for_test("SELECT COUNT(*) FROM sync_tombstones").unwrap(), 0);
        assert_eq!(store.sync_changes().unwrap(), before, "not counted as changes to send");
        assert_eq!(store.count_for_test("SELECT COUNT(*) FROM watchlist").unwrap(), 0, "still removed here");
        store.add_to_watchlist(&entry("s|mv|2", "s", "Two")).unwrap();
        assert!(store.sync_changes().unwrap() > before, "tracking carries on afterwards");
    }

    #[test]
    fn changes_are_stamped_and_counted() {
        let folder = tempfile::tempdir().unwrap();
        let store = open(&folder, "library.sqlite");
        let start = store.sync_changes().unwrap();
        store.add_to_watchlist(&entry("s|mv|1", "s", "One")).unwrap();
        let stamped = store.count_for_test("SELECT changed_at FROM watchlist").unwrap();
        assert!((crate::now() - stamped).abs() < 5, "stamped with the time it changed");
        assert!(store.sync_changes().unwrap() > start);
        store.age_changes_for_test(1000).unwrap();
        store.add_to_watchlist(&entry("s|mv|1", "s", "One again")).unwrap();
        assert!((crate::now() - store.count_for_test("SELECT changed_at FROM watchlist").unwrap()).abs() < 5, "an update stamps it again");
        store.save_progress(&progress("s|mv|1", 100.0, 42)).unwrap();
        assert_eq!(store.count_for_test("SELECT updated_at FROM progress").unwrap(), 42, "progress keeps its own time");
    }

    /// From the schema-8 library with something in every table: every row
    /// stays, and syncing works from there.
    #[test]
    fn a_schema_8_library_keeps_every_row() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("library.sqlite");
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch(eighth_schema()).unwrap();
            db.execute_batch(
                "INSERT INTO watchlist (item_id, kind, source_id, title, added_at, remind) VALUES ('s|mv|1', 'movie', 's', 'One', 5, 0);
                 INSERT INTO favorites (item_id, kind, list_id, sort_index, added_at) VALUES ('s|ch|1', 'channel', 'default', 0, 5);
                 INSERT INTO favorite_lists (id, name, sort_index, created_at) VALUES ('list', 'Sport', 0, 5);
                 INSERT INTO progress (item_id, kind, source_id, position, duration, finished, updated_at) VALUES ('s|mv|1', 'movie', 's', 60.5, 3600, 0, 9);
                 INSERT INTO history (item_id, kind, source_id, watched_at) VALUES ('s|mv|1', 'movie', 's', 9);
                 INSERT INTO history_titles (item_id, title) VALUES ('s|mv|1', 'One');
                 INSERT INTO dismissed_shows (show_id, dismissed_at) VALUES ('s|sr|1', 5);
                 INSERT INTO reminders_sent (item_id, sent_at) VALUES ('s|pg|1', 5);
                 INSERT INTO guide_sources (id, name, created_at, sort_index) VALUES ('guide-1', 'Guide', 5, 0);
                 INSERT INTO guide_overrides (channel_id, guide_id, epg_id, set_at) VALUES ('s|ch|1', 'guide-1', 'one', 5);
                 INSERT INTO item_edits (item_id, display_name, hidden, locked) VALUES ('s|ch|1', 'Mine', 1, 1);
                 INSERT INTO catchup_offsets (channel_id, minutes) VALUES ('s|ch|1', 5);
                 INSERT INTO preferred_versions (group_key, channel_id) VALUES ('s|group', 's|ch|1');
                 INSERT INTO preferences (key, value) VALUES ('parental_pin', 'v1:aa:bb');
                 INSERT INTO trakt_lookups (kind, title_key, year, looked_up_at) VALUES ('movie', 'one', 2020, 5);",
            )
            .unwrap();
            db.pragma_update(None, "user_version", 8).unwrap();
            assert!(db.prepare("SELECT 1 FROM sync_tombstones").is_err(), "the test starts from the old tables");
        }
        let counts = |store: &LibraryStore| -> Vec<i64> {
            [
                "watchlist", "favorites", "favorite_lists", "progress", "history", "history_titles", "dismissed_shows", "reminders_sent",
                "guide_sources", "guide_overrides", "item_edits", "catchup_offsets", "preferred_versions", "preferences", "trakt_lookups",
            ]
            .iter()
            .map(|table| store.count_for_test(&format!("SELECT COUNT(*) FROM {table}")).unwrap())
            .collect()
        };
        let store = LibraryStore::open(&path).unwrap();
        assert_eq!(counts(&store), vec![1; 15], "every row is still there");
        let version: i64 = store.with(|db| db.pragma_query_value(None, "user_version", |row| row.get(0))).unwrap();
        assert_eq!(version, super::super::SCHEMA_VERSION);
        assert_eq!(store.watchlist().unwrap()[0].title, "One");
        assert_eq!(store.progress("s|mv|1").unwrap().unwrap().position_seconds, 60.5);
        assert_eq!(store.item_edits().unwrap()[0].display_name.as_deref(), Some("Mine"));
        assert_eq!(store.preference("parental_pin").unwrap().as_deref(), Some("v1:aa:bb"));
        assert_eq!(store.count_for_test("SELECT changed_at FROM item_edits").unwrap(), 0, "old rows count as the oldest");
        assert_eq!(store.sync_changes().unwrap(), 0, "migrating isn't a change");

        drop(store);
        let store = LibraryStore::open(&path).unwrap();
        assert_eq!(counts(&store), vec![1; 15], "opening again changes nothing");
        store.remove_from_watchlist("s|mv|1").unwrap();
        assert_eq!(store.tombstone_keys_for_test("watchlist").unwrap(), [r#"["s|mv|1"]"#]);
    }

    #[test]
    fn journals_merge_newest_wins() {
        let folder = tempfile::tempdir().unwrap();
        let (here, there) = (open(&folder, "here.sqlite"), open(&folder, "there.sqlite"));
        here.add_to_watchlist(&entry("s|mv|1", "s", "Here's name")).unwrap();
        here.add_to_watchlist(&entry("s|mv|2", "s", "Deleted later")).unwrap();
        here.age_changes_for_test(100).unwrap();
        there.add_to_watchlist(&entry("s|mv|1", "s", "There's newer name")).unwrap();
        there.add_to_watchlist(&entry("s|mv|3", "s", "Only there")).unwrap();
        there.add_to_watchlist(&entry("other|mv|1", "other", "Another computer's source")).unwrap();
        there.add_to_watchlist(&entry("s|mv|2", "s", "Deleted later")).unwrap();
        there.remove_from_watchlist("s|mv|2").unwrap();

        let report = here.sync_merge(&[&there.sync_export().unwrap()], &sources()).unwrap();
        assert!(report.changed_tables.contains("watchlist"));
        let titles: Vec<String> = here.watchlist().unwrap().into_iter().map(|entry| entry.title).collect();
        assert_eq!(titles.len(), 2, "{titles:?}");
        assert!(titles.contains(&"There's newer name".to_string()) && titles.contains(&"Only there".to_string()));
        assert!(!here.is_watchlisted("other|mv|1").unwrap(), "a source this computer hasn't is left out");
        assert_eq!(here.tombstone_keys_for_test("watchlist").unwrap(), [r#"["s|mv|2"]"#], "the deletion is kept to pass on");
        assert_eq!(here.sync_changes().unwrap(), 2, "a merge isn't counted as this computer's changes");

        // Merging again, either way, changes nothing.
        assert_eq!(here.sync_merge(&[&there.sync_export().unwrap()], &sources()).unwrap().rows_changed, 0);
        let back = there.sync_merge(&[&here.sync_export().unwrap()], &sources()).unwrap();
        assert_eq!(back.rows_changed, 0, "{back:?}");
        assert_eq!(here.sync_export().unwrap()["watchlist"].rows, {
            let mut theirs = there.sync_export().unwrap()["watchlist"].rows.clone();
            theirs.retain(|row| row[0] != "other|mv|1");
            theirs
        });
    }

    #[test]
    fn an_older_tombstone_never_deletes_a_newer_row() {
        let folder = tempfile::tempdir().unwrap();
        let (here, there) = (open(&folder, "here.sqlite"), open(&folder, "there.sqlite"));
        there.add_to_watchlist(&entry("s|mv|1", "s", "One")).unwrap();
        there.remove_from_watchlist("s|mv|1").unwrap();
        there.age_changes_for_test(100).unwrap();
        here.add_to_watchlist(&entry("s|mv|1", "s", "Added again since")).unwrap();
        here.sync_merge(&[&there.sync_export().unwrap()], &sources()).unwrap();
        assert!(here.is_watchlisted("s|mv|1").unwrap());
        there.sync_merge(&[&here.sync_export().unwrap()], &sources()).unwrap();
        assert!(there.is_watchlisted("s|mv|1").unwrap(), "and the other computer gets it back");
        assert!(there.tombstone_keys_for_test("watchlist").unwrap().is_empty());
    }

    #[test]
    fn progress_merges_by_its_own_time() {
        let folder = tempfile::tempdir().unwrap();
        let (here, there) = (open(&folder, "here.sqlite"), open(&folder, "there.sqlite"));
        here.save_progress(&progress("s|mv|1", 900.0, 2000)).unwrap();
        // Written later, but about an earlier position (a media server's).
        there.save_progress(&progress("s|mv|1", 100.0, 1000)).unwrap();
        here.sync_merge(&[&there.sync_export().unwrap()], &sources()).unwrap();
        there.sync_merge(&[&here.sync_export().unwrap()], &sources()).unwrap();
        assert_eq!(here.progress("s|mv|1").unwrap().unwrap().position_seconds, 900.0);
        assert_eq!(there.progress("s|mv|1").unwrap().unwrap().position_seconds, 900.0);
    }

    #[test]
    fn history_rows_merge_by_item_and_time() {
        let folder = tempfile::tempdir().unwrap();
        let (here, there) = (open(&folder, "here.sqlite"), open(&folder, "there.sqlite"));
        here.add_history("s|mv|1", MediaKind::Movie, "s", 1000).unwrap();
        there.add_history("s|mv|2", MediaKind::Movie, "s", 2000).unwrap();
        there.add_history("s|mv|1", MediaKind::Movie, "s", 1000).unwrap();
        here.sync_merge(&[&there.sync_export().unwrap()], &sources()).unwrap();
        let history: Vec<(String, Timestamp)> = here.history(10).unwrap().into_iter().map(|row| (row.item_id, row.watched_at)).collect();
        assert_eq!(history, [("s|mv|2".to_string(), 2000), ("s|mv|1".to_string(), 1000)], "the same play once");
    }

    #[test]
    fn a_tie_settles_the_same_way_on_both_computers() {
        let folder = tempfile::tempdir().unwrap();
        let (here, there) = (open(&folder, "here.sqlite"), open(&folder, "there.sqlite"));
        here.edit_item("s|ch|1", |row| row.display_name = Some("Alpha".into())).unwrap();
        there.edit_item("s|ch|1", |row| row.display_name = Some("Beta".into())).unwrap();
        // The same second on both.
        here.execute_for_test("INSERT INTO sync_untracked VALUES (1); UPDATE item_edits SET changed_at = 500; DELETE FROM sync_untracked;").unwrap();
        there.execute_for_test("INSERT INTO sync_untracked VALUES (1); UPDATE item_edits SET changed_at = 500; DELETE FROM sync_untracked;").unwrap();
        let (mine, theirs) = (here.sync_export().unwrap(), there.sync_export().unwrap());
        here.sync_merge(&[&theirs], &sources()).unwrap();
        there.sync_merge(&[&mine], &sources()).unwrap();
        assert_eq!(here.item_edits().unwrap(), there.item_edits().unwrap());
    }

    #[test]
    fn rows_that_dont_fit_are_skipped_without_harm() {
        let folder = tempfile::tempdir().unwrap();
        let here = open(&folder, "here.sqlite");
        here.add_to_watchlist(&entry("s|mv|1", "s", "Mine")).unwrap();
        let mut journal = here.sync_export().unwrap();
        let table = journal.get_mut("watchlist").unwrap();
        let title = table.columns.iter().position(|column| column == "title").unwrap();
        let changed = table.columns.iter().position(|column| column == "changed_at").unwrap();
        let mut broken = table.rows[0].clone();
        (broken[title], broken[changed]) = (Value::Null, Value::from(crate::now() + 1000));
        table.rows = vec![broken, vec![Value::from("short")]];
        table.deleted = vec![(vec![], 5)];
        let report = here.sync_merge(&[&journal], &sources()).unwrap();
        assert_eq!(report.skipped, 3);
        assert_eq!(here.watchlist().unwrap()[0].title, "Mine", "this computer's row is untouched");

        let mut other = here.sync_export().unwrap();
        other.get_mut("watchlist").unwrap().key = vec!["something_else".into()];
        assert!(here.sync_merge(&[&other], &sources()).unwrap().skipped > 0, "a table keyed differently is left alone");
    }

    #[test]
    fn old_tombstones_are_pruned() {
        let folder = tempfile::tempdir().unwrap();
        let here = open(&folder, "here.sqlite");
        here.add_to_watchlist(&entry("s|mv|1", "s", "One")).unwrap();
        here.remove_from_watchlist("s|mv|1").unwrap();
        assert_eq!(here.prune_tombstones(crate::now() - 60).unwrap(), 0);
        assert_eq!(here.prune_tombstones(crate::now() + 60).unwrap(), 1);
        assert!(here.tombstone_keys_for_test("watchlist").unwrap().is_empty());
    }
}
