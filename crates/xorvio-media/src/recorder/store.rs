//! `recordings.sqlite`: every recording, scheduled or done, with what the
//! recorder needs to carry on after a restart (its file parts and how much of
//! the programme it captured), and the series recordings that schedule them.
//!
//! People keep real recordings and schedules here, so the schema only ever
//! grows: each change is a numbered step in [`migrate`] that adds columns or
//! tables in one transaction and leaves every row where it was.

use std::collections::HashSet;
use std::path::Path;

use anyhow::Context;
use rusqlite::{Connection, OptionalExtension, Row as SqlRow, params};
use xorvio_model::{Recording, RecordingStatus, SeriesRule, SeriesRuleOptions};

use crate::adbreaks::{AdBreak, AdBreakState, AdBreaks};

pub(crate) const FILE_NAME: &str = "recordings.sqlite";

/// A recording with the recorder's own bookkeeping.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub recording: Recording,
    /// Files written, in order; the first is `recording.file_path`.
    pub parts: Vec<String>,
    /// Seconds of the wanted part of the programme captured so far.
    pub covered_seconds: f64,
    /// When capture was first wanted: the programme start, or later when it
    /// was asked for while already on.
    pub wanted_from: i64,
    /// When someone pressed Stop.
    pub stopped_at: Option<i64>,
    /// For a series recording: which episode it is ("s2e5", or its subtitle
    /// or description when the guide numbers nothing), so it's skipped later.
    pub episode_key: Option<String>,
    /// For a series recording: which airing it is (its channel family and
    /// start), so it's never scheduled twice.
    pub airing_key: Option<String>,
}

/// What a running recording updates as it goes.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Progress {
    pub parts: Vec<String>,
    pub file_size_bytes: i64,
    pub recorded_seconds: f64,
    pub covered_seconds: f64,
}

pub(crate) struct Store {
    db: Connection,
}

/// The schema version [`migrate`] brings a database to.
pub(crate) const VERSION: i64 = 2;

/// The first schema, which every database starts from.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS recordings (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    channel_name TEXT NOT NULL,
    channel_logo_url TEXT,
    title TEXT NOT NULL,
    subtitle TEXT,
    summary TEXT,
    artwork_url TEXT,
    program_id TEXT,
    start INTEGER NOT NULL,
    end INTEGER NOT NULL,
    padding_before INTEGER NOT NULL DEFAULT 0,
    padding_after INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    file_path TEXT,
    file_size INTEGER,
    recorded_seconds REAL,
    error TEXT,
    created_at INTEGER NOT NULL,
    parts TEXT NOT NULL DEFAULT '[]',
    covered_seconds REAL NOT NULL DEFAULT 0,
    wanted_from INTEGER NOT NULL DEFAULT 0,
    stopped_at INTEGER
);
CREATE INDEX IF NOT EXISTS recordings_status ON recordings(status);
";

/// Version 1: series recordings, saving from catch-up, and notes.
const ADDED_IN_1: [(&str, &str); 6] = [
    ("rule_id", "TEXT"),
    ("episode", "TEXT"),
    ("episode_key", "TEXT"),
    ("airing_key", "TEXT"),
    ("from_catchup", "INTEGER NOT NULL DEFAULT 0"),
    ("note", "TEXT"),
];

/// Version 2: ad breaks. `ad_scan` is NULL (never looked for), 'queued',
/// 'found' or 'failed'; `ad_breaks` the breaks found, as JSON; and
/// `ad_scan_note` who found them, or why they couldn't be found.
const ADDED_IN_2: [(&str, &str); 3] = [("ad_scan", "TEXT"), ("ad_breaks", "TEXT"), ("ad_scan_note", "TEXT")];

const SERIES_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS series_rules (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    source_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    channel_name TEXT NOT NULL,
    channel_logo_url TEXT,
    artwork_url TEXT,
    any_channel INTEGER NOT NULL DEFAULT 0,
    new_only INTEGER NOT NULL DEFAULT 1,
    keep INTEGER NOT NULL DEFAULT 0,
    padding_before INTEGER,
    padding_after INTEGER,
    created_at INTEGER NOT NULL
);
-- What a series has had, kept after its recordings are deleted: episodes
-- recorded ('episode:…') and airings cancelled or deleted ('airing:…').
CREATE TABLE IF NOT EXISTS series_history (
    rule_id TEXT NOT NULL,
    key TEXT NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (rule_id, key)
);
CREATE INDEX IF NOT EXISTS recordings_rule ON recordings(rule_id);
";

const COLUMNS: &str = "id, source_id, channel_id, channel_name, channel_logo_url, title, subtitle, summary, artwork_url, \
    program_id, start, end, padding_before, padding_after, status, file_path, file_size, recorded_seconds, error, \
    created_at, parts, covered_seconds, wanted_from, stopped_at, rule_id, episode, episode_key, airing_key, \
    from_catchup, note";

const RULE_COLUMNS: &str = "id, title, source_id, channel_id, channel_name, channel_logo_url, artwork_url, \
    any_channel, new_only, keep, padding_before, padding_after, created_at";

pub(crate) fn status_name(status: RecordingStatus) -> &'static str {
    match status {
        RecordingStatus::Scheduled => "scheduled",
        RecordingStatus::Recording => "recording",
        RecordingStatus::Completed => "completed",
        RecordingStatus::Partial => "partial",
        RecordingStatus::Failed => "failed",
        RecordingStatus::Cancelled => "cancelled",
    }
}

fn status_from(name: &str) -> RecordingStatus {
    match name {
        "scheduled" => RecordingStatus::Scheduled,
        "recording" => RecordingStatus::Recording,
        "completed" => RecordingStatus::Completed,
        "partial" => RecordingStatus::Partial,
        "cancelled" => RecordingStatus::Cancelled,
        _ => RecordingStatus::Failed,
    }
}

fn read_row(row: &SqlRow) -> rusqlite::Result<Row> {
    let parts: String = row.get("parts")?;
    Ok(Row {
        recording: Recording {
            id: row.get("id")?,
            source_id: row.get("source_id")?,
            channel_id: row.get("channel_id")?,
            channel_name: row.get("channel_name")?,
            channel_logo_url: row.get("channel_logo_url")?,
            title: row.get("title")?,
            subtitle: row.get("subtitle")?,
            summary: row.get("summary")?,
            artwork_url: row.get("artwork_url")?,
            program_id: row.get("program_id")?,
            start: row.get("start")?,
            end: row.get("end")?,
            padding_before_seconds: row.get("padding_before")?,
            padding_after_seconds: row.get("padding_after")?,
            status: status_from(&row.get::<_, String>("status")?),
            file_path: row.get("file_path")?,
            file_size_bytes: row.get("file_size")?,
            recorded_seconds: row.get("recorded_seconds")?,
            error: row.get("error")?,
            created_at: row.get("created_at")?,
            rule_id: row.get("rule_id")?,
            episode: row.get("episode")?,
            from_catchup: row.get("from_catchup")?,
            note: row.get("note")?,
            conflict: None,
        },
        parts: serde_json::from_str(&parts).unwrap_or_default(),
        covered_seconds: row.get("covered_seconds")?,
        wanted_from: row.get("wanted_from")?,
        stopped_at: row.get("stopped_at")?,
        episode_key: row.get("episode_key")?,
        airing_key: row.get("airing_key")?,
    })
}

fn read_rule(row: &SqlRow) -> rusqlite::Result<SeriesRule> {
    Ok(SeriesRule {
        id: row.get("id")?,
        title: row.get("title")?,
        source_id: row.get("source_id")?,
        channel_id: row.get("channel_id")?,
        channel_name: row.get("channel_name")?,
        channel_logo_url: row.get("channel_logo_url")?,
        artwork_url: row.get("artwork_url")?,
        options: SeriesRuleOptions {
            any_channel: row.get("any_channel")?,
            new_only: row.get("new_only")?,
            keep: row.get("keep")?,
            padding_before_seconds: row.get("padding_before")?,
            padding_after_seconds: row.get("padding_after")?,
        },
        created_at: row.get("created_at")?,
        next_airing: None,
        next_channel_name: None,
        scheduled_count: 0,
        recorded_count: 0,
        conflict_count: 0,
    })
}

/// Brings the schema up to [`VERSION`] in one transaction: it happens whole
/// or not at all, and only adds, so every recording and schedule already
/// there stays as it was. A database from a newer Xorvio is left alone.
fn migrate(db: &mut Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= VERSION {
        return Ok(());
    }
    let tx = db.transaction()?;
    let existing: HashSet<String> = {
        let mut statement = tx.prepare("SELECT name FROM pragma_table_info('recordings')")?;
        statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    let add_columns = |columns: &[(&str, &str)]| -> rusqlite::Result<()> {
        for (name, definition) in columns {
            if !existing.contains(*name) {
                tx.execute_batch(&format!("ALTER TABLE recordings ADD COLUMN {name} {definition}"))?;
            }
        }
        Ok(())
    };
    if version < 1 {
        add_columns(&ADDED_IN_1)?;
        tx.execute_batch(SERIES_SCHEMA)?;
    }
    if version < 2 {
        add_columns(&ADDED_IN_2)?;
    }
    tx.pragma_update(None, "user_version", VERSION)?;
    tx.commit()?;
    Ok(())
}

impl Store {
    pub fn open(data_dir: &Path) -> anyhow::Result<Store> {
        std::fs::create_dir_all(data_dir).context("the data folder couldn't be created")?;
        let mut db = Connection::open(data_dir.join(FILE_NAME))
            .context("recordings.sqlite couldn't be opened")?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch(SCHEMA)?;
        migrate(&mut db).context("recordings.sqlite couldn't be updated")?;
        Ok(Store { db })
    }

    pub fn insert(&self, row: &Row) -> anyhow::Result<()> {
        let r = &row.recording;
        self.db.execute(
            &format!("INSERT INTO recordings ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)"),
            params![
                r.id, r.source_id, r.channel_id, r.channel_name, r.channel_logo_url, r.title, r.subtitle, r.summary,
                r.artwork_url, r.program_id, r.start, r.end, r.padding_before_seconds, r.padding_after_seconds,
                status_name(r.status), r.file_path, r.file_size_bytes, r.recorded_seconds, r.error, r.created_at,
                serde_json::to_string(&row.parts)?, row.covered_seconds, row.wanted_from, row.stopped_at,
                r.rule_id, r.episode, row.episode_key, row.airing_key, r.from_catchup, r.note,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Option<Row>> {
        Ok(self
            .db
            .query_row(
                &format!("SELECT {COLUMNS} FROM recordings WHERE id = ?1"),
                [id],
                read_row,
            )
            .optional()?)
    }

    /// Everything, by start time.
    pub fn list(&self) -> anyhow::Result<Vec<Row>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {COLUMNS} FROM recordings ORDER BY start, created_at"
        ))?;
        let rows = statement
            .query_map([], read_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Scheduled and recording ones: what the scheduler looks after.
    pub fn pending(&self) -> anyhow::Result<Vec<Row>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {COLUMNS} FROM recordings WHERE status IN ('scheduled', 'recording') ORDER BY start"
        ))?;
        let rows = statement
            .query_map([], read_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn set_status(
        &self,
        id: &str,
        status: RecordingStatus,
        error: Option<&str>,
    ) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE recordings SET status = ?2, error = ?3 WHERE id = ?1",
            params![id, status_name(status), error],
        )?;
        Ok(())
    }

    pub fn set_progress(&self, id: &str, progress: &Progress) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE recordings SET parts = ?2, file_path = ?3, file_size = ?4, recorded_seconds = ?5, covered_seconds = ?6 WHERE id = ?1",
            params![
                id,
                serde_json::to_string(&progress.parts)?,
                progress.parts.first(),
                (!progress.parts.is_empty()).then_some(progress.file_size_bytes),
                (!progress.parts.is_empty()).then_some(progress.recorded_seconds),
                progress.covered_seconds,
            ],
        )?;
        Ok(())
    }

    pub fn set_stopped_at(&self, id: &str, at: i64) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE recordings SET stopped_at = ?2 WHERE id = ?1",
            params![id, at],
        )?;
        Ok(())
    }

    pub fn set_note(&self, id: &str, note: Option<&str>) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE recordings SET note = ?2 WHERE id = ?1",
            params![id, note],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        self.db
            .execute("DELETE FROM recordings WHERE id = ?1", [id])?;
        Ok(())
    }

    /// A series' recordings, whatever became of them, by start time.
    pub fn rule_rows(&self, rule_id: &str) -> anyhow::Result<Vec<Row>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {COLUMNS} FROM recordings WHERE rule_id = ?1 ORDER BY start, created_at"
        ))?;
        let rows = statement
            .query_map([rule_id], read_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Leaves a deleted series' recordings as recordings of their own.
    pub fn detach_from_rule(&self, rule_id: &str) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE recordings SET rule_id = NULL WHERE rule_id = ?1",
            [rule_id],
        )?;
        Ok(())
    }

    // MARK: Ad breaks

    /// What's known of a recording's ad breaks; `None` when it's gone.
    pub fn ad_breaks(&self, id: &str) -> anyhow::Result<Option<AdBreaks>> {
        let found = self
            .db
            .query_row(
                "SELECT ad_scan, ad_breaks, ad_scan_note FROM recordings WHERE id = ?1",
                [id],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?)),
            )
            .optional()?;
        Ok(found.map(|(scan, breaks, note)| {
            let breaks: Vec<AdBreak> = breaks.and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default();
            // Breaks found before stay in use while they're looked for
            // again, and if looking again fails.
            match scan.as_deref() {
                Some("queued") => AdBreaks { state: AdBreakState::Finding, breaks, found_by: None, error: None },
                Some("found") => AdBreaks { state: AdBreakState::Found, breaks, found_by: note, error: None },
                Some("failed") => AdBreaks { state: AdBreakState::Failed, breaks, found_by: None, error: note },
                _ => AdBreaks::never(),
            }
        }))
    }

    /// Notes that a recording's breaks are to be looked for, which carries
    /// on after a restart.
    pub fn queue_ad_scan(&self, id: &str) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE recordings SET ad_scan = 'queued' WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    /// Keeps the breaks found in a recording, and who found them.
    pub fn set_ad_breaks(&self, id: &str, breaks: &[AdBreak], found_by: &str) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE recordings SET ad_scan = 'found', ad_breaks = ?2, ad_scan_note = ?3 WHERE id = ?1",
            params![id, serde_json::to_string(breaks)?, found_by],
        )?;
        Ok(())
    }

    /// Notes why a recording's breaks couldn't be found.
    pub fn set_ad_scan_failed(&self, id: &str, error: &str) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE recordings SET ad_scan = 'failed', ad_scan_note = ?2 WHERE id = ?1",
            params![id, error],
        )?;
        Ok(())
    }

    /// Recordings waiting for their breaks to be looked for.
    pub fn queued_ad_scans(&self) -> anyhow::Result<Vec<String>> {
        let mut statement = self
            .db
            .prepare("SELECT id FROM recordings WHERE ad_scan = 'queued' ORDER BY end")?;
        let ids = statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    // MARK: Series

    /// Every series recording, by title.
    pub fn rules(&self) -> anyhow::Result<Vec<SeriesRule>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {RULE_COLUMNS} FROM series_rules ORDER BY title COLLATE NOCASE, created_at"
        ))?;
        let rules = statement
            .query_map([], read_rule)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rules)
    }

    pub fn rule(&self, id: &str) -> anyhow::Result<Option<SeriesRule>> {
        Ok(self
            .db
            .query_row(
                &format!("SELECT {RULE_COLUMNS} FROM series_rules WHERE id = ?1"),
                [id],
                read_rule,
            )
            .optional()?)
    }

    /// Adds a series, or saves its options when it's already there.
    pub fn save_rule(&self, rule: &SeriesRule) -> anyhow::Result<()> {
        let o = &rule.options;
        self.db.execute(
            &format!(
                "INSERT INTO series_rules ({RULE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(id) DO UPDATE SET any_channel = excluded.any_channel, new_only = excluded.new_only,
                   keep = excluded.keep, padding_before = excluded.padding_before, padding_after = excluded.padding_after"
            ),
            params![
                rule.id, rule.title, rule.source_id, rule.channel_id, rule.channel_name, rule.channel_logo_url,
                rule.artwork_url, o.any_channel, o.new_only, o.keep, o.padding_before_seconds,
                o.padding_after_seconds, rule.created_at,
            ],
        )?;
        Ok(())
    }

    /// Removes a series and what it remembered. Its recordings are the
    /// caller's to deal with first.
    pub fn delete_rule(&self, id: &str) -> anyhow::Result<()> {
        self.db
            .execute("DELETE FROM series_history WHERE rule_id = ?1", [id])?;
        self.db
            .execute("DELETE FROM series_rules WHERE id = ?1", [id])?;
        Ok(())
    }

    /// What a series remembers having had.
    pub fn history(&self, rule_id: &str) -> anyhow::Result<HashSet<String>> {
        let mut statement = self
            .db
            .prepare("SELECT key FROM series_history WHERE rule_id = ?1")?;
        let keys = statement
            .query_map([rule_id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(keys)
    }

    pub fn remember(&self, rule_id: &str, key: &str, at: i64) -> anyhow::Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO series_history (rule_id, key, at) VALUES (?1, ?2, ?3)",
            params![rule_id, key, at],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// recordings.sqlite as the first release left it, with real-looking
    /// rows: a finished recording in two parts that was stopped by hand, and
    /// a schedule for tomorrow.
    fn first_release_database(dir: &Path) {
        let db = Connection::open(dir.join(FILE_NAME)).unwrap();
        db.pragma_update(None, "journal_mode", "WAL").unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch(
            "INSERT INTO recordings (id, source_id, channel_id, channel_name, channel_logo_url, title, subtitle, summary,
                artwork_url, program_id, start, end, padding_before, padding_after, status, file_path, file_size,
                recorded_seconds, error, created_at, parts, covered_seconds, wanted_from, stopped_at)
             VALUES
               ('done', 'src', 'src|ch|1', 'Harbor News', 'https://img.example/h.png', 'Late Edition', 'S1 E2',
                'The day''s news.', NULL, 'src|harbor|1000', 1000, 4600, 60, 180, 'partial', '/rec/a.ts', 123456,
                3000.5, 'About 10 minutes of the programme wasn''t recorded.', 900, '[\"/rec/a.ts\",\"/rec/a (part 2).ts\"]',
                2990.0, 1000, 4000),
               ('later', 'src', 'src|ch|2', 'Summit Sports', NULL, 'Matchday', NULL, NULL, NULL, NULL,
                2000000000, 2000003600, 0, 0, 'scheduled', NULL, NULL, NULL, NULL, 950, '[]', 0, 2000000000, NULL);",
        )
        .unwrap();
    }

    #[test]
    fn migrating_keeps_every_recording_and_schedule() {
        let dir = tempfile::tempdir().unwrap();
        first_release_database(dir.path());
        let before: Vec<(String, String)> = {
            let db = Connection::open(dir.path().join(FILE_NAME)).unwrap();
            let mut statement = db
                .prepare("SELECT id, status FROM recordings ORDER BY id")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(before.len(), 2);

        let store = Store::open(dir.path()).unwrap();
        let rows = store.list().unwrap();
        assert_eq!(rows.len(), 2);
        let done = &rows[0];
        assert_eq!(done.recording.id, "done");
        assert_eq!(done.recording.status, RecordingStatus::Partial);
        assert_eq!(done.recording.title, "Late Edition");
        assert_eq!(done.recording.subtitle.as_deref(), Some("S1 E2"));
        assert_eq!(done.recording.summary.as_deref(), Some("The day's news."));
        assert_eq!(
            done.recording.program_id.as_deref(),
            Some("src|harbor|1000")
        );
        assert_eq!((done.recording.start, done.recording.end), (1000, 4600));
        assert_eq!(
            (
                done.recording.padding_before_seconds,
                done.recording.padding_after_seconds
            ),
            (60, 180)
        );
        assert_eq!(done.recording.file_path.as_deref(), Some("/rec/a.ts"));
        assert_eq!(done.recording.file_size_bytes, Some(123456));
        assert_eq!(done.recording.recorded_seconds, Some(3000.5));
        assert!(
            done.recording
                .error
                .as_deref()
                .unwrap()
                .starts_with("About 10 minutes")
        );
        assert_eq!(done.parts, ["/rec/a.ts", "/rec/a (part 2).ts"]);
        assert_eq!(
            (done.covered_seconds, done.wanted_from, done.stopped_at),
            (2990.0, 1000, Some(4000))
        );
        // What's new starts empty.
        assert_eq!(done.recording.rule_id, None);
        assert_eq!(done.recording.episode, None);
        assert!(!done.recording.from_catchup);
        assert_eq!(done.recording.note, None);
        assert_eq!(
            (done.episode_key.as_deref(), done.airing_key.as_deref()),
            (None, None)
        );

        let later = &rows[1];
        assert_eq!(later.recording.id, "later");
        assert_eq!(later.recording.status, RecordingStatus::Scheduled);
        assert_eq!(
            store.pending().unwrap().len(),
            1,
            "the schedule still records"
        );
        assert!(store.rules().unwrap().is_empty());
        let version: i64 = store
            .db
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, VERSION);
        drop(store);

        // Opening again changes nothing, and the new columns hold what's put in them.
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.list().unwrap(), rows);
        store.set_note("later", Some("Saved elsewhere.")).unwrap();
        let rule = SeriesRule {
            id: "rule".into(),
            title: "Matchday".into(),
            source_id: "src".into(),
            channel_id: "src|ch|2".into(),
            channel_name: "Summit Sports".into(),
            channel_logo_url: None,
            artwork_url: None,
            options: SeriesRuleOptions {
                keep: 5,
                padding_after_seconds: Some(600),
                ..SeriesRuleOptions::default()
            },
            created_at: 1,
            next_airing: None,
            next_channel_name: None,
            scheduled_count: 0,
            recorded_count: 0,
            conflict_count: 0,
        };
        store.save_rule(&rule).unwrap();
        assert_eq!(store.rule("rule").unwrap(), Some(rule));
        store.remember("rule", "episode:s1e1", 5).unwrap();
        store.remember("rule", "episode:s1e1", 6).unwrap();
        assert_eq!(store.history("rule").unwrap().len(), 1);
        assert_eq!(
            store
                .get("later")
                .unwrap()
                .unwrap()
                .recording
                .note
                .as_deref(),
            Some("Saved elsewhere.")
        );
    }

    /// recordings.sqlite as version 1 left it (series recordings, saving
    /// from catch-up, notes), with a series and its history.
    fn version_one_database(dir: &Path) {
        first_release_database(dir);
        let db = Connection::open(dir.join(FILE_NAME)).unwrap();
        for (name, definition) in ADDED_IN_1 {
            db.execute_batch(&format!("ALTER TABLE recordings ADD COLUMN {name} {definition}")).unwrap();
        }
        db.execute_batch(SERIES_SCHEMA).unwrap();
        db.execute_batch(
            "UPDATE recordings SET rule_id = 'rule', episode = 'S1 E2', episode_key = 's1e2', airing_key = 'harbor|1000',
                note = 'Saved in Movies › Xorvio.' WHERE id = 'done';
             INSERT INTO series_rules (id, title, source_id, channel_id, channel_name, any_channel, new_only, keep, created_at)
               VALUES ('rule', 'Late Edition', 'src', 'src|ch|1', 'Harbor News', 1, 1, 5, 800);
             INSERT INTO series_history (rule_id, key, at) VALUES ('rule', 'episode:s1e1', 700);",
        )
        .unwrap();
        db.pragma_update(None, "user_version", 1).unwrap();
    }

    /// Every version 1 column of every recording, as text, for comparing.
    fn version_one_rows(dir: &Path) -> Vec<Vec<String>> {
        let db = Connection::open(dir.join(FILE_NAME)).unwrap();
        let mut statement = db.prepare(&format!("SELECT {COLUMNS} FROM recordings ORDER BY id")).unwrap();
        let count = statement.column_count();
        statement
            .query_map([], |row| {
                (0..count)
                    .map(|i| row.get::<_, rusqlite::types::Value>(i).map(|v| format!("{v:?}")))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn version_two_adds_ad_breaks_and_keeps_every_row() {
        let dir = tempfile::tempdir().unwrap();
        version_one_database(dir.path());
        let before = version_one_rows(dir.path());
        assert_eq!(before.len(), 2);

        let store = Store::open(dir.path()).unwrap();
        let version: i64 = store.db.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, 2);
        drop(store);
        assert_eq!(version_one_rows(dir.path()), before, "every recording as it was");
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.rules().unwrap().len(), 1);
        assert_eq!(store.history("rule").unwrap().len(), 1);
        assert_eq!(store.get("done").unwrap().unwrap().recording.note.as_deref(), Some("Saved in Movies › Xorvio."));
        assert_eq!(store.ad_breaks("done").unwrap(), Some(AdBreaks::never()), "nothing looked for yet");
        assert_eq!(store.ad_breaks("gone").unwrap(), None);

        // The new columns hold what's put in them, through a reopen.
        let found = [AdBreak { start_seconds: 600.0, end_seconds: 780.5 }];
        store.queue_ad_scan("done").unwrap();
        assert_eq!(store.queued_ad_scans().unwrap(), ["done"]);
        assert_eq!(store.ad_breaks("done").unwrap().unwrap().state, AdBreakState::Finding);
        store.set_ad_breaks("done", &found, "Xorvio").unwrap();
        assert!(store.queued_ad_scans().unwrap().is_empty());
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        let kept = store.ad_breaks("done").unwrap().unwrap();
        assert_eq!((kept.state, kept.breaks.as_slice(), kept.found_by.as_deref()), (AdBreakState::Found, &found[..], Some("Xorvio")));
        // Looking again keeps the breaks in use until it's done, and if it fails.
        store.queue_ad_scan("done").unwrap();
        assert_eq!(store.ad_breaks("done").unwrap().unwrap().breaks, found);
        store.set_ad_scan_failed("done", "ffmpeg couldn't read the recording.").unwrap();
        let failed = store.ad_breaks("done").unwrap().unwrap();
        assert_eq!((failed.state, failed.error.as_deref()), (AdBreakState::Failed, Some("ffmpeg couldn't read the recording.")));
        assert_eq!(failed.breaks, found);
        assert_eq!(version_one_rows(dir.path()), before, "and the recordings are still as they were");
    }

    #[test]
    fn a_new_database_starts_at_the_latest_schema() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let version: i64 = store
            .db
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, VERSION);
        assert!(store.list().unwrap().is_empty());
        assert!(store.rules().unwrap().is_empty());
    }
}
