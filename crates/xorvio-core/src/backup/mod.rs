//! Backups: one `.xorvio-backup` file holding what people make in Xorvio,
//! and putting one back.
//!
//! A backup is a zip: `manifest.json` (when, which computer, what's inside),
//! copies of `library.sqlite` and `recordings.sqlite` that SQLite makes
//! while they're open (`VACUUM INTO`), and `settings.json`, `sources.json`
//! and `addons.json`. Logins and links, the keychain's part, go in only when
//! the person chooses a passphrase for that backup, sealed with it
//! (`secrets.sealed`, see seal.rs). The catalog never does: sources load it
//! again.
//!
//! Restoring can't swap databases that are open, so it takes two steps.
//! [`stage_restore`] checks the backup and stages its files in
//! `restore-pending/` in the data folder (its logins sealed again with a
//! random key the keychain holds meanwhile). The app restarts, and before
//! any store opens [`apply_pending_restore`] sets this computer's files
//! aside in `restore-previous/`, puts the backup's in place and writes the
//! logins to the keychain. If anything fails part-way, even the app being
//! closed in the middle, what was set aside goes back.

mod seal;

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use xorvio_model::{SourceConfig, SourceKind, Timestamp};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::store::library::SCHEMA_VERSION;
use crate::{Core, Result, SecretStore, now};

/// What a backup file's name ends with.
pub const BACKUP_EXTENSION: &str = "xorvio-backup";
/// The shortest passphrase a backup's logins can be sealed with.
pub const MIN_PASSPHRASE_CHARS: usize = 8;
/// How many automatic backups of this computer a folder keeps.
pub const AUTOMATIC_KEEP: usize = 7;

/// The backup layout this version writes and reads.
const FORMAT: u32 = 1;
const APP: &str = "Xorvio";
const MANIFEST: &str = "manifest.json";
const SEALED_SECRETS: &str = "secrets.sealed";
/// The files a backup holds and a restore replaces, in the data folder.
const FILES: [&str; 5] = ["library.sqlite", "recordings.sqlite", "settings.json", "sources.json", "addons.json"];
const DATABASES: [&str; 2] = ["library.sqlite", "recordings.sqlite"];
/// What SQLite keeps beside a database, which belongs with it.
const SIDECARS: [&str; 2] = ["-wal", "-shm"];
const PENDING: &str = "restore-pending";
const STAGING: &str = "restore-pending.partial";
const PREVIOUS: &str = "restore-previous";
const FINISHED: &str = "restore-finished";
/// In `restore-pending/` while the restore is put in place, saying how far
/// it got, so one cut off is noticed and undone at the next launch.
const APPLYING: &str = "applying";
const STAGED_INFO: &str = "restore.json";
/// The keychain entry holding the key staged logins are sealed with.
const RESTORE_KEY: &str = "backup:restore-key";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_SECRETS_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024;

const NOT_A_BACKUP: &str = "This isn't a Xorvio backup, or it's damaged.";
const NEWER: &str = "This backup was made by a newer version of Xorvio. Update Xorvio, then restore it.";
const WRONG_PASSPHRASE: &str = "That passphrase doesn't open this backup's logins. Check it and try again, or leave it empty to restore without them.";

/// How much of each thing a backup holds, for the person to recognise it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BackupCounts {
    pub sources: i64,
    pub addons: i64,
    pub watchlist: i64,
    pub progress: i64,
    pub history: i64,
    pub favorites: i64,
    pub lists: i64,
    pub item_edits: i64,
    pub recordings: i64,
    pub scheduled_recordings: i64,
}

/// `manifest.json`: what a backup is. Never a login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    pub format: u32,
    pub app: String,
    pub app_version: String,
    pub created_at: Timestamp,
    /// The computer it was made on, by name.
    pub computer: String,
    pub library_schema: i64,
    pub files: Vec<String>,
    #[serde(default)]
    pub counts: BackupCounts,
    /// Logins and links are inside, sealed with a passphrase.
    pub includes_secrets: bool,
    #[serde(default)]
    pub automatic: bool,
}

/// How to make a backup.
pub struct BackupOptions<'a> {
    pub data_dir: &'a Path,
    /// This computer's name, for the manifest.
    pub computer: &'a str,
    /// Seals logins and links into the backup; none leaves them out.
    pub passphrase: Option<&'a str>,
    /// Keychain entries Core doesn't know of (add-on links, which the app keeps).
    pub extra_secrets: &'a [String],
    /// Made on a schedule: never with logins.
    pub automatic: bool,
}

impl Core {
    /// Writes a backup to `destination`, whole or not at all (it's written
    /// beside it and renamed into place).
    pub fn back_up(&self, destination: &Path, options: &BackupOptions<'_>) -> Result<BackupManifest> {
        let passphrase = options.passphrase.filter(|passphrase| !passphrase.is_empty() && !options.automatic);
        if passphrase.is_some_and(|passphrase| passphrase.chars().count() < MIN_PASSPHRASE_CHARS) {
            anyhow::bail!("Choose a passphrase of at least {MIN_PASSPHRASE_CHARS} characters.");
        }
        let work = tempfile::Builder::new().prefix(".backup-").tempdir_in(options.data_dir)?;
        let library = work.path().join("library.sqlite");
        self.library.snapshot_into(&library).context("The library couldn't be copied for the backup.")?;
        let mut entries: Vec<(&str, PathBuf)> = vec![("library.sqlite", library)];
        let recordings = options.data_dir.join("recordings.sqlite");
        if recordings.exists() {
            let copy = work.path().join("recordings.sqlite");
            snapshot_database(&recordings, &copy).context("The recordings couldn't be copied for the backup.")?;
            entries.push(("recordings.sqlite", copy));
        }
        // Each is written whole, to a temporary file and renamed, so reading
        // one now finds a complete file.
        for name in ["settings.json", "sources.json", "addons.json"] {
            let path = options.data_dir.join(name);
            if path.exists() {
                entries.push((name, path));
            }
        }
        let sealed = match passphrase {
            Some(passphrase) => Some(seal::seal_with_passphrase(&serde_json::to_vec(&self.keychain_entries(options.extra_secrets)?)?, passphrase)?),
            None => None,
        };
        let manifest = BackupManifest {
            format: FORMAT,
            app: APP.into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            created_at: now(),
            computer: options.computer.to_string(),
            library_schema: SCHEMA_VERSION,
            files: entries.iter().map(|(name, _)| name.to_string()).collect(),
            counts: count(&entries),
            includes_secrets: sealed.is_some(),
            automatic: options.automatic,
        };
        write_archive(destination, &manifest, &entries, sealed.as_deref())?;
        Ok(manifest)
    }

    /// Everything Xorvio keeps in the keychain: each source's and added
    /// guide's logins and links, Trakt's connection and app, the TMDB key,
    /// and the `extra` entries the app names.
    fn keychain_entries(&self, extra: &[String]) -> Result<BTreeMap<String, String>> {
        let mut keys: Vec<String> = self.configs().into_iter().map(|config| config.id).collect();
        keys.extend(self.library.guide_sources()?.into_iter().map(|guide| guide.id));
        keys.extend([crate::trakt::APP_KEY, crate::trakt::CONNECTION_KEY, crate::tmdb::KEY_ENTRY].map(String::from));
        keys.extend(extra.iter().cloned());
        let mut entries = BTreeMap::new();
        for key in keys {
            if let Some(value) = self.secrets.get(&key)? {
                entries.insert(key, value);
            }
        }
        Ok(entries)
    }
}

/// A database's consistent copy, made by SQLite through a connection of
/// its own while another (the recorder's) keeps it open.
fn snapshot_database(source: &Path, target: &Path) -> Result<()> {
    let db = Connection::open(source)?;
    db.busy_timeout(std::time::Duration::from_secs(30))?;
    let target = target.to_str().ok_or_else(|| anyhow!("The backup's working folder has an unusual name."))?;
    db.execute("VACUUM INTO ?1", [target])?;
    Ok(())
}

fn count(entries: &[(&str, PathBuf)]) -> BackupCounts {
    let path_of = |name: &str| entries.iter().find(|(entry, _)| *entry == name).map(|(_, path)| path.as_path());
    let mut counts = BackupCounts::default();
    let open = |path: &Path| Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok();
    let rows = |db: &Connection, sql: &str| db.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap_or(0);
    if let Some(db) = path_of("library.sqlite").and_then(open) {
        counts.watchlist = rows(&db, "SELECT COUNT(*) FROM watchlist");
        counts.progress = rows(&db, "SELECT COUNT(*) FROM progress");
        counts.history = rows(&db, "SELECT COUNT(*) FROM history");
        counts.favorites = rows(&db, "SELECT COUNT(*) FROM favorites");
        counts.lists = rows(&db, "SELECT COUNT(*) FROM favorite_lists");
        counts.item_edits = rows(&db, "SELECT COUNT(*) FROM item_edits");
    }
    if let Some(db) = path_of("recordings.sqlite").and_then(open) {
        counts.recordings = rows(&db, "SELECT COUNT(*) FROM recordings");
        counts.scheduled_recordings = rows(&db, "SELECT COUNT(*) FROM recordings WHERE status IN ('scheduled', 'recording')");
    }
    let json = |name: &str| path_of(name).and_then(|path| fs::read(path).ok()).and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    counts.sources = json("sources.json").and_then(|value| value.as_array().map(|list| list.len() as i64)).unwrap_or(0);
    counts.addons = json("addons.json").and_then(|value| value["addons"].as_array().map(|list| list.len() as i64)).unwrap_or(0);
    counts
}

fn write_archive(destination: &Path, manifest: &BackupManifest, entries: &[(&str, PathBuf)], sealed: Option<&[u8]>) -> Result<()> {
    let folder = destination.parent().filter(|folder| !folder.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let temporary = tempfile::Builder::new()
        .prefix(".xorvio-backup-")
        .suffix(".partial")
        .tempfile_in(folder)
        .map_err(|error| save_error(&error))?;
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(temporary);
    zip.start_file(MANIFEST, options)?;
    zip.write_all(&serde_json::to_vec_pretty(manifest)?)?;
    for (name, path) in entries {
        let size = fs::metadata(path)?.len();
        zip.start_file(*name, options.large_file(size >= u64::from(u32::MAX)))?;
        std::io::copy(&mut File::open(path)?, &mut zip).map_err(|error| save_error(&error))?;
    }
    if let Some(sealed) = sealed {
        zip.start_file(SEALED_SECRETS, options)?;
        zip.write_all(sealed)?;
    }
    let temporary = zip.finish()?;
    temporary.as_file().sync_all().map_err(|error| save_error(&error))?;
    temporary.persist(destination).map_err(|error| save_error(&error.error))?;
    Ok(())
}

fn save_error(error: &std::io::Error) -> anyhow::Error {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => anyhow!("Xorvio isn't allowed to save in that folder. Choose another one."),
        std::io::ErrorKind::StorageFull => anyhow!("There isn't enough space there for the backup."),
        std::io::ErrorKind::NotFound => anyhow!("The folder for the backup can't be found."),
        _ => anyhow!("The backup couldn't be saved ({error})."),
    }
}

// MARK: Reading a backup

/// What a backup holds, from its manifest.
pub fn inspect_backup(path: &Path) -> Result<BackupManifest> {
    read_manifest(&mut open_archive(path)?)
}

fn open_archive(path: &Path) -> Result<ZipArchive<File>> {
    let file = File::open(path).map_err(|_| anyhow!("The backup couldn't be opened. Check that it's still there."))?;
    ZipArchive::new(file).map_err(|_| anyhow!(NOT_A_BACKUP))
}

fn read_manifest(archive: &mut ZipArchive<File>) -> Result<BackupManifest> {
    let bytes = read_entry(archive, MANIFEST, MAX_MANIFEST_BYTES)?;
    let manifest: BackupManifest = serde_json::from_slice(&bytes).map_err(|_| anyhow!(NOT_A_BACKUP))?;
    anyhow::ensure!(manifest.app == APP, NOT_A_BACKUP);
    anyhow::ensure!(manifest.format <= FORMAT && manifest.library_schema <= SCHEMA_VERSION, NEWER);
    Ok(manifest)
}

fn read_entry(archive: &mut ZipArchive<File>, name: &str, limit: u64) -> Result<Vec<u8>> {
    let entry = archive.by_name(name).map_err(|_| anyhow!(NOT_A_BACKUP))?;
    anyhow::ensure!(entry.size() <= limit, NOT_A_BACKUP);
    let mut bytes = Vec::new();
    entry.take(limit).read_to_end(&mut bytes).map_err(|_| anyhow!(NOT_A_BACKUP))?;
    Ok(bytes)
}

// MARK: Restoring

/// What the restore staged, for the message after the restart.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StagedInfo {
    created_at: Timestamp,
    computer: String,
    logins: bool,
}

/// How a restore went, told once after the restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    pub restored: bool,
    pub message: String,
}

/// Checks a backup and stages it to replace this computer's library,
/// schedules, settings and sources at the next launch. With the passphrase
/// its logins come too; without one (or for a backup without them) they're
/// left as this computer has them. A wrong passphrase stops here, before
/// anything is written.
pub fn stage_restore(backup: &Path, data_dir: &Path, passphrase: Option<&str>, secrets: &dyn SecretStore) -> Result<BackupManifest> {
    let staging = data_dir.join(STAGING);
    let staged = stage(backup, data_dir, &staging, passphrase, secrets);
    if staged.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    staged
}

fn stage(backup: &Path, data_dir: &Path, staging: &Path, passphrase: Option<&str>, secrets: &dyn SecretStore) -> Result<BackupManifest> {
    let mut archive = open_archive(backup)?;
    let manifest = read_manifest(&mut archive)?;
    anyhow::ensure!(manifest.files.iter().any(|name| name == "library.sqlite"), NOT_A_BACKUP);
    let logins = match passphrase.filter(|passphrase| !passphrase.is_empty()) {
        Some(passphrase) if manifest.includes_secrets => {
            let sealed = read_entry(&mut archive, SEALED_SECRETS, MAX_SECRETS_BYTES)?;
            let plain = match seal::open_with_passphrase(&sealed, passphrase) {
                Ok(plain) => plain,
                Err(seal::SealError::WrongKey) => anyhow::bail!(WRONG_PASSPHRASE),
                Err(seal::SealError::Damaged) => anyhow::bail!(NOT_A_BACKUP),
            };
            serde_json::from_slice::<BTreeMap<String, String>>(&plain).map_err(|_| anyhow!(NOT_A_BACKUP))?;
            Some(plain)
        }
        _ => None,
    };

    if staging.exists() {
        fs::remove_dir_all(staging)?;
    }
    fs::create_dir_all(staging)?;
    for name in FILES.iter().filter(|name| manifest.files.iter().any(|file| file == *name)) {
        // Fixed names, never a path from inside the zip.
        let mut entry = archive.by_name(name).map_err(|_| anyhow!(NOT_A_BACKUP))?;
        let mut out = File::create(staging.join(name))?;
        let copied = std::io::copy(&mut (&mut entry).take(MAX_FILE_BYTES), &mut out).map_err(|_| anyhow!(NOT_A_BACKUP))?;
        anyhow::ensure!(copied == entry.size(), NOT_A_BACKUP);
        out.sync_all()?;
    }
    check_staged(staging)?;
    let key = match &logins {
        Some(plain) => {
            let key = seal::random_key()?;
            write_synced(&staging.join(SEALED_SECRETS), &seal::seal_with_key(plain, &key)?)?;
            Some(key)
        }
        None => None,
    };
    let info = StagedInfo { created_at: manifest.created_at, computer: manifest.computer.clone(), logins: key.is_some() };
    write_synced(&staging.join(STAGED_INFO), &serde_json::to_vec(&info)?)?;

    // An earlier restore that never ran gives way to this one.
    let pending = data_dir.join(PENDING);
    if pending.exists() {
        fs::remove_dir_all(&pending)?;
    }
    if let Some(key) = key {
        secrets.set(RESTORE_KEY, &seal::hex(&key))?;
    }
    if let Err(error) = fs::rename(staging, &pending) {
        if logins.is_some() {
            let _ = secrets.delete(RESTORE_KEY);
        }
        return Err(error.into());
    }
    Ok(manifest)
}

/// The staged files are what they claim: databases SQLite can read, from a
/// version of Xorvio this one understands, and JSON that parses.
fn check_staged(staging: &Path) -> Result<()> {
    for name in DATABASES {
        let path = staging.join(name);
        if !path.exists() {
            continue;
        }
        let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_| anyhow!(NOT_A_BACKUP))?;
        let check: String = db.query_row("PRAGMA quick_check", [], |row| row.get(0)).map_err(|_| anyhow!(NOT_A_BACKUP))?;
        anyhow::ensure!(check == "ok", NOT_A_BACKUP);
        if name == "library.sqlite" {
            let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
            anyhow::ensure!(version <= SCHEMA_VERSION, NEWER);
        }
    }
    for name in ["settings.json", "sources.json", "addons.json"] {
        let path = staging.join(name);
        if path.exists() {
            serde_json::from_slice::<serde_json::Value>(&fs::read(&path)?).map_err(|_| anyhow!(NOT_A_BACKUP))?;
        }
    }
    Ok(())
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Puts a staged restore in place. Call it before anything opens the
/// stores. Returns what to tell the person, or nothing when no restore was
/// waiting.
pub fn apply_pending_restore(data_dir: &Path, secrets: &dyn SecretStore) -> Option<RestoreOutcome> {
    remove_leftover_work(data_dir);
    let pending = data_dir.join(PENDING);
    let previous = data_dir.join(PREVIOUS);
    let finished = data_dir.join(FINISHED);
    if !pending.exists() {
        // What a finished restore had left to tidy when the app closed.
        if finished.exists() || previous.exists() {
            let _ = fs::remove_dir_all(&finished);
            let _ = fs::remove_dir_all(&previous);
            let _ = secrets.delete(RESTORE_KEY);
        }
        return None;
    }
    let info: StagedInfo = fs::read(pending.join(STAGED_INFO)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    if pending.join(APPLYING).exists() {
        // The app closed while the restore was being put in place.
        return Some(match roll_back(data_dir, secrets) {
            Ok(()) => RestoreOutcome {
                restored: false,
                message: "The last restore didn't finish, so Xorvio put everything back as it was. You can restore the backup again.".into(),
            },
            Err(error) => not_put_back(&error),
        });
    }
    match apply(data_dir, secrets) {
        Ok(()) => Some(RestoreOutcome { restored: true, message: restored_message(&info, sources_missing_logins(data_dir, secrets)) }),
        Err(error) => {
            tracing::error!("the restore couldn't be put in place: {error:#}");
            Some(match roll_back(data_dir, secrets) {
                Ok(()) => RestoreOutcome {
                    restored: false,
                    message: format!("The backup couldn't be restored ({error}), so everything is as it was."),
                },
                Err(undo) => not_put_back(&undo),
            })
        }
    }
}

/// Copies a backup was being made from, and a staging the app closed in
/// the middle of: at launch nothing is using them.
fn remove_leftover_work(data_dir: &Path) {
    let Ok(entries) = fs::read_dir(data_dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if (name.starts_with(".backup-") || name == STAGING) && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

fn not_put_back(error: &anyhow::Error) -> RestoreOutcome {
    tracing::error!("a restore couldn't be undone: {error:#}");
    RestoreOutcome {
        restored: false,
        message: format!(
            "A restore stopped part-way and Xorvio couldn't put everything back ({error}). Your earlier files are in the \"{PREVIOUS}\" folder in Xorvio's data folder; Xorvio tries again at the next launch."
        ),
    }
}

/// The files a restore moves: the backed-up ones and the databases' sidecars.
fn moved_names() -> Vec<String> {
    let mut names: Vec<String> = FILES.iter().map(|name| name.to_string()).collect();
    for database in DATABASES {
        names.extend(SIDECARS.iter().map(|suffix| format!("{database}{suffix}")));
    }
    names
}

/// How far a restore got, kept in `restore-pending/applying`: `aside`
/// while this computer's files are being set aside, then the list of those
/// that were (a JSON array) while the backup's go in.
fn mark_setting_aside(pending: &Path) -> Result<()> {
    write_synced(&pending.join(APPLYING), b"aside")
}

fn mark_set_aside(pending: &Path, names: &[String]) -> Result<()> {
    write_synced(&pending.join(APPLYING), &serde_json::to_vec(names)?)
}

/// The files that were set aside, once they all were.
fn set_aside(pending: &Path) -> Option<Vec<String>> {
    serde_json::from_slice(&fs::read(pending.join(APPLYING)).ok()?).ok()
}

fn apply(data_dir: &Path, secrets: &dyn SecretStore) -> Result<()> {
    let pending = data_dir.join(PENDING);
    let previous = data_dir.join(PREVIOUS);
    if previous.exists() {
        fs::remove_dir_all(&previous)?;
    }
    fs::create_dir_all(&previous)?;
    mark_setting_aside(&pending)?;
    let mut aside = Vec::new();
    for name in moved_names() {
        let here = data_dir.join(&name);
        if here.exists() {
            fs::rename(&here, previous.join(&name)).with_context(|| format!("{name} couldn't be set aside"))?;
            aside.push(name);
        }
    }
    mark_set_aside(&pending, &aside)?;
    for name in FILES {
        let staged = pending.join(name);
        if !staged.exists() {
            continue;
        }
        // Copied, so the staged file stays until the restore is done.
        let temporary = data_dir.join(format!("{name}.restoring"));
        fs::copy(&staged, &temporary).with_context(|| format!("{name} couldn't be put in place"))?;
        fs::OpenOptions::new().write(true).open(&temporary)?.sync_all()?;
        fs::rename(&temporary, data_dir.join(name)).with_context(|| format!("{name} couldn't be put in place"))?;
    }
    let sealed = pending.join(SEALED_SECRETS);
    if sealed.exists() {
        let key = restore_key(secrets)?;
        let plain = seal::open_with_key(&fs::read(&sealed)?, &key).map_err(|_| anyhow!("the backup's logins couldn't be opened"))?;
        let logins: BTreeMap<String, String> = serde_json::from_slice(&plain)?;
        // What the keychain had, sealed with the same key, to put back should
        // a later step fail.
        let mut before: BTreeMap<String, Option<String>> = BTreeMap::new();
        for key in logins.keys() {
            before.insert(key.clone(), secrets.get(key)?);
        }
        write_synced(&previous.join(SEALED_SECRETS), &seal::seal_with_key(&serde_json::to_vec(&before)?, &key)?)?;
        for (key, value) in &logins {
            secrets.set(key, value)?;
        }
    }
    // Done: this is the one step that can't be half-made.
    fs::rename(&pending, data_dir.join(FINISHED))?;
    let _ = fs::remove_dir_all(data_dir.join(FINISHED));
    let _ = fs::remove_dir_all(&previous);
    let _ = secrets.delete(RESTORE_KEY);
    Ok(())
}

fn restore_key(secrets: &dyn SecretStore) -> Result<[u8; seal::KEY_LEN]> {
    secrets
        .get(RESTORE_KEY)?
        .and_then(|text| seal::key_from_hex(&text))
        .ok_or_else(|| anyhow!("the backup's logins couldn't be read back from the keychain"))
}

/// Undoes a restore that stopped part-way: the backup's files go, this
/// computer's come back, and the keychain gets back what it had. Cut off,
/// it can run again: a file already back is left alone.
fn roll_back(data_dir: &Path, secrets: &dyn SecretStore) -> Result<()> {
    let pending = data_dir.join(PENDING);
    let previous = data_dir.join(PREVIOUS);
    let aside = set_aside(&pending);
    for name in moved_names() {
        let here = data_dir.join(&name);
        let kept = previous.join(&name);
        let _ = fs::remove_file(data_dir.join(format!("{name}.restoring")));
        if aside.as_ref().is_none_or(|aside| aside.contains(&name)) {
            // Setting aside was cut off (none of the backup's files are in
            // place yet), or this computer had this one: it goes back,
            // unless it's back already.
            if kept.exists() {
                fs::rename(&kept, &here).with_context(|| format!("{name} couldn't be put back"))?;
            }
        } else if here.exists() && pending.join(&name).exists() {
            // This computer had none; the one here is the backup's.
            fs::remove_file(&here).with_context(|| format!("{name} couldn't be removed"))?;
        }
    }
    // The earlier logins are sealed whole before the first one is changed,
    // so one that can't be read means none were.
    let kept_logins = previous.join(SEALED_SECRETS);
    let before = restore_key(secrets).ok().and_then(|key| {
        let plain = seal::open_with_key(&fs::read(&kept_logins).ok()?, &key).ok()?;
        serde_json::from_slice::<BTreeMap<String, Option<String>>>(&plain).ok()
    });
    for (key, value) in before.iter().flatten() {
        match value {
            Some(value) => secrets.set(key, value)?,
            None => secrets.delete(key)?,
        }
    }
    fs::remove_dir_all(&pending)?;
    let _ = fs::remove_dir_all(&previous);
    let _ = secrets.delete(RESTORE_KEY);
    Ok(())
}

/// Sources that sign in but have no login here, after a restore.
fn sources_missing_logins(data_dir: &Path, secrets: &dyn SecretStore) -> usize {
    let Some(sources) = fs::read(data_dir.join("sources.json")).ok().and_then(|bytes| serde_json::from_slice::<Vec<SourceConfig>>(&bytes).ok())
    else {
        return 0;
    };
    sources
        .iter()
        .filter(|source| matches!(source.kind, SourceKind::Xtream | SourceKind::M3uUrl | SourceKind::Plex | SourceKind::Jellyfin | SourceKind::Emby))
        .filter(|source| matches!(secrets.get(&source.id), Ok(None)))
        .count()
}

fn restored_message(info: &StagedInfo, missing_logins: usize) -> String {
    let made = Local.timestamp_opt(info.created_at, 0).single().map(|at| at.format("%-d %B %Y").to_string());
    let mut message = match (info.computer.trim(), made) {
        ("", Some(made)) => format!("Restored the backup from {made}."),
        (computer, Some(made)) => format!("Restored the backup made on {computer} on {made}."),
        _ => "Restored the backup.".to_string(),
    };
    match missing_logins {
        0 => {}
        1 => message.push_str(" One source needs its login again: open Sources to sign in."),
        count => message.push_str(&format!(" {count} sources need their logins again: open Sources to sign in.")),
    }
    message
}

// MARK: Automatic backups and names

/// A backup made by hand: "Xorvio Backup 2026-10-31.xorvio-backup".
pub fn manual_backup_name(at: DateTime<Local>) -> String {
    format!("Xorvio Backup {}.{BACKUP_EXTENSION}", at.format("%Y-%m-%d"))
}

/// An automatic one, named for its computer so several can share a folder
/// without one tidying away another's:
/// "Xorvio Backup 2026-10-31 09.30 (Studio Mac).xorvio-backup".
pub fn automatic_backup_name(computer: &str, at: DateTime<Local>) -> String {
    format!("Xorvio Backup {} ({}).{BACKUP_EXTENSION}", at.format(AUTOMATIC_TIME), file_safe(computer))
}

const AUTOMATIC_TIME: &str = "%Y-%m-%d %H.%M";

/// This computer's automatic backups in `folder`, newest first.
fn automatic_backups(folder: &Path, computer: &str) -> Vec<(NaiveDateTime, PathBuf)> {
    let suffix = format!(" ({}).{BACKUP_EXTENSION}", file_safe(computer));
    let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
    let mut found: Vec<(NaiveDateTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let when = name.strip_prefix("Xorvio Backup ")?.strip_suffix(&suffix)?;
            Some((NaiveDateTime::parse_from_str(when, AUTOMATIC_TIME).ok()?, entry.path()))
        })
        .collect();
    found.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
    found
}

/// When this computer last backed up automatically into `folder`.
pub fn last_automatic_backup(folder: &Path, computer: &str) -> Option<Timestamp> {
    let (when, _) = automatic_backups(folder, computer).into_iter().next()?;
    Local.from_local_datetime(&when).earliest().map(|at| at.timestamp())
}

/// Deletes this computer's automatic backups beyond the newest `keep`, and
/// says how many went. Backups made by hand are never touched.
pub fn prune_automatic_backups(folder: &Path, computer: &str, keep: usize) -> Result<usize> {
    let mut removed = 0;
    for (_, path) in automatic_backups(folder, computer).into_iter().skip(keep) {
        fs::remove_file(&path).with_context(|| format!("an old automatic backup couldn't be removed ({})", path.display()))?;
        removed += 1;
    }
    Ok(removed)
}

/// A name that's safe in a file name on every system.
fn file_safe(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|character| if character.is_control() || "/\\:*?\"<>|".contains(character) { '-' } else { character })
        .collect();
    let cleaned: String = cleaned.trim().trim_matches('.').chars().take(60).collect();
    if cleaned.trim().is_empty() { "Computer".into() } else { cleaned.trim().to_string() }
}

/// This computer's name as people know it ("Studio Mac"), or its host name.
pub fn computer_name() -> String {
    if let Some(name) = friendly_computer_name() {
        return name;
    }
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let host = host.strip_suffix(".local").unwrap_or(&host).trim().to_string();
    if host.is_empty() { "This computer".into() } else { host }
}

#[cfg(target_os = "macos")]
fn friendly_computer_name() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/scutil").args(["--get", "ComputerName"]).output().ok()?;
    let name = String::from_utf8(output.stdout).ok()?;
    (output.status.success() && !name.trim().is_empty()).then(|| name.trim().to_string())
}

#[cfg(not(target_os = "macos"))]
fn friendly_computer_name() -> Option<String> {
    None
}

#[cfg(test)]
mod tests;
