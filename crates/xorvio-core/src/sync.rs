//! Syncing the library between computers through a folder they share
//! (iCloud Drive, Dropbox, Syncthing, a NAS share). There's no server: each
//! computer writes one file of its own, `<device id>.xorvio-sync`, holding
//! its synced rows with when each changed and its deletions (see
//! store/library/sync.rs), and merges the other computers' files into its
//! library, newest wins per row.
//!
//! Cloud folders hand files over while they're still arriving and leave
//! conflict copies behind, so a file that can't be read whole is skipped
//! until it can, a copy is just one more file to merge, and this computer's
//! own file is written to a temporary name and renamed into place.
//!
//! What syncs: the watchlist, progress, history and what each title was
//! called, favorites and channel lists, renames, hiding and locks, guide
//! channels chosen by hand, catch-up offsets, preferred versions and
//! profiles. Not sources, logins, settings, Trakt's record or the parental
//! PIN, which stay with each computer.

use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use xorvio_model::Timestamp;

use crate::store::library::JournalTables;
use crate::{Core, Result, now};

/// What a sync file's name ends with.
pub const JOURNAL_EXTENSION: &str = "xorvio-sync";
/// The journal layout this version writes and reads.
const FORMAT: u32 = 1;
/// Far beyond any library; a file claiming more is damaged.
const MAX_JOURNAL_BYTES: u64 = 512 * 1024 * 1024;
/// How long deletions are remembered (see `prune_tombstones`).
const KEEP_DELETIONS_FOR: i64 = 180 * 86_400;
/// Add-on titles belong to no source, and play on every computer.
const ADDON_SOURCE_ID: &str = "addon";
/// This computer's sync bookkeeping, in the data folder. It's never in a
/// backup, so a computer restored from another's backup stays itself.
const STATE_FILE: &str = "sync-state.json";

/// One computer's sync file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    format: u32,
    device_id: String,
    device_name: String,
    app_version: String,
    written_at: Timestamp,
    tables: JournalTables,
}

/// Another computer seen in the sync folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncComputer {
    pub name: String,
    /// When it last wrote its file.
    pub last_seen: Timestamp,
}

/// What this computer keeps about syncing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncState {
    /// Names this computer's file in the folder.
    pub device_id: String,
    pub last_sync: Option<Timestamp>,
    /// Why the last sync failed, in plain words.
    pub last_error: Option<String>,
    /// Something worth knowing that isn't a failure (a file still arriving).
    pub notice: Option<String>,
    /// The folder synced with; another one starts afresh.
    pub folder: Option<String>,
    /// The other computers, by device id.
    pub computers: BTreeMap<String, SyncComputer>,
    /// A digest of what this computer last wrote, and when, so an unchanged
    /// library isn't written again and a copy of this computer's data folder
    /// running elsewhere (writing as it) is noticed.
    pub written_digest: Option<String>,
    pub written_at: Option<Timestamp>,
}

impl SyncState {
    /// The saved state, or a new one with a fresh device id.
    pub fn load(data_dir: &Path) -> SyncState {
        let mut state: SyncState =
            std::fs::read(data_dir.join(STATE_FILE)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        if state.device_id.trim().is_empty() {
            state.device_id = new_device_id();
        }
        state
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(data_dir)?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.persist(data_dir.join(STATE_FILE)).context("The sync state couldn't be saved.")?;
        Ok(())
    }

    /// The other computers, most recently seen first.
    pub fn computers_seen(&self) -> Vec<SyncComputer> {
        let mut computers: Vec<SyncComputer> = self.computers.values().cloned().collect();
        computers.sort_by_key(|computer| std::cmp::Reverse(computer.last_seen));
        computers
    }
}

fn new_device_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// What one sync did.
#[derive(Debug, Default)]
pub struct SyncOutcome {
    /// The library changed here, so screens should reload.
    pub library_changed: bool,
    /// This computer's file was written (it had changed).
    pub wrote: bool,
}

impl Core {
    /// How many synced rows this computer has changed itself. When it moves,
    /// there's something to tell the other computers.
    pub fn sync_change_count(&self) -> i64 {
        self.library.sync_changes().unwrap_or(0)
    }

    /// Syncs with the other computers' files in `folder`: merges them into
    /// the library, then writes this computer's own. `this_computer` is the
    /// name the others show for it.
    pub fn sync_with_folder(&self, folder: &Path, state: &mut SyncState, this_computer: &str) -> Result<SyncOutcome> {
        let folder_text = folder.to_string_lossy().into_owned();
        if state.folder.as_deref() != Some(&folder_text) {
            state.folder = Some(folder_text);
            state.computers.clear();
            state.last_sync = None;
            state.notice = None;
            state.written_digest = None;
            state.written_at = None;
        }
        if !folder.is_dir() {
            anyhow::bail!("The sync folder can't be found. If it's on a drive or in a cloud folder, check that it's available.");
        }

        let (journals, unreadable) = read_journals(folder)?;
        let mut others: Vec<Journal> = Vec::new();
        for journal in journals {
            if journal.device_id != state.device_id {
                others.push(journal);
                continue;
            }
            // This computer's own file (or an older copy of it), unless
            // another computer is writing as it (a data folder copied to a
            // new Mac): then that one's merged like any other and this
            // computer takes a new id.
            let digest = digest_of(&journal.tables)?;
            let ours = state.written_digest.as_deref() == Some(digest.as_str())
                || (journal.device_name == this_computer && state.written_at.is_none_or(|written| journal.written_at <= written));
            if !ours {
                tracing::info!("another computer was syncing as this one; this computer takes a new sync id");
                state.device_id = new_device_id();
                state.written_digest = None;
                others.push(journal);
            }
        }

        let mut known_sources: HashSet<String> = self.configs().into_iter().map(|config| config.id).collect();
        known_sources.insert(ADDON_SOURCE_ID.to_string());
        let tables: Vec<&JournalTables> = others.iter().map(|journal| &journal.tables).collect();
        let report = self.library.sync_merge(&tables, &known_sources)?;
        if report.skipped > 0 {
            tracing::debug!("sync left out {} rows it couldn't use", report.skipped);
        }
        self.apply_synced_changes(&report.changed_tables)?;
        if let Err(error) = self.library.prune_tombstones(now() - KEEP_DELETIONS_FOR) {
            tracing::warn!("old sync deletions couldn't be cleared: {error}");
        }

        let tables = self.library.sync_export()?;
        let digest = digest_of(&tables)?;
        let path = folder.join(format!("{}.{JOURNAL_EXTENSION}", state.device_id));
        let wrote = state.written_digest.as_deref() != Some(digest.as_str()) || !path.exists();
        if wrote {
            let written_at = now();
            let journal = Journal {
                format: FORMAT,
                device_id: state.device_id.clone(),
                device_name: this_computer.to_string(),
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                written_at,
                tables,
            };
            write_journal(&path, &journal)?;
            state.written_digest = Some(digest);
            state.written_at = Some(written_at);
        }

        for journal in &others {
            let seen = SyncComputer { name: journal.device_name.clone(), last_seen: journal.written_at };
            let newer = state.computers.get(&journal.device_id).is_none_or(|known| known.last_seen <= seen.last_seen);
            if newer {
                state.computers.insert(journal.device_id.clone(), seen);
            }
        }
        state.computers.remove(&state.device_id);
        state.last_sync = Some(now());
        state.last_error = None;
        state.notice = match unreadable.len() {
            0 => None,
            1 => Some("One file in the sync folder couldn't be read yet, probably because it's still arriving. It's tried again next time.".into()),
            count => Some(format!("{count} files in the sync folder couldn't be read yet, probably because they're still arriving. They're tried again next time.")),
        };
        Ok(SyncOutcome { library_changed: !report.changed_tables.is_empty(), wrote })
    }

    /// What has to follow rows another computer changed: names, hiding and
    /// versions are read again, and channels follow guide choices.
    fn apply_synced_changes(&self, tables: &std::collections::BTreeSet<&'static str>) -> Result<()> {
        if ["item_edits", "catchup_offsets", "preferred_versions"].iter().any(|table| tables.contains(table)) {
            self.edits_changed();
        }
        if tables.contains("guide_overrides") {
            self.catalog.relink_guides(None, &self.guide_links()?)?;
        }
        Ok(())
    }
}

/// Every whole sync file in the folder, and the names of those that
/// couldn't be read (half-written, or damaged).
fn read_journals(folder: &Path) -> Result<(Vec<Journal>, Vec<String>)> {
    let entries = std::fs::read_dir(folder).map_err(|error| folder_error(&error))?;
    let mut journals = Vec::new();
    let mut unreadable = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        // Temporary files start with a dot.
        if name.starts_with('.') || path.extension().and_then(|extension| extension.to_str()) != Some(JOURNAL_EXTENSION) {
            continue;
        }
        match read_journal(&path) {
            Ok(journal) => journals.push(journal),
            Err(error) => {
                tracing::debug!("the sync file {name} couldn't be read: {error:#}");
                unreadable.push(name);
            }
        }
    }
    Ok((journals, unreadable))
}

/// A sync file, whole and sound, or an error.
fn read_journal(path: &Path) -> Result<Journal> {
    let size = std::fs::metadata(path)?.len();
    anyhow::ensure!(size <= MAX_JOURNAL_BYTES, "it's too big to be a sync file");
    let bytes = std::fs::read(path)?;
    let text = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut text = Vec::new();
        GzDecoder::new(bytes.as_slice()).take(MAX_JOURNAL_BYTES).read_to_end(&mut text)?;
        text
    } else {
        bytes
    };
    let journal: Journal = serde_json::from_slice(&text)?;
    anyhow::ensure!(journal.format <= FORMAT, "it was written by a newer version of Xorvio");
    anyhow::ensure!(!journal.device_id.trim().is_empty(), "it doesn't say which computer wrote it");
    Ok(journal)
}

/// Writes this computer's file in one step: a temporary file beside it,
/// renamed over it, so the others never see half of it.
fn write_journal(path: &Path, journal: &Journal) -> Result<()> {
    let folder = path.parent().context("The sync file has no folder.")?;
    let prefix = format!(".{}-", journal.device_id);
    remove_stale_temporaries(folder, &prefix);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    serde_json::to_writer(&mut encoder, journal)?;
    let bytes = encoder.finish()?;
    let mut file = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".tmp")
        .tempfile_in(folder)
        .map_err(|error| write_error(&error))?;
    file.write_all(&bytes).map_err(|error| write_error(&error))?;
    file.as_file().sync_all().map_err(|error| write_error(&error))?;
    file.persist(path).map_err(|error| write_error(&error.error))?;
    Ok(())
}

/// This computer's temporary files left in the folder by an app that was
/// closed mid-write (each computer tidies only its own).
fn remove_stale_temporaries(folder: &Path, prefix: &str) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age.as_secs() > 3600));
        if name.starts_with(prefix) && name.ends_with(".tmp") && stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn digest_of(tables: &JournalTables) -> Result<String> {
    let bytes = serde_json::to_vec(tables)?;
    Ok(Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect())
}

fn folder_error(error: &std::io::Error) -> anyhow::Error {
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            anyhow::anyhow!("The sync folder can't be found. If it's on a drive or in a cloud folder, check that it's available.")
        }
        std::io::ErrorKind::PermissionDenied => {
            anyhow::anyhow!("Xorvio isn't allowed to open the sync folder. Choose it again in Settings, or check its permissions.")
        }
        _ => anyhow::anyhow!("The sync folder couldn't be read ({error})."),
    }
}

fn write_error(error: &std::io::Error) -> anyhow::Error {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            anyhow::anyhow!("Xorvio isn't allowed to save in the sync folder. Choose it again in Settings, or check its permissions.")
        }
        std::io::ErrorKind::StorageFull => anyhow::anyhow!("There isn't enough space to save in the sync folder."),
        _ => anyhow::anyhow!("Xorvio couldn't save its file in the sync folder ({error})."),
    }
}

/// Where this computer's file is in the folder, for tests and support.
pub fn journal_path(folder: &Path, state: &SyncState) -> PathBuf {
    folder.join(format!("{}.{JOURNAL_EXTENSION}", state.device_id))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use xorvio_model::{MediaKind, PlaybackProgress, WatchlistEntry};

    use super::*;
    use crate::MemorySecrets;

    /// A computer: its own data folder with the demo source, and its sync state.
    struct Computer {
        _data: tempfile::TempDir,
        core: Arc<Core>,
        state: SyncState,
        name: &'static str,
    }

    impl Computer {
        fn new(name: &'static str) -> Computer {
            let data = tempfile::tempdir().unwrap();
            let core = Core::open(data.path(), Arc::new(MemorySecrets::new())).unwrap();
            core.add_demo_source().unwrap();
            let state = SyncState::load(data.path());
            Computer { _data: data, core, state, name }
        }

        fn sync(&mut self, folder: &Path) -> SyncOutcome {
            self.core.sync_with_folder(folder, &mut self.state, self.name).unwrap()
        }

        /// Everything this computer changed so far happened a while ago.
        fn later(&self) {
            self.core.library.age_changes_for_test(100).unwrap();
        }

        fn titles(&self) -> Vec<String> {
            let mut titles: Vec<String> = self.core.watchlist().unwrap().into_iter().map(|entry| entry.title).collect();
            titles.sort();
            titles
        }
    }

    fn film(id: &str, title: &str) -> WatchlistEntry {
        WatchlistEntry {
            id: format!("demo|mv|{id}"),
            kind: MediaKind::Movie,
            source_id: "demo".into(),
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

    /// Time passes for every computer and the folder (the tests run within
    /// a second or two): what each changed so far happened a while ago, and
    /// each writes its file again.
    fn time_passes(folder: &Path, computers: &mut [&mut Computer]) {
        for computer in computers.iter_mut() {
            computer.later();
            computer.state.written_digest = None;
            std::fs::remove_file(journal_path(folder, &computer.state)).unwrap();
        }
    }

    #[test]
    fn two_computers_sync_through_a_folder() {
        let folder = tempfile::tempdir().unwrap();
        let (mut desk, mut laptop) = (Computer::new("Desk"), Computer::new("Laptop"));

        // Adds on both, then each syncs twice (so both have read the other).
        desk.core.set_watchlisted(film("1", "Paper Boats"), true).unwrap();
        desk.core.set_watchlisted(film("2", "Tidewater"), true).unwrap();
        laptop.core.set_watchlisted(film("3", "Low Orbit"), true).unwrap();
        desk.sync(folder.path());
        assert!(laptop.sync(folder.path()).library_changed);
        desk.sync(folder.path());
        assert_eq!(desk.titles(), ["Low Orbit", "Paper Boats", "Tidewater"]);
        assert_eq!(laptop.titles(), desk.titles());
        assert_eq!(desk.state.computers_seen().iter().map(|seen| seen.name.as_str()).collect::<Vec<_>>(), ["Laptop"]);
        assert_eq!(laptop.state.computers_seen()[0].name, "Desk");

        // A delete on one, an edit on the other.
        time_passes(folder.path(), &mut [&mut desk, &mut laptop]);
        laptop.core.set_watchlisted(film("2", "Tidewater"), false).unwrap();
        desk.core.set_watchlisted(film("1", "Paper Boats (Director's Cut)"), true).unwrap();
        laptop.sync(folder.path());
        desk.sync(folder.path());
        laptop.sync(folder.path());
        assert_eq!(desk.titles(), ["Low Orbit", "Paper Boats (Director's Cut)"]);
        assert_eq!(laptop.titles(), desk.titles());

        // The same title changed on both: the later change wins everywhere.
        time_passes(folder.path(), &mut [&mut desk, &mut laptop]);
        desk.core.set_watchlisted(film("3", "Low Orbit (desk)"), true).unwrap();
        desk.later();
        laptop.core.set_watchlisted(film("3", "Low Orbit (laptop, later)"), true).unwrap();
        desk.sync(folder.path());
        laptop.sync(folder.path());
        desk.sync(folder.path());
        assert!(desk.titles().contains(&"Low Orbit (laptop, later)".to_string()), "{:?}", desk.titles());
        assert_eq!(laptop.titles(), desk.titles());

        // Progress: the newer position wins, whichever computer wrote last.
        let place = |position: f64, at: Timestamp| PlaybackProgress {
            item_id: "demo|mv|9".into(),
            kind: MediaKind::Movie,
            source_id: "demo".into(),
            series_id: None,
            position_seconds: position,
            duration_seconds: 5400.0,
            finished: false,
            updated_at: at,
        };
        laptop.core.save_progress(place(2400.0, now())).unwrap();
        desk.core.save_progress(place(300.0, now() - 3600)).unwrap();
        laptop.sync(folder.path());
        desk.sync(folder.path());
        assert_eq!(desk.core.progress("demo|mv|9").unwrap().unwrap().position_seconds, 2400.0);

        // Nothing new: nothing is written.
        assert!(!desk.sync(folder.path()).wrote);
    }

    #[test]
    fn half_written_and_strange_files_are_passed_over() {
        let folder = tempfile::tempdir().unwrap();
        let (mut desk, mut laptop) = (Computer::new("Desk"), Computer::new("Laptop"));
        laptop.core.set_watchlisted(film("1", "Paper Boats"), true).unwrap();
        laptop.sync(folder.path());
        let whole = std::fs::read(journal_path(folder.path(), &laptop.state)).unwrap();

        // The laptop's file, cut off part-way as a cloud drive delivers it.
        std::fs::write(journal_path(folder.path(), &laptop.state), &whole[..whole.len() / 2]).unwrap();
        std::fs::write(folder.path().join("notes.txt"), "not a sync file").unwrap();
        std::fs::write(folder.path().join("garbage.xorvio-sync"), "{ not json").unwrap();
        std::fs::write(folder.path().join(".laptop.xorvio-sync"), &whole).unwrap();
        desk.sync(folder.path());
        assert!(desk.titles().is_empty(), "nothing merged from a half file");
        assert!(desk.state.notice.as_deref().is_some_and(|notice| notice.starts_with("2 files")), "{:?}", desk.state.notice);
        assert!(desk.state.last_error.is_none(), "that isn't a failure");

        // Once it's whole, it merges; a conflict copy is merged too, harmlessly.
        std::fs::write(journal_path(folder.path(), &laptop.state), &whole).unwrap();
        std::fs::write(folder.path().join("conflicted copy.xorvio-sync"), &whole).unwrap();
        std::fs::remove_file(folder.path().join("garbage.xorvio-sync")).unwrap();
        desk.sync(folder.path());
        assert_eq!(desk.titles(), ["Paper Boats"]);
        assert!(desk.state.notice.is_none());
        assert_eq!(desk.state.computers_seen().len(), 1, "one computer, however many copies of its file");
        let leftovers: Vec<String> = std::fs::read_dir(folder.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no temporary files left behind: {leftovers:?}");
    }

    #[test]
    fn a_missing_folder_is_a_plain_error() {
        let mut desk = Computer::new("Desk");
        let error = desk.core.sync_with_folder(Path::new("/nowhere/at/all"), &mut desk.state, "Desk").unwrap_err();
        assert!(error.to_string().starts_with("The sync folder can't be found"), "{error}");
    }

    #[test]
    fn a_copied_data_folder_takes_a_new_id() {
        let folder = tempfile::tempdir().unwrap();
        let (mut desk, mut copy) = (Computer::new("Desk"), Computer::new("New Mac"));
        desk.sync(folder.path());
        // The new Mac was set up with a copy of the desk's data folder.
        copy.state = desk.state.clone();
        copy.core.set_watchlisted(film("1", "Paper Boats"), true).unwrap();
        copy.sync(folder.path());
        let first = desk.state.device_id.clone();
        desk.sync(folder.path());
        assert_ne!(desk.state.device_id, first, "the desk noticed and moved to its own id");
        assert_eq!(desk.titles(), ["Paper Boats"]);
        desk.sync(folder.path());
        copy.sync(folder.path());
        assert_eq!(copy.state.computers_seen()[0].name, "Desk");
    }

    #[test]
    fn the_state_survives_a_restart() {
        let data = tempfile::tempdir().unwrap();
        let mut state = SyncState::load(data.path());
        let id = state.device_id.clone();
        assert_eq!(id.len(), 32);
        state.last_sync = Some(5);
        state.save(data.path()).unwrap();
        let again = SyncState::load(data.path());
        assert_eq!((again.device_id, again.last_sync), (id, Some(5)));
    }
}
