//! Settings › Backup and Settings › Sync: backing up now, restoring (staged,
//! then a restart, then put in place before anything opens the data),
//! automatic backups, and syncing with other computers through a shared
//! folder, the last two from background loops. The work itself is in
//! xorvio-core's backup and sync modules; this is when it runs and what the
//! interface hears.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use xorvio_core::SecretStore;
use xorvio_core::backup::{self, BACKUP_EXTENSION, BackupManifest, BackupOptions, RestoreOutcome};
use xorvio_core::sync::{SyncComputer, SyncState};
use xorvio_model::{Recording, RecordingStatus};

use crate::AppState;

type CmdResult<T> = Result<T, String>;

fn fail(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// How often the sync runs when nothing has changed.
const SYNC_EVERY: Duration = Duration::from_secs(10 * 60);
/// After a change, how long the library has to stay quiet before it syncs
/// (watching saves progress every few seconds), and the longest a change
/// waits however busy it stays.
const SYNC_QUIET: Duration = Duration::from_secs(45);
const SYNC_AT_MOST_AFTER: Duration = Duration::from_secs(5 * 60);
/// How often the sync loop looks around.
const SYNC_TICK: Duration = Duration::from_secs(10);
/// How often the automatic backup checks whether one is due.
const BACKUP_TICK: Duration = Duration::from_secs(15 * 60);

/// What the backup and sync side of the app keeps, managed beside AppState.
pub struct BackupService {
    data_dir: PathBuf,
    secrets: Arc<dyn SecretStore>,
    /// This computer's name, for backups and the other computers.
    computer: String,
    sync: Mutex<SyncState>,
    /// One sync at a time, from the loop or Sync Now.
    sync_turn: tokio::sync::Mutex<()>,
    syncing: AtomicBool,
    automatic_error: Mutex<Option<String>>,
    /// How the restore made before this launch went, told once.
    restored: Mutex<Option<RestoreOutcome>>,
}

impl BackupService {
    pub fn new(data_dir: PathBuf, secrets: Arc<dyn SecretStore>, restored: Option<RestoreOutcome>) -> BackupService {
        let sync = SyncState::load(&data_dir);
        BackupService {
            computer: backup::computer_name(),
            data_dir,
            secrets,
            sync: Mutex::new(sync),
            sync_turn: tokio::sync::Mutex::new(()),
            syncing: AtomicBool::new(false),
            automatic_error: Mutex::new(None),
            restored: Mutex::new(restored),
        }
    }
}

/// Puts a restore staged before the restart in place. Call it before
/// anything opens the data folder.
pub fn apply_pending_restore(data_dir: &Path, secrets: &dyn SecretStore) -> Option<RestoreOutcome> {
    let outcome = backup::apply_pending_restore(data_dir, secrets)?;
    if outcome.restored {
        tracing::info!("a backup was restored");
    } else {
        tracing::warn!("a restore didn't go ahead: {}", outcome.message);
    }
    Some(outcome)
}

/// Starts automatic backups and syncing.
pub fn spawn(app: AppHandle) {
    spawn_automatic_backups(app.clone());
    spawn_sync(app);
}

/// Where automatic backups go: the folder chosen, or Backups in the data folder.
fn backups_folder(settings: &crate::Settings, data_dir: &Path) -> (PathBuf, bool) {
    match &settings.auto_backup_folder {
        Some(folder) => (PathBuf::from(folder), false),
        None => (data_dir.join("Backups"), true),
    }
}

/// The keychain entries holding add-on links: the add-on crate's key for
/// each, under the prefix `commands::addon_secrets` files them with.
fn addon_link_keys(state: &AppState) -> Vec<String> {
    state.addons.list().into_iter().map(|addon| format!("addon:{}", xorvio_addons::secret_key(&addon.id))).collect()
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// MARK: - Backing up and restoring

/// Settings › Backup.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStatus {
    computer_name: String,
    /// Where automatic backups go.
    automatic_folder: String,
    last_automatic: Option<i64>,
    /// Why the last automatic backup didn't happen, in plain words.
    automatic_error: Option<String>,
    /// How the restore before this launch went.
    restore_notice: Option<RestoreOutcome>,
    min_passphrase_length: usize,
}

#[tauri::command]
pub async fn backup_status(state: State<'_, AppState>, service: State<'_, BackupService>) -> CmdResult<BackupStatus> {
    let (folder, _) = backups_folder(&state.settings.lock().unwrap(), &service.data_dir);
    let computer = service.computer.clone();
    let last_automatic = {
        let (folder, computer) = (folder.clone(), computer.clone());
        tokio::task::spawn_blocking(move || backup::last_automatic_backup(&folder, &computer)).await.map_err(fail)?
    };
    Ok(BackupStatus {
        computer_name: computer,
        automatic_folder: folder.to_string_lossy().into_owned(),
        last_automatic,
        automatic_error: service.automatic_error.lock().unwrap().clone(),
        restore_notice: service.restored.lock().unwrap().clone(),
        min_passphrase_length: backup::MIN_PASSPHRASE_CHARS,
    })
}

/// Backs up now, to a file the person names. With a passphrase, logins and
/// links go in too, sealed with it. `None` when they cancel.
#[tauri::command]
pub async fn back_up_now(app: AppHandle, state: State<'_, AppState>, service: State<'_, BackupService>, passphrase: Option<String>) -> CmdResult<Option<String>> {
    let passphrase = passphrase.filter(|passphrase| !passphrase.is_empty());
    if passphrase.as_ref().is_some_and(|passphrase| passphrase.chars().count() < backup::MIN_PASSPHRASE_CHARS) {
        return Err(format!("Choose a passphrase of at least {} characters.", backup::MIN_PASSPHRASE_CHARS));
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name(backup::manual_backup_name(chrono::Local::now()))
        .add_filter("Xorvio Backup", &[BACKUP_EXTENSION])
        .save_file(move |file| {
            let _ = tx.send(file.and_then(|f| f.into_path().ok()));
        });
    let Some(mut path) = rx.await.map_err(fail)? else { return Ok(None) };
    if path.extension().and_then(|extension| extension.to_str()) != Some(BACKUP_EXTENSION) {
        path.as_mut_os_string().push(format!(".{BACKUP_EXTENSION}"));
    }
    let core = state.core.clone();
    let (data_dir, computer, extra) = (service.data_dir.clone(), service.computer.clone(), addon_link_keys(&state));
    let saved = path.clone();
    tokio::task::spawn_blocking(move || {
        let options = BackupOptions { data_dir: &data_dir, computer: &computer, passphrase: passphrase.as_deref(), extra_secrets: &extra, automatic: false };
        core.back_up(&saved, &options)
    })
    .await
    .map_err(fail)?
    .map_err(fail)?;
    tracing::info!("backed up by hand");
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// A backup picked to restore, with what this computer has that it replaces.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    path: String,
    file_name: String,
    #[serde(flatten)]
    manifest: BackupManifest,
    /// Recordings running here now; a restore waits for them.
    recording_now: usize,
    /// Recordings scheduled here, which the backup's schedule replaces.
    scheduled_here: usize,
}

/// Asks for a backup to restore and says what's in it; `None` when they cancel.
#[tauri::command]
pub async fn choose_backup(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Option<BackupInfo>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().add_filter("Xorvio Backup", &[BACKUP_EXTENSION]).pick_file(move |file| {
        let _ = tx.send(file.and_then(|f| f.into_path().ok()));
    });
    let Some(path) = rx.await.map_err(fail)? else { return Ok(None) };
    let read = path.clone();
    let manifest = tokio::task::spawn_blocking(move || backup::inspect_backup(&read)).await.map_err(fail)?.map_err(fail)?;
    let recordings = state.recorder.list().unwrap_or_default();
    let count = |status: RecordingStatus| recordings.iter().filter(|recording| recording.status == status).count();
    Ok(Some(BackupInfo {
        file_name: path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
        path: path.to_string_lossy().into_owned(),
        manifest,
        recording_now: count(RecordingStatus::Recording),
        scheduled_here: count(RecordingStatus::Scheduled),
    }))
}

/// Stages a backup to replace this computer's library, schedules, settings
/// and sources, then restarts Xorvio to put it in place. The passphrase
/// opens its logins; without one they stay as this computer has them.
#[tauri::command]
pub async fn restore_backup(app: AppHandle, state: State<'_, AppState>, service: State<'_, BackupService>, path: String, passphrase: Option<String>) -> CmdResult<()> {
    let recording = state.recorder.active_count() > 0
        || state.recorder.list().unwrap_or_default().iter().any(|recording| recording.status == RecordingStatus::Recording);
    if recording {
        return Err("Xorvio is recording right now. Restore once the recording has finished, or stop it in Recordings.".into());
    }
    let (data_dir, secrets) = (service.data_dir.clone(), service.secrets.clone());
    tokio::task::spawn_blocking(move || backup::stage_restore(Path::new(&path), &data_dir, passphrase.as_deref(), secrets.as_ref()))
        .await
        .map_err(fail)?
        .map_err(fail)?;
    tracing::info!("a backup is staged; restarting to restore it");
    // The exit runs as a quit does: live buffers and the gateway are let go.
    app.request_restart();
    Ok(())
}

/// Opens the folder automatic backups go to.
#[tauri::command]
pub async fn show_backups_folder(app: AppHandle, state: State<'_, AppState>, service: State<'_, BackupService>) -> CmdResult<()> {
    let (folder, default) = backups_folder(&state.settings.lock().unwrap(), &service.data_dir);
    if default {
        std::fs::create_dir_all(&folder).map_err(|e| format!("The backups folder couldn't be made: {e}"))?;
    }
    app.opener().open_path(folder.to_string_lossy(), None::<&str>).map_err(fail)
}

/// Backs up a few minutes after launch and then every quarter of an hour,
/// when a backup is due.
fn spawn_automatic_backups(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        loop {
            automatic_backup_if_due(&app).await;
            tokio::time::sleep(BACKUP_TICK).await;
        }
    });
}

async fn automatic_backup_if_due(app: &AppHandle) {
    let (state, service) = (app.state::<AppState>(), app.state::<BackupService>());
    let (every, (folder, default)) = {
        let settings = state.settings.lock().unwrap();
        (settings.auto_backup.every(), backups_folder(&settings, &service.data_dir))
    };
    let Some(every) = every else {
        *service.automatic_error.lock().unwrap() = None;
        return;
    };
    let computer = service.computer.clone();
    let (last_folder, last_computer) = (folder.clone(), computer.clone());
    let last = tokio::task::spawn_blocking(move || backup::last_automatic_backup(&last_folder, &last_computer)).await.ok().flatten();
    if last.is_some_and(|last| now() - last < every) {
        return;
    }
    // A recording getting under way gets the disk and the processor to itself.
    if recording_starting(&state.recorder.list().unwrap_or_default(), now()) {
        tracing::debug!("the automatic backup waits for a recording to get going");
        return;
    }
    let core = state.core.clone();
    let data_dir = service.data_dir.clone();
    let made = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
        if default {
            std::fs::create_dir_all(&folder)?;
        } else if !folder.is_dir() {
            anyhow::bail!("The folder for automatic backups can't be found. Choose it again, or check that its drive is connected.");
        }
        let path = folder.join(backup::automatic_backup_name(&computer, chrono::Local::now()));
        let options = BackupOptions { data_dir: &data_dir, computer: &computer, passphrase: None, extra_secrets: &[], automatic: true };
        core.back_up(&path, &options)?;
        backup::prune_automatic_backups(&folder, &computer, backup::AUTOMATIC_KEEP)
    })
    .await
    .map_err(anyhow::Error::from)
    .and_then(|made| made);
    match made {
        Ok(removed) => {
            tracing::info!("backed up automatically{}", if removed > 0 { format!(", removing {removed} older") } else { String::new() });
            *service.automatic_error.lock().unwrap() = None;
        }
        Err(error) => {
            tracing::warn!("the automatic backup couldn't be made: {error}");
            *service.automatic_error.lock().unwrap() = Some(error.to_string());
        }
    }
    let _ = app.emit("backup-changed", ());
}

/// A recording starts within ten minutes or started in the last two.
fn recording_starting(recordings: &[Recording], now: i64) -> bool {
    recordings.iter().any(|recording| {
        let begins = recording.start - recording.padding_before_seconds;
        matches!(recording.status, RecordingStatus::Scheduled | RecordingStatus::Recording) && (now - 120..=now + 600).contains(&begins)
    })
}

// MARK: - Sync

/// Settings › Sync.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    enabled: bool,
    folder: Option<String>,
    syncing: bool,
    last_sync: Option<i64>,
    last_error: Option<String>,
    notice: Option<String>,
    this_computer: String,
    computers: Vec<SyncComputer>,
}

fn current_sync_status(state: &AppState, service: &BackupService) -> SyncStatus {
    let (enabled, folder) = {
        let settings = state.settings.lock().unwrap();
        (settings.sync_enabled, settings.sync_folder.clone())
    };
    let sync = service.sync.lock().unwrap();
    // Another folder's computers and errors aren't this one's.
    let same_folder = folder.is_some() && sync.folder == folder;
    SyncStatus {
        enabled,
        syncing: service.syncing.load(Ordering::Relaxed),
        last_sync: sync.last_sync.filter(|_| same_folder),
        last_error: sync.last_error.clone().filter(|_| same_folder || folder.is_none()),
        notice: sync.notice.clone().filter(|_| same_folder),
        this_computer: service.computer.clone(),
        computers: if same_folder { sync.computers_seen() } else { Vec::new() },
        folder,
    }
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>, service: State<'_, BackupService>) -> CmdResult<SyncStatus> {
    Ok(current_sync_status(&state, &service))
}

/// Syncs at once and says how it went.
#[tauri::command]
pub async fn sync_now(app: AppHandle) -> CmdResult<SyncStatus> {
    run_sync(&app).await;
    Ok(current_sync_status(&app.state::<AppState>(), &app.state::<BackupService>()))
}

/// Syncs on launch, every ten minutes, and after changes once the library
/// has been quiet a little while. Turning sync on, or choosing another
/// folder, syncs within a few seconds.
fn spawn_sync(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let mut last_sync: Option<Instant> = None;
        let mut last_settings: Option<(bool, Option<String>)> = None;
        let mut synced_changes: Option<i64> = None;
        let mut seen_changes: Option<i64> = None;
        let mut first_change: Option<Instant> = None;
        let mut last_change = Instant::now();
        loop {
            let state = app.state::<AppState>();
            let settings = {
                let settings = state.settings.lock().unwrap();
                (settings.sync_enabled, settings.sync_folder.clone())
            };
            let changes = state.core.sync_change_count();
            if seen_changes != Some(changes) {
                seen_changes = Some(changes);
                last_change = Instant::now();
                if synced_changes.is_some_and(|synced| synced != changes) {
                    first_change.get_or_insert_with(Instant::now);
                }
            }
            let due = settings.0
                && settings.1.is_some()
                && (last_settings.as_ref() != Some(&settings)
                    || last_sync.is_none_or(|at| at.elapsed() >= SYNC_EVERY)
                    || first_change.is_some_and(|first| last_change.elapsed() >= SYNC_QUIET || first.elapsed() >= SYNC_AT_MOST_AFTER));
            last_settings = Some(settings);
            if due {
                // Counted before, so a change made while it runs is sent next time.
                synced_changes = Some(changes);
                first_change = None;
                run_sync(&app).await;
                last_sync = Some(Instant::now());
            }
            tokio::time::sleep(SYNC_TICK).await;
        }
    });
}

/// One sync, quietly: the outcome goes to Settings › Sync and the log.
async fn run_sync(app: &AppHandle) {
    let (state, service) = (app.state::<AppState>(), app.state::<BackupService>());
    let folder = {
        let settings = state.settings.lock().unwrap();
        settings.sync_folder.clone().filter(|_| settings.sync_enabled)
    };
    let Some(folder) = folder else { return };
    let _turn = service.sync_turn.lock().await;
    service.syncing.store(true, Ordering::Relaxed);
    let _ = app.emit("sync-changed", current_sync_status(&state, &service));
    let core = state.core.clone();
    let computer = service.computer.clone();
    let mut sync = service.sync.lock().unwrap().clone();
    let worked = tokio::task::spawn_blocking(move || {
        let outcome = core.sync_with_folder(Path::new(&folder), &mut sync, &computer);
        (sync, outcome)
    })
    .await;
    match worked {
        Ok((mut sync, outcome)) => {
            match outcome {
                Ok(outcome) => {
                    if outcome.library_changed {
                        tracing::info!("the library changed from another computer's sync file");
                        let _ = app.emit("library-changed", ());
                    }
                }
                Err(error) => {
                    tracing::warn!("the sync couldn't run: {error}");
                    sync.last_error = Some(error.to_string());
                }
            }
            if let Err(error) = sync.save(&service.data_dir) {
                tracing::warn!("the sync state couldn't be saved: {error}");
            }
            *service.sync.lock().unwrap() = sync;
        }
        Err(error) => tracing::error!("the sync stopped unexpectedly: {error}"),
    }
    service.syncing.store(false, Ordering::Relaxed);
    let _ = app.emit("sync-changed", current_sync_status(&state, &service));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording(status: RecordingStatus, start: i64) -> Recording {
        Recording {
            id: "r".into(),
            source_id: "s".into(),
            channel_id: "c".into(),
            channel_name: "Channel".into(),
            channel_logo_url: None,
            title: "Programme".into(),
            subtitle: None,
            summary: None,
            artwork_url: None,
            program_id: None,
            start,
            end: start + 3600,
            padding_before_seconds: 60,
            padding_after_seconds: 180,
            status,
            file_path: None,
            file_size_bytes: None,
            recorded_seconds: None,
            error: None,
            created_at: 0,
            rule_id: None,
            episode: None,
            from_catchup: false,
            note: None,
            conflict: None,
        }
    }

    #[test]
    fn automatic_backups_stay_out_of_a_recording_s_way() {
        let now = 1_000_000;
        assert!(recording_starting(&[recording(RecordingStatus::Scheduled, now + 300)], now));
        assert!(recording_starting(&[recording(RecordingStatus::Recording, now)], now), "just started");
        assert!(!recording_starting(&[recording(RecordingStatus::Scheduled, now + 3600)], now), "an hour off");
        assert!(!recording_starting(&[recording(RecordingStatus::Recording, now - 1800)], now), "well under way");
        assert!(!recording_starting(&[recording(RecordingStatus::Completed, now)], now));
    }
}
