//! Backups and restores in temporary folders, with secrets in memory.
//! Nothing here touches the keychain or anyone's own data.

use std::sync::{Arc, Mutex};

use chrono::Duration;
use xorvio_model::{LiveStreamFormat, MediaKind, RefreshInterval, WatchlistEntry};

use super::*;
use crate::MemorySecrets;

const PASSWORD: &str = "hunter2-provider-password";
const TRAKT_TOKEN: &str = "trakt-access-token-value";
const PASSPHRASE: &str = "a long backup passphrase";

/// A computer's data folder as Xorvio leaves it: a library with one film on
/// the watchlist, a recordings database the recorder keeps open, settings,
/// one provider and the add-on list.
struct Computer {
    data: tempfile::TempDir,
    core: Option<Arc<Core>>,
    /// The recorder's connection, open while backups are made.
    recorder: Option<Connection>,
}

impl Computer {
    fn new(name: &str, secrets: Arc<dyn SecretStore>) -> Computer {
        let data = tempfile::tempdir().unwrap();
        let source = SourceConfig {
            id: "provider-1".into(),
            name: "Provider".into(),
            kind: SourceKind::Xtream,
            display_host: Some("tv.example".into()),
            file_path: None,
            folders: Vec::new(),
            is_paused: false,
            live_format: LiveStreamFormat::default(),
            refresh_interval: RefreshInterval::default(),
            user_agent: None,
            created_at: 1,
            sort_index: 0,
            guide_offset_minutes: 0,
            include_live_tv: true,
        };
        fs::write(data.path().join("sources.json"), serde_json::to_vec_pretty(&[source]).unwrap()).unwrap();
        fs::write(data.path().join("settings.json"), format!(r#"{{"launchScreen":"{name}"}}"#)).unwrap();
        fs::write(data.path().join("addons.json"), r#"{"version":1,"addons":[]}"#).unwrap();
        let recorder = Connection::open(data.path().join("recordings.sqlite")).unwrap();
        recorder
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE recordings (id TEXT PRIMARY KEY, title TEXT NOT NULL, status TEXT NOT NULL);",
            )
            .unwrap();
        recorder.execute("INSERT INTO recordings VALUES ('r1', ?1, 'scheduled'), ('r2', 'Old news', 'completed')", [name]).unwrap();
        let core = Core::open(data.path(), secrets).unwrap();
        core.set_watchlisted(film(&format!("{name}'s film")), true).unwrap();
        Computer { data, core: Some(core), recorder: Some(recorder) }
    }

    fn dir(&self) -> &Path {
        self.data.path()
    }

    fn core(&self) -> &Core {
        self.core.as_ref().unwrap()
    }

    /// The app quits (as it does to restore).
    fn quit(&mut self) {
        self.core = None;
        self.recorder = None;
    }

    fn back_up(&self, to: &Path, passphrase: Option<&str>) -> BackupManifest {
        let extra = vec!["addon:addon-link:a1".to_string()];
        let options = BackupOptions { data_dir: self.dir(), computer: "Desk", passphrase, extra_secrets: &extra, automatic: false };
        self.core().back_up(to, &options).unwrap()
    }

    /// What the library holds, read as the app would after a launch.
    fn titles(&self) -> Vec<String> {
        let core = Core::open(self.dir(), Arc::new(MemorySecrets::new())).unwrap();
        core.watchlist().unwrap().into_iter().map(|entry| entry.title).collect()
    }

    fn settings(&self) -> String {
        fs::read_to_string(self.dir().join("settings.json")).unwrap()
    }

    fn leftovers(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.dir())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("restore-") || name.ends_with(".restoring"))
            .collect();
        names.sort();
        names
    }
}

fn film(title: &str) -> WatchlistEntry {
    WatchlistEntry {
        id: format!("provider-1|mv|{title}"),
        kind: MediaKind::Movie,
        source_id: "provider-1".into(),
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

fn desk_secrets() -> Arc<MemorySecrets> {
    let secrets = Arc::new(MemorySecrets::new());
    secrets.set("provider-1", &format!(r#"{{"serverUrl":"http://tv.example","username":"desk","password":"{PASSWORD}"}}"#)).unwrap();
    secrets.set(crate::trakt::CONNECTION_KEY, &format!(r#"{{"accessToken":"{TRAKT_TOKEN}"}}"#)).unwrap();
    secrets.set(crate::tmdb::KEY_ENTRY, "tmdb-key-value").unwrap();
    secrets.set("addon:addon-link:a1", "https://addon.example/debrid-key/manifest.json").unwrap();
    secrets
}

/// Every file in a backup, uncompressed.
fn contents(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
    let names: Vec<String> = archive.file_names().map(|name| name.unwrap().into_owned()).collect();
    names
        .into_iter()
        .map(|name| {
            let mut bytes = Vec::new();
            archive.by_name(&name).unwrap().read_to_end(&mut bytes).unwrap();
            (name, bytes)
        })
        .collect()
}

fn holds(bytes: &[u8], text: &str) -> bool {
    bytes.windows(text.len()).any(|window| window == text.as_bytes())
}

#[test]
fn a_backup_without_logins_round_trips() {
    let out = tempfile::tempdir().unwrap();
    let desk = Computer::new("Desk", desk_secrets());
    let path = out.path().join(manual_backup_name(Local::now()));
    let manifest = desk.back_up(&path, None);
    assert!(!manifest.includes_secrets);
    assert_eq!(manifest.files, FILES);
    assert_eq!(manifest.computer, "Desk");
    assert_eq!((manifest.counts.sources, manifest.counts.watchlist), (1, 1));
    assert_eq!((manifest.counts.recordings, manifest.counts.scheduled_recordings), (2, 1));
    assert_eq!(inspect_backup(&path).unwrap(), manifest);
    for (name, bytes) in contents(&path) {
        for secret in [PASSWORD, TRAKT_TOKEN, "debrid-key"] {
            assert!(!holds(&bytes, secret), "{name} holds a login");
        }
    }
    let partials = fs::read_dir(out.path()).unwrap().count();
    assert_eq!(partials, 1, "nothing but the backup is left beside it");

    let laptop_secrets = Arc::new(MemorySecrets::new());
    let mut laptop = Computer::new("Laptop", laptop_secrets.clone());
    laptop.quit();
    stage_restore(&path, laptop.dir(), None, laptop_secrets.as_ref()).unwrap();
    assert_eq!(laptop.titles(), ["Laptop's film"], "nothing changes until the app restarts");
    let outcome = apply_pending_restore(laptop.dir(), laptop_secrets.as_ref()).unwrap();
    assert!(outcome.restored, "{outcome:?}");
    assert!(outcome.message.starts_with("Restored the backup made on Desk on "), "{}", outcome.message);
    assert!(outcome.message.ends_with("One source needs its login again: open Sources to sign in."), "{}", outcome.message);
    assert_eq!(laptop.titles(), ["Desk's film"]);
    assert_eq!(laptop.settings(), r#"{"launchScreen":"Desk"}"#);
    let recordings = Connection::open(laptop.dir().join("recordings.sqlite")).unwrap();
    let title: String = recordings.query_row("SELECT title FROM recordings WHERE id = 'r1'", [], |row| row.get(0)).unwrap();
    assert_eq!(title, "Desk");
    assert!(laptop.dir().join("catalog.sqlite").exists(), "the catalog stays");
    assert!(laptop.leftovers().is_empty(), "{:?}", laptop.leftovers());
    assert!(apply_pending_restore(laptop.dir(), laptop_secrets.as_ref()).is_none(), "it happens once");
}

#[test]
fn a_backup_with_logins_brings_them_back() {
    let out = tempfile::tempdir().unwrap();
    let desk = Computer::new("Desk", desk_secrets());
    let path = out.path().join("with logins.xorvio-backup");
    assert!(desk.back_up(&path, Some(PASSPHRASE)).includes_secrets);
    for (name, bytes) in contents(&path) {
        for secret in [PASSWORD, TRAKT_TOKEN, "debrid-key", "tmdb-key-value", PASSPHRASE] {
            assert!(!holds(&bytes, secret), "{name} holds something in the clear");
        }
    }

    let laptop_secrets = Arc::new(MemorySecrets::new());
    laptop_secrets.set("provider-1", "the laptop's own login").unwrap();
    let mut laptop = Computer::new("Laptop", laptop_secrets.clone());
    laptop.quit();
    stage_restore(&path, laptop.dir(), Some(PASSPHRASE), laptop_secrets.as_ref()).unwrap();
    let staged = fs::read(laptop.dir().join(PENDING).join(SEALED_SECRETS)).unwrap();
    assert!(!holds(&staged, PASSWORD), "staged logins are sealed too");
    assert_eq!(laptop_secrets.get("provider-1").unwrap().as_deref(), Some("the laptop's own login"), "untouched until the restart");

    let outcome = apply_pending_restore(laptop.dir(), laptop_secrets.as_ref()).unwrap();
    assert!(outcome.restored && !outcome.message.contains("login"), "{outcome:?}");
    assert!(laptop_secrets.get("provider-1").unwrap().unwrap().contains(PASSWORD));
    assert!(laptop_secrets.get(crate::trakt::CONNECTION_KEY).unwrap().unwrap().contains(TRAKT_TOKEN));
    assert_eq!(laptop_secrets.get(crate::tmdb::KEY_ENTRY).unwrap().as_deref(), Some("tmdb-key-value"));
    assert!(laptop_secrets.get("addon:addon-link:a1").unwrap().is_some());
    assert_eq!(laptop_secrets.get(RESTORE_KEY).unwrap(), None, "the staging key goes");
    assert_eq!(laptop.titles(), ["Desk's film"]);
    assert!(laptop.leftovers().is_empty());
}

#[test]
fn a_wrong_passphrase_is_turned_away_plainly() {
    let out = tempfile::tempdir().unwrap();
    let desk = Computer::new("Desk", desk_secrets());
    let path = out.path().join("b.xorvio-backup");
    desk.back_up(&path, Some(PASSPHRASE));
    let laptop_secrets = Arc::new(MemorySecrets::new());
    let mut laptop = Computer::new("Laptop", laptop_secrets.clone());
    laptop.quit();

    let error = stage_restore(&path, laptop.dir(), Some("not the passphrase"), laptop_secrets.as_ref()).unwrap_err();
    assert_eq!(error.to_string(), WRONG_PASSPHRASE);
    assert!(laptop.leftovers().is_empty(), "nothing staged: {:?}", laptop.leftovers());
    assert!(!laptop.dir().join(STAGING).exists());
    assert_eq!(laptop_secrets.get(RESTORE_KEY).unwrap(), None);
    assert!(apply_pending_restore(laptop.dir(), laptop_secrets.as_ref()).is_none());
    assert_eq!(laptop.titles(), ["Laptop's film"]);

    // No passphrase: everything but the logins.
    stage_restore(&path, laptop.dir(), Some(""), laptop_secrets.as_ref()).unwrap();
    assert!(apply_pending_restore(laptop.dir(), laptop_secrets.as_ref()).unwrap().restored);
    assert_eq!(laptop_secrets.get("provider-1").unwrap(), None);
    assert_eq!(laptop.titles(), ["Desk's film"]);

    // Too short a passphrase isn't accepted for a backup.
    let options = BackupOptions { data_dir: desk.dir(), computer: "Desk", passphrase: Some("short"), extra_secrets: &[], automatic: false };
    let error = desk.core().back_up(&out.path().join("short.xorvio-backup"), &options).unwrap_err();
    assert_eq!(error.to_string(), "Choose a passphrase of at least 8 characters.");
}

#[test]
fn a_restore_that_fails_part_way_puts_everything_back() {
    let out = tempfile::tempdir().unwrap();
    let desk = Computer::new("Desk", desk_secrets());
    let path = out.path().join("b.xorvio-backup");
    desk.back_up(&path, None);
    let secrets = Arc::new(MemorySecrets::new());
    let mut laptop = Computer::new("Laptop", secrets.clone());
    laptop.quit();
    stage_restore(&path, laptop.dir(), None, secrets.as_ref()).unwrap();
    // The library and recordings go in, then settings can't.
    let staged = laptop.dir().join(PENDING).join("settings.json");
    fs::remove_file(&staged).unwrap();
    fs::create_dir(&staged).unwrap();

    let outcome = apply_pending_restore(laptop.dir(), secrets.as_ref()).unwrap();
    assert!(!outcome.restored);
    assert!(outcome.message.starts_with("The backup couldn't be restored (settings.json couldn't be put in place)"), "{}", outcome.message);
    assert!(outcome.message.ends_with("so everything is as it was."));
    assert_eq!(laptop.titles(), ["Laptop's film"]);
    assert_eq!(laptop.settings(), r#"{"launchScreen":"Laptop"}"#);
    assert!(laptop.leftovers().is_empty(), "{:?}", laptop.leftovers());
}

/// A keychain that turns down one write, as a locked keychain might.
struct FlakyKeychain {
    inner: MemorySecrets,
    writes: Mutex<usize>,
    refuse: Mutex<Option<usize>>,
}

impl SecretStore for FlakyKeychain {
    fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        self.inner.get(key)
    }
    fn set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let mut writes = self.writes.lock().unwrap();
        *writes += 1;
        if *self.refuse.lock().unwrap() == Some(*writes) {
            anyhow::bail!("The keychain could not be written (locked).");
        }
        self.inner.set(key, value)
    }
    fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.inner.delete(key)
    }
}

#[test]
fn a_keychain_failure_part_way_puts_the_logins_back_too() {
    let out = tempfile::tempdir().unwrap();
    let desk = Computer::new("Desk", desk_secrets());
    let path = out.path().join("b.xorvio-backup");
    desk.back_up(&path, Some(PASSPHRASE));
    let keychain = Arc::new(FlakyKeychain { inner: MemorySecrets::new(), writes: Mutex::new(0), refuse: Mutex::new(None) });
    keychain.set("provider-1", "the laptop's own login").unwrap();
    let mut laptop = Computer::new("Laptop", keychain.clone());
    laptop.quit();
    stage_restore(&path, laptop.dir(), Some(PASSPHRASE), keychain.as_ref()).unwrap();
    // The logins go in name order: the add-on link, then the provider's,
    // which is turned down.
    let writes = *keychain.writes.lock().unwrap();
    *keychain.refuse.lock().unwrap() = Some(writes + 2);

    let outcome = apply_pending_restore(laptop.dir(), keychain.as_ref()).unwrap();
    assert!(!outcome.restored);
    assert!(outcome.message.contains("locked"), "{}", outcome.message);
    assert_eq!(keychain.get("provider-1").unwrap().as_deref(), Some("the laptop's own login"));
    assert_eq!(keychain.get("addon:addon-link:a1").unwrap(), None, "what the restore added is gone again");
    assert_eq!(keychain.get(crate::tmdb::KEY_ENTRY).unwrap(), None);
    assert_eq!(keychain.get(RESTORE_KEY).unwrap(), None);
    assert_eq!(laptop.titles(), ["Laptop's film"]);
    assert!(laptop.leftovers().is_empty(), "{:?}", laptop.leftovers());
}

#[test]
fn a_restore_cut_off_by_the_app_closing_is_undone_at_the_next_launch() {
    let out = tempfile::tempdir().unwrap();
    let desk = Computer::new("Desk", desk_secrets());
    let path = out.path().join("b.xorvio-backup");
    desk.back_up(&path, None);
    let secrets = Arc::new(MemorySecrets::new());
    let mut laptop = Computer::new("Laptop", secrets.clone());
    laptop.quit();
    let (dir, pending, previous) = (laptop.dir().to_path_buf(), laptop.dir().join(PENDING), laptop.dir().join(PREVIOUS));

    // Closed while setting the laptop's files aside: only the library went.
    stage_restore(&path, &dir, None, secrets.as_ref()).unwrap();
    fs::create_dir_all(&previous).unwrap();
    mark_setting_aside(&pending).unwrap();
    for name in ["library.sqlite", "library.sqlite-wal", "library.sqlite-shm"] {
        if dir.join(name).exists() {
            fs::rename(dir.join(name), previous.join(name)).unwrap();
        }
    }
    let outcome = apply_pending_restore(&dir, secrets.as_ref()).unwrap();
    assert!(!outcome.restored);
    assert!(outcome.message.starts_with("The last restore didn't finish"), "{}", outcome.message);
    assert_eq!(laptop.titles(), ["Laptop's film"]);
    assert_eq!(laptop.settings(), r#"{"launchScreen":"Laptop"}"#);
    assert!(laptop.leftovers().is_empty());

    // Closed while putting the backup's files in place, on a laptop that had
    // no add-ons file: the backup's goes, the laptop's library comes back.
    fs::remove_file(dir.join("addons.json")).unwrap();
    stage_restore(&path, &dir, None, secrets.as_ref()).unwrap();
    fs::create_dir_all(&previous).unwrap();
    let mut aside = Vec::new();
    for name in moved_names() {
        if dir.join(&name).exists() {
            fs::rename(dir.join(&name), previous.join(&name)).unwrap();
            aside.push(name);
        }
    }
    mark_set_aside(&pending, &aside).unwrap();
    for name in ["library.sqlite", "addons.json"] {
        fs::copy(pending.join(name), dir.join(name)).unwrap();
    }
    // And the app closed again while undoing it, after the library was back.
    fs::rename(previous.join("library.sqlite"), dir.join("library.sqlite")).unwrap();
    let outcome = apply_pending_restore(&dir, secrets.as_ref()).unwrap();
    assert!(outcome.message.starts_with("The last restore didn't finish"), "{}", outcome.message);
    assert_eq!(laptop.titles(), ["Laptop's film"]);
    assert!(!dir.join("addons.json").exists(), "the backup's add-ons file is gone");
    assert_eq!(laptop.settings(), r#"{"launchScreen":"Laptop"}"#);
    assert!(laptop.leftovers().is_empty());

    // After all that, restoring works.
    stage_restore(&path, &dir, None, secrets.as_ref()).unwrap();
    assert!(apply_pending_restore(&dir, secrets.as_ref()).unwrap().restored);
    assert_eq!(laptop.titles(), ["Desk's film"]);
}

#[test]
fn leftovers_of_a_finished_restore_are_tidied_at_launch() {
    let data = tempfile::tempdir().unwrap();
    let secrets = MemorySecrets::new();
    fs::create_dir_all(data.path().join(FINISHED)).unwrap();
    fs::create_dir_all(data.path().join(PREVIOUS)).unwrap();
    // A backup's working copy and a half-made staging, from an app that was closed.
    fs::create_dir_all(data.path().join(".backup-x1y2")).unwrap();
    fs::write(data.path().join(".backup-x1y2").join("library.sqlite"), "copy").unwrap();
    fs::create_dir_all(data.path().join(STAGING)).unwrap();
    fs::write(data.path().join("library.sqlite"), "kept").unwrap();
    secrets.set(RESTORE_KEY, "00").unwrap();
    assert!(apply_pending_restore(data.path(), &secrets).is_none());
    assert!(!data.path().join(FINISHED).exists() && !data.path().join(PREVIOUS).exists());
    assert!(!data.path().join(".backup-x1y2").exists() && !data.path().join(STAGING).exists());
    assert_eq!(fs::read_to_string(data.path().join("library.sqlite")).unwrap(), "kept");
    assert_eq!(secrets.get(RESTORE_KEY).unwrap(), None);
}

#[test]
fn things_that_arent_backups_are_turned_away() {
    let out = tempfile::tempdir().unwrap();
    let text = out.path().join("notes.xorvio-backup");
    fs::write(&text, "just some text").unwrap();
    assert_eq!(inspect_backup(&text).unwrap_err().to_string(), NOT_A_BACKUP);

    let write_zip = |name: &str, manifest: Option<&str>| {
        let path = out.path().join(name);
        let mut zip = ZipWriter::new(File::create(&path).unwrap());
        zip.start_file("library.sqlite", SimpleFileOptions::default()).unwrap();
        zip.write_all(b"SQLite format 3\0").unwrap();
        if let Some(manifest) = manifest {
            zip.start_file(MANIFEST, SimpleFileOptions::default()).unwrap();
            zip.write_all(manifest.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        path
    };
    let no_manifest = write_zip("plain.xorvio-backup", None);
    assert_eq!(inspect_backup(&no_manifest).unwrap_err().to_string(), NOT_A_BACKUP);
    let newer = write_zip(
        "newer.xorvio-backup",
        Some(r#"{"format":99,"app":"Xorvio","appVersion":"9.0","createdAt":1,"computer":"Desk","librarySchema":99,"files":["library.sqlite"],"includesSecrets":false}"#),
    );
    assert_eq!(inspect_backup(&newer).unwrap_err().to_string(), NEWER);
    let damaged = write_zip(
        "damaged.xorvio-backup",
        Some(r#"{"format":1,"app":"Xorvio","appVersion":"0.1","createdAt":1,"computer":"Desk","librarySchema":1,"files":["library.sqlite"],"includesSecrets":false}"#),
    );
    let data = tempfile::tempdir().unwrap();
    let error = stage_restore(&damaged, data.path(), None, &MemorySecrets::new()).unwrap_err();
    assert_eq!(error.to_string(), NOT_A_BACKUP, "a library that isn't one is caught before the restart");
    assert!(!data.path().join(PENDING).exists() && !data.path().join(STAGING).exists());
}

#[test]
fn automatic_backups_keep_the_latest_seven_of_this_computer() {
    let folder = tempfile::tempdir().unwrap();
    let start = Local.with_ymd_and_hms(2026, 3, 1, 9, 30, 0).single().unwrap();
    for day in 0..10 {
        fs::write(folder.path().join(automatic_backup_name("Desk", start + Duration::days(day))), "").unwrap();
    }
    for day in 0..2 {
        fs::write(folder.path().join(automatic_backup_name("Laptop", start + Duration::days(day))), "").unwrap();
    }
    fs::write(folder.path().join(manual_backup_name(start)), "").unwrap();
    fs::write(folder.path().join("notes.txt"), "").unwrap();

    assert_eq!(last_automatic_backup(folder.path(), "Desk"), Some((start + Duration::days(9)).timestamp()));
    assert_eq!(prune_automatic_backups(folder.path(), "Desk", AUTOMATIC_KEEP).unwrap(), 3);
    let desk: Vec<String> = automatic_backups(folder.path(), "Desk").into_iter().map(|(when, _)| when.format("%d").to_string()).collect();
    assert_eq!(desk, ["10", "09", "08", "07", "06", "05", "04"], "the newest seven");
    assert_eq!(automatic_backups(folder.path(), "Laptop").len(), 2, "another computer's are left alone");
    assert!(folder.path().join(manual_backup_name(start)).exists(), "one made by hand is never removed");
    assert_eq!(fs::read_dir(folder.path()).unwrap().count(), 11);

    let odd = automatic_backup_name("Rich's Mac/Book: \"Pro\"", start);
    assert_eq!(odd, "Xorvio Backup 2026-03-01 09.30 (Rich's Mac-Book- -Pro-).xorvio-backup");
    assert_eq!(automatic_backup_name("  ", start), "Xorvio Backup 2026-03-01 09.30 (Computer).xorvio-backup");
}

#[test]
fn an_automatic_backup_never_holds_logins() {
    let out = tempfile::tempdir().unwrap();
    let desk = Computer::new("Desk", desk_secrets());
    let path = out.path().join(automatic_backup_name("Desk", Local::now()));
    let options = BackupOptions { data_dir: desk.dir(), computer: "Desk", passphrase: Some(PASSPHRASE), extra_secrets: &[], automatic: true };
    let manifest = desk.core().back_up(&path, &options).unwrap();
    assert!(manifest.automatic && !manifest.includes_secrets);
    assert!(contents(&path).iter().all(|(name, _)| name != SEALED_SECRETS));
}
