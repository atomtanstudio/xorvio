//! A media folder's own library page: listing its folders and videos, the
//! changes made there (deleting to Recently Deleted, putting back, moving,
//! renaming, new folders), and reading what each video file holds in the
//! background, so the page can tell GIFs, animations and videos apart and
//! long from short.
//!
//! After each change the catalog is right at once ("library-changed"), and
//! the folder is read again in the background to bring the rest in line.

use std::path::PathBuf;
use std::time::Duration;

use futures_util::StreamExt;
use tauri::{AppHandle, Emitter, Manager, State};
use xorvio_model::{DeletedBatch, FolderListing, FolderQuery, RefreshProgress};

use crate::{AppState, REFRESH_FINISHED};

type CmdResult<T> = Result<T, String>;

/// When the files start to be read after launch, once the window's settled.
const FIRST_READ_AFTER: Duration = Duration::from_secs(20);
/// Files read at once, at low priority: a network share answers several faster than one.
const READ_AT_ONCE: usize = 4;
/// Files taken from the catalog at a time.
const BATCH: i64 = 48;
/// How often to look for new files once all have been read.
const LOOK_AGAIN_EVERY: Duration = Duration::from_secs(120);
/// How often Recently Deleted is checked for what's been there a month.
const TIDY_EVERY: Duration = Duration::from_secs(6 * 3600);

fn fail(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Reads the media folders' video files, a few at a time, for as long as
/// the app runs; "folder-facts" tells the interface each time more are read.
/// Recently Deleted is tidied now and then too.
pub fn spawn(app: AppHandle, ffprobe: Option<PathBuf>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_READ_AFTER).await;
        let mut tidied_at: Option<std::time::Instant> = None;
        loop {
            let core = app.state::<AppState>().core.clone();
            if tidied_at.is_none_or(|at| at.elapsed() >= TIDY_EVERY) {
                let tidying = core.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || tidying.tidy_recently_deleted()).await;
                tidied_at = Some(std::time::Instant::now());
            }
            let Some(ffprobe) = ffprobe.as_deref() else {
                tokio::time::sleep(TIDY_EVERY).await;
                continue;
            };
            let wanted = core.videos_without_facts(BATCH).unwrap_or_default();
            if wanted.is_empty() {
                tokio::time::sleep(LOOK_AGAIN_EVERY).await;
                continue;
            }
            let read: Vec<_> = futures_util::stream::iter(wanted)
                .map(|(id, source_id, file)| async move {
                    let facts = xorvio_media::file_facts(ffprobe, &file).await;
                    // A file that can't be reached (a share gone away) is read another time.
                    let reached = facts.is_some() || tokio::fs::metadata(&file).await.is_ok();
                    (id, source_id, facts, reached)
                })
                .buffer_unordered(READ_AT_ONCE)
                .collect()
                .await;
            let mut kept = 0;
            for (id, source_id, facts, reached) in &read {
                if !reached {
                    continue;
                }
                match core.keep_file_facts(id, source_id, facts.as_ref()) {
                    Ok(()) => kept += 1,
                    Err(error) => tracing::warn!("what a video file holds couldn't be kept: {error}"),
                }
            }
            if kept > 0 {
                let _ = app.emit("folder-facts", kept);
            } else {
                // Nothing could be reached: the share's away, so wait before trying again.
                tokio::time::sleep(LOOK_AGAIN_EVERY).await;
            }
        }
    });
}

/// Reads a media folder again after a change, in the background.
fn read_again_soon(app: &AppHandle, source_id: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let core = app.state::<AppState>().core.clone();
        let events = app.clone();
        let progress = move |p: RefreshProgress| {
            let _ = events.emit("refresh-progress", &p);
        };
        let outcome = core.read_folder_again(&source_id, &progress).await;
        let _ = app.emit(REFRESH_FINISHED, &source_id);
        match outcome {
            Ok(()) => {
                let _ = app.emit("library-changed", ());
            }
            Err(error) => tracing::warn!("a media folder couldn't be read again after a change: {error}"),
        }
    });
}

/// After a change: the catalog's already right, and the folder's read again.
fn changed(app: &AppHandle, source_id: &str) {
    let _ = app.emit("library-changed", ());
    read_again_soon(app, source_id.to_string());
}

#[tauri::command]
pub async fn folder_listing(state: State<'_, AppState>, query: FolderQuery) -> CmdResult<FolderListing> {
    let core = state.core.clone();
    tauri::async_runtime::spawn_blocking(move || core.folder_listing(&query)).await.map_err(fail)?.map_err(fail)
}

/// Every video a listing shows on all its pages, for Select All.
#[tauri::command]
pub async fn folder_video_ids(state: State<'_, AppState>, query: FolderQuery) -> CmdResult<Vec<String>> {
    let core = state.core.clone();
    tauri::async_runtime::spawn_blocking(move || core.folder_video_ids(&query)).await.map_err(fail)?.map_err(fail)
}

/// Moves videos and folders to Recently Deleted; how many went.
#[tauri::command]
pub async fn delete_folder_items(app: AppHandle, state: State<'_, AppState>, source_id: String, videos: Vec<String>, folders: Vec<String>) -> CmdResult<usize> {
    let outcome = state.core.delete_folder_items(&source_id, &videos, &folders).await;
    changed(&app, &source_id);
    outcome.map_err(fail)
}

#[tauri::command]
pub async fn recently_deleted(state: State<'_, AppState>, source_id: String) -> CmdResult<Vec<DeletedBatch>> {
    let core = state.core.clone();
    tauri::async_runtime::spawn_blocking(move || core.recently_deleted(&source_id)).await.map_err(fail)?.map_err(fail)
}

#[tauri::command]
pub async fn restore_deleted(app: AppHandle, state: State<'_, AppState>, source_id: String, batch_id: String) -> CmdResult<()> {
    let outcome = state.core.restore_deleted(&source_id, &batch_id).await;
    changed(&app, &source_id);
    outcome.map_err(fail)
}

/// Deletes for good these deletions (all of Recently Deleted with none named).
#[tauri::command]
pub async fn empty_recently_deleted(state: State<'_, AppState>, source_id: String, batch_ids: Option<Vec<String>>) -> CmdResult<()> {
    state.core.empty_recently_deleted(&source_id, batch_ids.as_deref()).await.map_err(fail)
}

#[tauri::command]
pub async fn move_folder_items(app: AppHandle, state: State<'_, AppState>, source_id: String, videos: Vec<String>, folders: Vec<String>, to: String) -> CmdResult<()> {
    let outcome = state.core.move_folder_items(&source_id, &videos, &folders, &to).await;
    changed(&app, &source_id);
    outcome.map_err(fail)
}

#[tauri::command]
pub async fn rename_folder_video(app: AppHandle, state: State<'_, AppState>, source_id: String, item_id: String, name: String) -> CmdResult<()> {
    let outcome = state.core.rename_folder_video(&source_id, &item_id, &name).await;
    changed(&app, &source_id);
    outcome.map_err(fail)
}

/// Makes a folder; its path.
#[tauri::command]
pub async fn create_media_folder(state: State<'_, AppState>, source_id: String, parent: String, name: String) -> CmdResult<String> {
    state.core.create_media_folder(&source_id, &parent, &name).await.map_err(fail)
}

#[tauri::command]
pub async fn rename_media_folder(app: AppHandle, state: State<'_, AppState>, source_id: String, path: String, name: String) -> CmdResult<()> {
    let outcome = state.core.rename_media_folder(&source_id, &path, &name).await;
    changed(&app, &source_id);
    outcome.map_err(fail)
}
