//! Every command the interface calls, named as in src/lib/api.ts (snake_case
//! here, camelCase there). Errors come back as plain-English strings.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use xorvio_core::{AddonReach, BrowseQuery, BrowseSort, ChannelFilter, PlexServerChoice, PlexSignInStart, SecretStore, WatchedFilter};
use xorvio_media::{AdBreaks, Allowance, ConnectionCheck, PlayHint, PlaybackSupport, StreamOptions, Thumbnails};
use xorvio_model::*;

use crate::diagnostics::DiagnosticsLog;
use crate::players::{self, ExternalPlayer};
use crate::settings::{CommercialSkip, LiveBuffer};
use crate::{AppState, Settings};

type CmdResult<T> = Result<T, String>;

fn fail(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        list_sources,
        add_source,
        add_folder_source,
        plex_start_sign_in,
        plex_check_sign_in,
        add_plex_server,
        trakt_status,
        trakt_start_sign_in,
        trakt_check_sign_in,
        trakt_disconnect,
        trakt_sync_now,
        trakt_set_options,
        trakt_set_app,
        trakt_scrobble,
        trakt_row,
        trakt_lists,
        trailer_page,
        tmdb_status,
        tmdb_set_key,
        search_lookups_left,
        sign_in_to_source_again,
        replace_plex_sign_in,
        update_source,
        remove_source,
        source_secrets,
        refresh_source,
        cancel_refresh,
        reorder_sources,
        test_xtream_login,
        xtream_login_in_link,
        keep_dropped_playlist,
        crate::menu::set_menu_state,
        add_demo_source,
        swift_import_available,
        import_swift_sources,
        swift_library_available,
        import_swift_library,
        swift_data_available,
        import_swift_progress,
        categories,
        channels,
        channel,
        movies,
        movie,
        series_list,
        series,
        episodes,
        details,
        related_movies,
        related_series,
        title_marks,
        movie_versions,
        fill_posters,
        person,
        genres,
        top_titles,
        search,
        guide,
        now_next,
        program,
        upcoming_programs,
        search_guide,
        guide_span,
        guide_sources,
        add_guide_source,
        update_guide_source,
        remove_guide_source,
        guide_source_link,
        refresh_guide_source,
        guide_channels,
        guide_match,
        match_guide,
        reset_guide_match,
        watchlist,
        set_watchlisted,
        favorite_channel_ids,
        set_favorite,
        favorite_lists,
        create_favorite_list,
        rename_favorite_list,
        delete_favorite_list,
        set_in_favorite_list,
        reorder_favorite_list,
        item_edits,
        rename_item,
        set_hidden,
        set_catchup_offset,
        set_preferred_version,
        show_all_hidden,
        set_locked,
        parental_status,
        set_pin,
        unlock_parental,
        lock_parental,
        save_progress,
        set_watched,
        series_progress,
        progress,
        continue_watching,
        remove_from_continue_watching,
        recent_channels,
        record_channel_watched,
        record_watched,
        history,
        clear_history,
        play,
        start_timeshift,
        seek_stream,
        release_stream,
        restream,
        stream_failure,
        stream_position,
        track_choice,
        set_track_choice,
        thumbnails,
        set_playback_support,
        external_players,
        open_in_player,
        crate::cast::cast_devices,
        crate::cast::cast_check,
        crate::cast::cast_connect,
        crate::cast::cast_load,
        crate::cast::cast_play,
        crate::cast::cast_pause,
        crate::cast::cast_seek,
        crate::cast::cast_volume,
        crate::cast::cast_mute,
        crate::cast::cast_stop,
        addon_subtitle_text,
        folder_subtitles,
        folder_subtitle_text,
        episode_segments,
        recordings,
        connection_warning,
        schedule_recording,
        record_now,
        stop_recording,
        cancel_recording,
        delete_recording,
        reveal_recording,
        record_from_catchup,
        ad_breaks,
        find_ad_breaks,
        series_rules,
        create_series_rule,
        update_series_rule,
        delete_series_rule,
        addons,
        install_addon,
        remove_addon,
        set_addon_enabled,
        discover_rows,
        addon_catalog,
        addon_meta,
        addon_streams,
        addon_subtitles,
        addon_search,
        settings,
        save_settings,
        storage_usage,
        clear_guide_listings,
        clear_temporary_files,
        show_recordings_folder,
        choose_folder,
        choose_file,
        choose_media_folders,
        open_external,
        client_log,
        diagnostics,
        clear_diagnostics,
        export_diagnostics,
        crate::popout::pop_out,
        crate::popout::take_pop_out,
        crate::popout::dock,
        crate::popout::set_pop_out_on_top,
        profiles,
        set_active_profile,
        save_profile,
        delete_profile,
        profile_categories,
        home_layout,
        save_home_layout,
        on_now,
        crate::backup::backup_status,
        crate::backup::back_up_now,
        crate::backup::choose_backup,
        crate::backup::restore_backup,
        crate::backup::show_backups_folder,
        crate::backup::sync_status,
        crate::backup::sync_now,
        new_releases,
        new_episode_series,
        new_episode_counts,
        mark_episodes_seen,
        crate::reminders::notification_permission,
        crate::reminders::send_test_reminder,
        crate::reminders::open_notification_settings,
        crate::media_library::folder_listing,
        crate::media_library::folder_video_ids,
        crate::media_library::delete_folder_items,
        crate::media_library::recently_deleted,
        crate::media_library::restore_deleted,
        crate::media_library::empty_recently_deleted,
        crate::media_library::move_folder_items,
        crate::media_library::rename_folder_video,
        crate::media_library::create_media_folder,
        crate::media_library::rename_media_folder,
    ]
}

/// The add-ons crate keeps manifest links in the same keychain as sources.
pub fn addon_secrets(secrets: Arc<dyn SecretStore>) -> Arc<dyn xorvio_addons::SecretStore> {
    struct Bridge(Arc<dyn SecretStore>);
    impl xorvio_addons::SecretStore for Bridge {
        fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
            self.0.get(&format!("addon:{key}"))
        }
        fn set(&self, key: &str, value: &str) -> anyhow::Result<()> {
            self.0.set(&format!("addon:{key}"), value)
        }
        fn delete(&self, key: &str) -> anyhow::Result<()> {
            self.0.delete(&format!("addon:{key}"))
        }
    }
    Arc::new(Bridge(secrets))
}

// MARK: - Sources

#[tauri::command]
pub async fn list_sources(state: State<'_, AppState>) -> CmdResult<Vec<SourceSummary>> {
    state.core.list_sources().map_err(fail)
}

#[tauri::command]
pub async fn add_source(state: State<'_, AppState>, source: NewSource) -> CmdResult<SourceConfig> {
    state.picked.check(source.file_path.as_deref(), &[])?;
    state.core.add_source(source).await.map_err(fail)
}

/// Adds a media folder source with the folders chosen. It's read by a refresh of its own.
#[tauri::command]
pub async fn add_folder_source(state: State<'_, AppState>, name: String, folders: Vec<String>) -> CmdResult<SourceConfig> {
    state.picked.check(folders.iter().map(String::as_str), &[])?;
    state.core.add_folder_source(&name, &folders).map_err(fail)
}

/// Starts signing in to Plex: the code, and the plex.tv page to open.
#[tauri::command]
pub async fn plex_start_sign_in(state: State<'_, AppState>) -> CmdResult<PlexSignInStart> {
    state.core.plex_start_sign_in().await.map_err(fail)
}

/// The account's servers once the sign-in is approved; null while it waits.
#[tauri::command]
pub async fn plex_check_sign_in(state: State<'_, AppState>, pin_id: i64) -> CmdResult<Option<Vec<PlexServerChoice>>> {
    state.core.plex_check_sign_in(pin_id).await.map_err(fail)
}

#[tauri::command]
pub async fn add_plex_server(state: State<'_, AppState>, pin_id: i64, server_id: String, name: String) -> CmdResult<SourceConfig> {
    state.core.add_plex_server(pin_id, &server_id, &name).await.map_err(fail)
}

// MARK: - Trakt

#[tauri::command]
pub async fn trakt_status(state: State<'_, AppState>) -> CmdResult<TraktStatus> {
    Ok(state.core.trakt_status())
}

/// Starts connecting: a code to enter at the verification address.
#[tauri::command]
pub async fn trakt_start_sign_in(state: State<'_, AppState>) -> CmdResult<TraktSignInStart> {
    state.core.trakt_start_sign_in().await.map_err(fail)
}

/// True once the code has been entered and Xorvio allowed, which starts the
/// first sync; false while waiting. Fails when the code expired or was refused.
#[tauri::command]
pub async fn trakt_check_sign_in(state: State<'_, AppState>) -> CmdResult<bool> {
    let connected = state.core.trakt_check_sign_in().await.map_err(fail)?;
    if connected {
        crate::spawn_trakt_sync(state.core.clone());
    }
    Ok(connected)
}

#[tauri::command]
pub async fn trakt_disconnect(state: State<'_, AppState>) -> CmdResult<()> {
    state.core.trakt_disconnect().await.map_err(fail)
}

/// Runs a sync now (or waits for the one running) and returns how it went.
#[tauri::command]
pub async fn trakt_sync_now(state: State<'_, AppState>) -> CmdResult<TraktStatus> {
    state.core.trakt_sync().await.map_err(fail)
}

#[tauri::command]
pub async fn trakt_set_options(state: State<'_, AppState>, options: TraktOptions) -> CmdResult<()> {
    state.core.trakt_set_options(options).map_err(fail)
}

/// Uses this Trakt app instead of the built-in one; an empty client id goes
/// back to the built-in one. The redirect address is the one the app was
/// registered with; empty is the default.
#[tauri::command]
pub async fn trakt_set_app(state: State<'_, AppState>, client_id: String, redirect_uri: String) -> CmdResult<()> {
    state.core.trakt_set_app(&client_id, &redirect_uri).await.map_err(fail)
}

/// A Trakt chart, list or recommendations for a row on Home or its own page:
/// the sources' copies, and add-on titles for the rest when an add-on can
/// open and play them (for all of them, when add-ons lead, so their pages
/// offer every add-on's copies and the sources' too).
#[tauri::command]
pub async fn trakt_row(state: State<'_, AppState>, source: TraktRowSource, limit: usize) -> CmdResult<TraktRow> {
    let addons = state.addons.list();
    let reaches = |media_type: &str| {
        let on = |resource: &str| {
            addons.iter().any(|a| a.enabled && a.types.iter().any(|t| t == media_type) && a.resources.iter().any(|r| r == resource))
        };
        on("meta") && on("stream")
    };
    let lead = state.settings.lock().unwrap().addons_lead;
    let reach = AddonReach { movies: reaches("movie"), series: reaches("series"), lead };
    state.core.trakt_row(&source, limit.min(1000), reach).await.map_err(fail)
}

/// Trakt lists to make rows of: the account's own or liked, popular, or found.
#[tauri::command]
pub async fn trakt_lists(state: State<'_, AppState>, query: TraktListQuery) -> CmdResult<Vec<TraktListInfo>> {
    state.core.trakt_lists(&query).await.map_err(fail)
}

// MARK: - TMDB

#[tauri::command]
pub async fn tmdb_status(state: State<'_, AppState>) -> CmdResult<TmdbStatus> {
    Ok(state.core.tmdb_status())
}

/// How many titles are still to be looked up on TMDB for search, or none
/// when they aren't being looked up.
#[tauri::command]
pub async fn search_lookups_left(state: State<'_, AppState>) -> CmdResult<Option<usize>> {
    let core = state.core.clone();
    tauri::async_runtime::spawn_blocking(move || core.search_lookups_left()).await.map_err(fail)?.map_err(fail)
}

/// Saves the person's own TMDB key once TMDB takes it; empty removes it.
#[tauri::command]
pub async fn tmdb_set_key(state: State<'_, AppState>, key: String) -> CmdResult<TmdbStatus> {
    state.core.tmdb_set_key(&key).await.map_err(fail)
}

/// Tells Trakt what's playing (progress in percent). Never fails: a problem
/// is only logged.
#[tauri::command]
pub async fn trakt_scrobble(
    state: State<'_, AppState>,
    item_id: String,
    kind: MediaKind,
    action: TraktScrobbleAction,
    progress: f64,
) -> CmdResult<()> {
    state.core.trakt_scrobble(&item_id, kind, action, progress).await;
    Ok(())
}

/// Signs a Jellyfin or Emby source in again as the same user.
#[tauri::command]
pub async fn sign_in_to_source_again(state: State<'_, AppState>, id: String, password: String) -> CmdResult<()> {
    state.core.sign_in_again(&id, &password).await.map_err(fail)
}

/// Gives a Plex source the token from a new Plex sign-in.
#[tauri::command]
pub async fn replace_plex_sign_in(state: State<'_, AppState>, id: String, pin_id: i64) -> CmdResult<()> {
    state.core.replace_plex_sign_in(&id, pin_id).await.map_err(fail)
}

#[tauri::command]
pub async fn update_source(state: State<'_, AppState>, config: SourceConfig, secrets: Option<SourceSecrets>) -> CmdResult<()> {
    // Folders and playlist files it has already are fine; new ones must have been chosen.
    let existing = state.core.list_sources().map_err(fail)?.into_iter().find(|s| s.config.id == config.id).map(|s| s.config);
    let known: Vec<&str> = existing.iter().flat_map(|e| e.folders.iter().map(String::as_str).chain(e.file_path.as_deref())).collect();
    state.picked.check(config.folders.iter().map(String::as_str).chain(config.file_path.as_deref()), &known)?;
    state.core.update_source(config, secrets).map_err(fail)
}

#[tauri::command]
pub async fn remove_source(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.core.remove_source(&id).map_err(fail)
}

#[tauri::command]
pub async fn source_secrets(state: State<'_, AppState>, id: String) -> CmdResult<SourceSecrets> {
    state.core.source_secrets(&id).map_err(fail)
}

#[tauri::command]
pub async fn refresh_source(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<SourceStatus> {
    let events = app.clone();
    let progress = move |p: RefreshProgress| {
        let _ = events.emit("refresh-progress", &p);
    };
    let status = state.core.refresh_source(&id, &progress).await;
    let _ = app.emit(crate::REFRESH_FINISHED, &id);
    let status = status.map_err(fail)?;
    let _ = app.emit("library-changed", ());
    state.recorder.evaluate_rules_soon();
    Ok(status)
}

/// Asks a running refresh to stop; false when none is running. The refresh
/// returns soon after, with the source as it was.
#[tauri::command]
pub async fn cancel_refresh(state: State<'_, AppState>, id: String) -> CmdResult<bool> {
    Ok(state.core.cancel_refresh(&id))
}

/// The sources in a new order, which Live TV lists channels in.
#[tauri::command]
pub async fn reorder_sources(app: AppHandle, state: State<'_, AppState>, ids: Vec<String>) -> CmdResult<()> {
    state.core.reorder_sources(&ids).map_err(fail)?;
    let _ = app.emit("library-changed", ());
    Ok(())
}

/// Test Connection: what the provider says about the account, never the login.
#[tauri::command]
pub async fn test_xtream_login(state: State<'_, AppState>, secrets: SourceSecrets, user_agent: Option<String>) -> CmdResult<xorvio_core::XtreamAccountCheck> {
    state.core.test_xtream_login(&secrets, user_agent.as_deref()).await.map_err(fail)
}

/// The Xtream login inside a playlist link the person pasted, if it holds one.
#[tauri::command]
pub fn xtream_login_in_link(link: String) -> Option<SourceSecrets> {
    xorvio_core::xtream_login_in_link(&link)
}

/// Keeps a playlist dropped onto the add sheet, which reaches the
/// interface as its text alone, and returns where for an M3U file source.
#[tauri::command]
pub async fn keep_dropped_playlist(state: State<'_, AppState>, name: String, contents: String) -> CmdResult<String> {
    let path = state.core.keep_playlist_copy(&name, contents.as_bytes()).map_err(fail)?;
    state.picked.add(&path);
    Ok(path)
}

#[tauri::command]
pub async fn add_demo_source(state: State<'_, AppState>) -> CmdResult<()> {
    state.core.add_demo_source().map_err(fail)
}

#[tauri::command]
pub async fn swift_import_available(state: State<'_, AppState>) -> CmdResult<bool> {
    let has_providers = state.core.list_sources().map(|l| l.iter().any(|s| s.config.kind.is_provider())).unwrap_or(false);
    Ok(!has_providers && crate::import::swift_sources_available())
}

/// Adds the Swift app's provider sources. Each login is checked as it's added;
/// ones that fail are skipped and named in the error.
#[tauri::command]
pub async fn import_swift_sources(state: State<'_, AppState>) -> CmdResult<Vec<SourceConfig>> {
    let sources = tokio::task::spawn_blocking(crate::import::read_swift_sources).await.map_err(fail)?.map_err(fail)?;
    if sources.is_empty() {
        return Err("Xorvio for Mac has no provider logins to import.".into());
    }
    let mut added = Vec::new();
    let mut failed = Vec::new();
    for source in sources {
        let name = source.name.clone();
        match state.core.add_source(source).await {
            Ok(config) => added.push(config),
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    if added.is_empty() {
        return Err(format!("Nothing could be imported. {}", failed.join(" ")));
    }
    Ok(added)
}

#[tauri::command]
pub async fn swift_library_available(state: State<'_, AppState>) -> CmdResult<bool> {
    let done = state.settings.lock().unwrap().swift_library_imported;
    Ok(!done && crate::import::swift_library_available())
}

/// Whether either Swift app's data is on this Mac at all (for Settings).
#[tauri::command]
pub async fn swift_data_available() -> CmdResult<bool> {
    Ok(crate::import::swift_library_available())
}

/// The Swift app's source ids mapped to the same sources here, matched by
/// name and kind (ids differ between the apps).
fn swift_source_map(swift_sources: &[(String, String, String)], here: &[SourceSummary]) -> std::collections::HashMap<String, String> {
    swift_sources
        .iter()
        .filter_map(|(id, kind, name)| {
            let found = here.iter().find(|s| {
                s.config.name == *name
                    && matches!((kind.as_str(), s.config.kind), ("xtream", SourceKind::Xtream) | ("m3uURL", SourceKind::M3uUrl) | ("m3uFile", SourceKind::M3uFile))
            })?;
            Some((id.clone(), found.config.id.clone()))
        })
        .collect()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressImport {
    imported: usize,
    /// Records for sources that aren't here.
    skipped: usize,
}

/// Brings both Swift apps' watch progress over: where each film and episode
/// got to and what was finished. The newest record wins, so nothing watched
/// here is rolled back.
#[tauri::command]
pub async fn import_swift_progress(state: State<'_, AppState>) -> CmdResult<ProgressImport> {
    use crate::import::{addon_item_id, read_swift_progress, remap_source};
    let here = state.core.list_sources().map_err(fail)?;
    let all = tokio::task::spawn_blocking(read_swift_progress).await.map_err(fail)?;
    let (mut imported, mut skipped) = (0, 0);
    for (swift_sources, rows) in all {
        let sources = swift_source_map(&swift_sources, &here);
        for row in rows {
            let kind = match row.kind.as_str() {
                "movie" => MediaKind::Movie,
                "episode" => MediaKind::Episode,
                _ => continue,
            };
            let mapped = if let Some(id) = addon_item_id(&row.item_id) {
                Some((id, xorvio_addons::SOURCE_ID.to_string(), row.series_id.as_deref().and_then(addon_item_id)))
            } else {
                sources.get(&row.source_id).and_then(|new| {
                    let id = remap_source(&row.item_id, &row.source_id, new)?;
                    let series = row.series_id.as_deref().and_then(|s| remap_source(s, &row.source_id, new));
                    Some((id, new.clone(), series))
                })
            };
            let Some((item_id, source_id, series_id)) = mapped else {
                skipped += 1;
                continue;
            };
            state
                .core
                .save_progress(PlaybackProgress {
                    item_id,
                    kind,
                    source_id,
                    series_id,
                    position_seconds: row.position,
                    duration_seconds: row.duration,
                    finished: row.finished,
                    updated_at: row.updated_at,
                })
                .map_err(fail)?;
            imported += 1;
        }
    }
    Ok(ProgressImport { imported, skipped })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryImport {
    /// Names of the add-ons now installed.
    addons: Vec<String>,
    /// Plain-English problems with add-ons that couldn't be installed.
    addon_problems: Vec<String>,
    watchlist_added: usize,
    /// Titles left out: a source that isn't here, or something no longer in it.
    watchlist_skipped: Vec<String>,
    progress_imported: usize,
}

/// Brings Xorvio for Mac's add-ons (Personal's, plus Cinemeta, which it had
/// built in) and both Swift apps' watchlists over. The Swift apps' own
/// files and keychain items are only read.
#[tauri::command]
pub async fn import_swift_library(state: State<'_, AppState>) -> CmdResult<LibraryImport> {
    use crate::import::{CINEMETA, addon_item_id, read_personal_addons, read_swift_watchlists, remap_source};

    forget_discover(&state);
    // Add-ons first, so the watchlist's add-on titles have details to show.
    let mut links = tokio::task::spawn_blocking(read_personal_addons).await.map_err(fail)?;
    links.push(CINEMETA.to_string());
    let mut addons = Vec::new();
    let mut addon_problems = Vec::new();
    for link in links {
        match state.addons.install(&link).await {
            Ok(info) => addons.push(info.name),
            Err(e) => addon_problems.push(e.to_string()),
        }
    }

    let here = state.core.list_sources().map_err(fail)?;
    let watchlists = tokio::task::spawn_blocking(read_swift_watchlists).await.map_err(fail)?;
    let already: std::collections::HashSet<String> = state.core.watchlist().map_err(fail)?.into_iter().map(|e| e.id).collect();
    let mut added = std::collections::HashSet::new();
    let mut watchlist_skipped = Vec::new();
    let now = now();
    for (swift_sources, rows) in watchlists {
        for row in rows {
            let kind = match row.kind.as_str() {
                "movie" => MediaKind::Movie,
                "series" => MediaKind::Series,
                "program" => MediaKind::Program,
                _ => continue,
            };
            if kind == MediaKind::Program && row.end.is_some_and(|end| end <= now) {
                continue;
            }
            let mapped = if let Some(id) = addon_item_id(&row.item_id) {
                Some((id, xorvio_addons::SOURCE_ID.to_string(), None))
            } else {
                // The same provider here: matched by name and kind, since ids differ.
                swift_sources
                    .iter()
                    .find(|(id, _, _)| *id == row.source_id)
                    .and_then(|(_, swift_kind, name)| {
                        here.iter().find(|s| {
                            s.config.name == *name
                                && matches!((swift_kind.as_str(), s.config.kind), ("xtream", SourceKind::Xtream) | ("m3uURL", SourceKind::M3uUrl) | ("m3uFile", SourceKind::M3uFile))
                        })
                    })
                    .and_then(|s| {
                        let id = remap_source(&row.item_id, &row.source_id, &s.config.id)?;
                        let channel = row.channel_id.as_deref().and_then(|c| remap_source(c, &row.source_id, &s.config.id));
                        Some((id, s.config.id.clone(), channel))
                    })
                    .filter(|(id, _, _)| match kind {
                        MediaKind::Movie => state.core.movie(id).ok().flatten().is_some(),
                        MediaKind::Series => state.core.series(id).ok().flatten().is_some(),
                        _ => true,
                    })
            };
            let Some((id, source_id, channel_id)) = mapped else {
                watchlist_skipped.push(row.title);
                continue;
            };
            if already.contains(&id) || !added.insert(id.clone()) {
                continue;
            }
            let entry = WatchlistEntry {
                id,
                kind,
                source_id,
                title: row.title,
                subtitle: row.subtitle,
                artwork_url: row.artwork,
                added_at: row.added_at,
                channel_id,
                start: row.start,
                end: row.end,
                remind: row.remind,
            };
            state.core.set_watchlisted(entry, true).map_err(fail)?;
        }
    }

    let mut settings = state.settings.lock().unwrap().clone();
    settings.swift_library_imported = true;
    // Xorvio Personal leads with its add-ons; keep that unless nothing installed.
    if !addons.is_empty() {
        settings.addons_lead = true;
    }
    settings.save(&state.settings_path).map_err(fail)?;
    *state.settings.lock().unwrap() = settings;
    state.addon_metas.lock().unwrap().clear();
    let progress = import_swift_progress(state).await?;

    Ok(LibraryImport { addons, addon_problems, watchlist_added: added.len(), watchlist_skipped, progress_imported: progress.imported })
}

// MARK: - Catalog

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BrowseSortArg {
    RecentlyAdded,
    Title,
    Year,
    Rating,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseQueryArg {
    category_id: Option<String>,
    genre: Option<String>,
    /// Home's genre rows: the genre field or a category named for it.
    content_genre: Option<ContentGenre>,
    sort: BrowseSortArg,
    limit: usize,
    offset: usize,
    /// Movies and Series' Source picker.
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    watched: WatchedFilter,
    /// The copies of a film as one poster (Movies).
    #[serde(default)]
    group_versions: bool,
}

impl BrowseQueryArg {
    fn into_query(self) -> BrowseQuery {
        BrowseQuery {
            category_id: self.category_id,
            genre: self.genre,
            content_genre: self.content_genre,
            sort: match self.sort {
                BrowseSortArg::RecentlyAdded => BrowseSort::RecentlyAdded,
                BrowseSortArg::Title => BrowseSort::Title,
                BrowseSortArg::Year => BrowseSort::Year,
                BrowseSortArg::Rating => BrowseSort::Rating,
            },
            limit: self.limit.min(500),
            offset: self.offset,
            source_id: self.source_id,
            watched: self.watched,
            group_versions: self.group_versions,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChannelFilterArg {
    category_id: Option<String>,
    favorites_only: bool,
    list_id: Option<String>,
    source_id: Option<String>,
    include_hidden: bool,
    limit: Option<usize>,
    /// Live TV says, so a list loaded just after the setting changed follows it.
    group_similar: Option<bool>,
}

/// Categories that hold something; with `include_hidden`, hidden ones too
/// (Live TV's "Show Hidden").
#[tauri::command]
pub async fn categories(state: State<'_, AppState>, kind: CategoryKind, include_hidden: Option<bool>) -> CmdResult<Vec<Category>> {
    if include_hidden.unwrap_or(false) {
        state.core.categories_with_hidden(kind).map_err(fail)
    } else {
        state.core.categories(kind).map_err(fail)
    }
}

#[tauri::command]
pub async fn channels(state: State<'_, AppState>, filter: Option<ChannelFilterArg>) -> CmdResult<Vec<Channel>> {
    let f = filter.unwrap_or_default();
    let filter = ChannelFilter {
        category_id: f.category_id,
        favorites_only: f.favorites_only,
        list_id: f.list_id,
        source_id: f.source_id,
        include_hidden: f.include_hidden,
        limit: f.limit,
        group_similar: f.group_similar,
    };
    state.core.channels(&filter).map_err(fail)
}

#[tauri::command]
pub async fn channel(state: State<'_, AppState>, id: String) -> CmdResult<Option<Channel>> {
    state.core.channel(&id).map_err(fail)
}

/// "addon|movie|tt0111161" → ("movie", "tt0111161").
fn addon_ref(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix("addon|")?;
    rest.split_once('|')
}

/// A title's add-on meta, fetched once per session.
async fn cached_meta(state: &AppState, media_type: &str, media_id: &str) -> anyhow::Result<AddonMeta> {
    let key = format!("{media_type}|{media_id}");
    if let Some(meta) = state.addon_metas.lock().unwrap().get(&key) {
        return Ok(meta.clone());
    }
    let meta = state.addons.meta(media_type, media_id).await?;
    state.addon_metas.lock().unwrap().insert(key, meta.clone());
    Ok(meta)
}

/// A followed add-on series as its add-on has it now, for the new-episode
/// check. It replaces the session's copy, so its page and Continue Watching
/// have the new episodes too.
pub(crate) async fn fresh_addon_series(state: &AppState, id: &str) -> Option<(Series, Vec<Episode>)> {
    let (media_type, media_id) = addon_ref(id)?;
    let meta = match state.addons.meta(media_type, media_id).await {
        Ok(meta) => meta,
        Err(error) => {
            tracing::warn!("an add-on series couldn't be looked at for new episodes: {error:#}");
            return None;
        }
    };
    state.addon_metas.lock().unwrap().insert(format!("{media_type}|{media_id}"), meta.clone());
    Some((meta.series?, meta.episodes))
}

#[tauri::command]
pub async fn movies(state: State<'_, AppState>, query: BrowseQueryArg) -> CmdResult<Vec<Movie>> {
    state.core.movies(&query.into_query()).map_err(fail)
}

#[tauri::command]
pub async fn movie(state: State<'_, AppState>, id: String) -> CmdResult<Option<Movie>> {
    if let Some((media_type, media_id)) = addon_ref(&id) {
        return Ok(cached_meta(&state, media_type, media_id).await.map_err(fail)?.movie);
    }
    state.core.movie(&id).map_err(fail)
}

#[tauri::command]
pub async fn series_list(state: State<'_, AppState>, query: BrowseQueryArg) -> CmdResult<Vec<Series>> {
    state.core.series_list(&query.into_query()).map_err(fail)
}

#[tauri::command]
pub async fn series(state: State<'_, AppState>, id: String) -> CmdResult<Option<Series>> {
    if let Some((media_type, media_id)) = addon_ref(&id) {
        return Ok(cached_meta(&state, media_type, media_id).await.map_err(fail)?.series);
    }
    state.core.series(&id).map_err(fail)
}

#[tauri::command]
pub async fn episodes(state: State<'_, AppState>, series_id: String) -> CmdResult<Vec<Episode>> {
    if let Some((media_type, media_id)) = addon_ref(&series_id) {
        return Ok(cached_meta(&state, media_type, media_id).await.map_err(fail)?.episodes);
    }
    state.core.episodes(&series_id).await.map_err(fail)
}

#[tauri::command]
pub async fn details(state: State<'_, AppState>, item_id: String) -> CmdResult<MediaDetails> {
    if let Some((media_type, media_id)) = addon_ref(&item_id) {
        let meta = cached_meta(&state, media_type, media_id).await.map_err(fail)?;
        return Ok(match (&meta.movie, &meta.series) {
            (Some(movie), _) => state.core.addon_movie_details(movie, meta.details).await,
            (_, Some(series)) => state.core.addon_series_details(series, meta.details).await,
            _ => meta.details,
        });
    }
    state.core.details(&item_id).await.map_err(fail)
}

/// Films TMDB recommends with this one that the sources carry (none
/// without a TMDB key).
#[tauri::command]
pub async fn related_movies(state: State<'_, AppState>, movie_id: String) -> CmdResult<Vec<Movie>> {
    if let Some((media_type, media_id)) = addon_ref(&movie_id) {
        let Some(movie) = cached_meta(&state, media_type, media_id).await.map_err(fail)?.movie else { return Ok(Vec::new()) };
        return state.core.movies_related_to(&movie).await.map_err(fail);
    }
    state.core.related_movies(&movie_id).await.map_err(fail)
}

#[tauri::command]
pub async fn related_series(state: State<'_, AppState>, series_id: String) -> CmdResult<Vec<Series>> {
    if let Some((media_type, media_id)) = addon_ref(&series_id) {
        let Some(series) = cached_meta(&state, media_type, media_id).await.map_err(fail)?.series else { return Ok(Vec::new()) };
        return state.core.series_related_to(&series).await.map_err(fail);
    }
    state.core.related_series(&series_id).await.map_err(fail)
}

/// Versions and what's been watched, for the posters of a list.
#[tauri::command]
pub async fn title_marks(state: State<'_, AppState>, kind: CategoryKind, ids: Vec<String>) -> CmdResult<Vec<TitleMark>> {
    state.core.title_marks(kind, &ids).map_err(fail)
}

/// Every copy of a film the sources carry, for its page's Versions picker.
#[tauri::command]
pub async fn movie_versions(state: State<'_, AppState>, movie_id: String) -> CmdResult<Vec<TitleVersion>> {
    state.core.movie_versions(&movie_id).map_err(fail)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilledPoster {
    id: String,
    poster_url: String,
}

/// Pictures for titles a list shows without one: TMDB's posters (with a
/// key), and for a media folder's videos still without one, a frame of the
/// video, kept for next time.
#[tauri::command]
pub async fn fill_posters(state: State<'_, AppState>, ids: Vec<String>) -> CmdResult<Vec<FilledPoster>> {
    let mut found = state.core.fill_posters(&ids).await.map_err(fail)?;
    let have: std::collections::HashSet<String> = found.iter().map(|(id, _)| id.clone()).collect();
    let rest: Vec<String> = ids.into_iter().filter(|id| !have.contains(id)).collect();
    let videos = state.core.videos_without_pictures(&rest).map_err(fail)?;
    let stills = &state.stills;
    let framed = futures_util::future::join_all(videos.into_iter().map(|(id, source_id, file)| async move {
        let name = stills.still(&file).await?;
        Some((id, source_id, xorvio_media::still_url(&name)))
    }))
    .await;
    for (id, source_id, url) in framed.into_iter().flatten() {
        if let Err(error) = state.core.keep_video_picture(&id, &source_id, &url) {
            tracing::warn!("a video's frame couldn't be kept for its card: {error}");
        }
        found.push((id, url));
    }
    Ok(found.into_iter().map(|(id, poster_url)| FilledPoster { id, poster_url }).collect())
}

/// Someone from a cast, from TMDB (or Wikidata without a key), with their
/// titles the sources carry; `null` when neither knows them.
#[tauri::command]
pub async fn person(state: State<'_, AppState>, name: String, tmdb_id: Option<String>, item_id: Option<String>) -> CmdResult<Option<PersonDetails>> {
    state.core.person(&name, tmdb_id.as_deref(), item_id.as_deref()).await.map_err(fail)
}

#[tauri::command]
pub async fn genres(state: State<'_, AppState>, kind: CategoryKind) -> CmdResult<Vec<String>> {
    state.core.genres(kind).map_err(fail)
}

#[derive(Debug, Serialize)]
pub struct TopTitles {
    movies: Vec<Movie>,
    series: Vec<Series>,
}

#[tauri::command]
pub async fn top_titles(state: State<'_, AppState>, kind: CategoryKind, limit: usize) -> CmdResult<TopTitles> {
    let (movies, series) = state.core.top_titles(kind, limit).map_err(fail)?;
    Ok(TopTitles { movies, series })
}

/// Search lists this many programmes at most, for "Show All Listings".
const PROGRAM_RESULTS: usize = 300;

#[tauri::command]
pub async fn search(state: State<'_, AppState>, text: String) -> CmdResult<SearchResults> {
    state.core.search_with(&text, 40, PROGRAM_RESULTS).map_err(fail)
}

// MARK: - Guide

#[tauri::command]
pub async fn guide(state: State<'_, AppState>, channel_ids: Vec<String>, from: Timestamp, to: Timestamp) -> CmdResult<Vec<GuideRow>> {
    state.core.guide(&channel_ids, from, to).map_err(fail)
}

#[tauri::command]
pub async fn now_next(state: State<'_, AppState>, channel_ids: Vec<String>) -> CmdResult<Vec<GuideRow>> {
    state.core.now_next(&channel_ids, now()).map_err(fail)
}

#[tauri::command]
pub async fn program(state: State<'_, AppState>, program_id: String) -> CmdResult<Option<ProgramHit>> {
    state.core.program(&program_id).map_err(fail)
}

#[tauri::command]
pub async fn upcoming_programs(state: State<'_, AppState>, from: Timestamp, to: Timestamp, limit: usize) -> CmdResult<Vec<ProgramHit>> {
    state.core.upcoming_programs(from, to, limit).map_err(fail)
}

/// Programmes by title for Live TV's guide search, from `from` on.
#[tauri::command]
pub async fn search_guide(state: State<'_, AppState>, text: String, from: Timestamp) -> CmdResult<Vec<ProgramHit>> {
    state.core.search_guide(&text, from, 30).map_err(fail)
}

#[derive(Debug, Serialize)]
pub struct GuideSpan {
    from: Timestamp,
    to: Timestamp,
}

/// About when the loaded listings begin and end, for the guide's day list.
#[tauri::command]
pub async fn guide_span(state: State<'_, AppState>) -> CmdResult<Option<GuideSpan>> {
    Ok(state.core.guide_span().map_err(fail)?.map(|(from, to)| GuideSpan { from, to }))
}

// MARK: - Added guides and matching

#[tauri::command]
pub async fn guide_sources(state: State<'_, AppState>) -> CmdResult<Vec<GuideSourceSummary>> {
    state.core.guide_sources().map_err(fail)
}

#[tauri::command]
pub async fn add_guide_source(state: State<'_, AppState>, guide: NewGuideSource) -> CmdResult<GuideSource> {
    state.core.add_guide_source(guide).map_err(fail)
}

#[tauri::command]
pub async fn update_guide_source(state: State<'_, AppState>, guide: GuideSource, url: Option<String>) -> CmdResult<()> {
    state.core.update_guide_source(guide, url).map_err(fail)
}

#[tauri::command]
pub async fn remove_guide_source(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.core.remove_guide_source(&id).map_err(fail)?;
    let _ = app.emit("library-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn guide_source_link(state: State<'_, AppState>, id: String) -> CmdResult<Option<String>> {
    state.core.guide_source_link(&id).map_err(fail)
}

#[tauri::command]
pub async fn refresh_guide_source(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<SourceStatus> {
    let status = state.core.refresh_guide_source(&id).await.map_err(fail)?;
    let _ = app.emit("library-changed", ());
    state.recorder.evaluate_rules_soon();
    Ok(status)
}

#[tauri::command]
pub async fn guide_channels(state: State<'_, AppState>, text: String) -> CmdResult<Vec<GuideChannel>> {
    state.core.guide_channels(&text, 60).map_err(fail)
}

#[tauri::command]
pub async fn guide_match(state: State<'_, AppState>, channel_id: String) -> CmdResult<GuideMatch> {
    state.core.guide_match(&channel_id).map_err(fail)
}

#[tauri::command]
pub async fn match_guide(state: State<'_, AppState>, channel_id: String, guide_id: String, epg_id: String) -> CmdResult<()> {
    state.core.match_guide(&channel_id, &guide_id, &epg_id).map_err(fail)
}

#[tauri::command]
pub async fn reset_guide_match(state: State<'_, AppState>, channel_id: String) -> CmdResult<()> {
    state.core.reset_guide_match(&channel_id).map_err(fail)
}

fn now() -> Timestamp {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

// MARK: - Library

#[tauri::command]
pub async fn watchlist(state: State<'_, AppState>) -> CmdResult<Vec<WatchlistEntry>> {
    state.core.watchlist_shown().map_err(fail)
}

#[tauri::command]
pub async fn set_watchlisted(state: State<'_, AppState>, entry: WatchlistEntry, saved: bool) -> CmdResult<()> {
    state.core.set_watchlisted(entry, saved).map_err(fail)
}

#[tauri::command]
pub async fn favorite_channel_ids(state: State<'_, AppState>) -> CmdResult<Vec<String>> {
    state.core.favorite_channel_ids().map_err(fail)
}

#[tauri::command]
pub async fn set_favorite(state: State<'_, AppState>, channel_id: String, favorite: bool) -> CmdResult<()> {
    state.core.set_favorite(&channel_id, favorite).map_err(fail)
}

// MARK: - Channel groups, names, hiding and the parental PIN

#[tauri::command]
pub async fn favorite_lists(state: State<'_, AppState>) -> CmdResult<Vec<FavoriteList>> {
    state.core.favorite_lists().map_err(fail)
}

#[tauri::command]
pub async fn create_favorite_list(state: State<'_, AppState>, name: String, channel_id: Option<String>) -> CmdResult<FavoriteList> {
    state.core.create_favorite_list(&name, channel_id.as_deref()).map_err(fail)
}

#[tauri::command]
pub async fn rename_favorite_list(state: State<'_, AppState>, id: String, name: String) -> CmdResult<()> {
    state.core.rename_favorite_list(&id, &name).map_err(fail)
}

#[tauri::command]
pub async fn delete_favorite_list(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.core.delete_favorite_list(&id).map_err(fail)
}

/// Adds a channel to a list ("default" is Favorites) or takes it out.
#[tauri::command]
pub async fn set_in_favorite_list(state: State<'_, AppState>, list_id: String, channel_id: String, member: bool) -> CmdResult<()> {
    state.core.set_in_favorite_list(&list_id, &channel_id, member).map_err(fail)
}

#[tauri::command]
pub async fn reorder_favorite_list(state: State<'_, AppState>, list_id: String, channel_ids: Vec<String>) -> CmdResult<()> {
    state.core.reorder_favorite_list(&list_id, &channel_ids).map_err(fail)
}

#[tauri::command]
pub async fn item_edits(state: State<'_, AppState>) -> CmdResult<Vec<ItemEdit>> {
    state.core.item_edits().map_err(fail)
}

/// A channel's or category's own name; none goes back to the provider's.
#[tauri::command]
pub async fn rename_item(state: State<'_, AppState>, item_id: String, name: Option<String>) -> CmdResult<()> {
    state.core.rename_item(&item_id, name.as_deref()).map_err(fail)
}

#[tauri::command]
pub async fn set_hidden(state: State<'_, AppState>, item_id: String, hidden: bool) -> CmdResult<()> {
    state.core.set_hidden(&item_id, hidden).map_err(fail)
}

/// Moves a channel's replays by some minutes (−30…30; 0 for none).
#[tauri::command]
pub async fn set_catchup_offset(state: State<'_, AppState>, channel_id: String, minutes: i64) -> CmdResult<()> {
    state.core.set_catchup_offset(&channel_id, minutes).map_err(fail)
}

/// "Always play this version" for a channel listed several times, or back to the best one.
#[tauri::command]
pub async fn set_preferred_version(state: State<'_, AppState>, channel_id: String, preferred: bool) -> CmdResult<()> {
    state.core.set_preferred_version(&channel_id, preferred).map_err(fail)
}

#[tauri::command]
pub async fn show_all_hidden(state: State<'_, AppState>) -> CmdResult<()> {
    state.core.show_all_hidden().map_err(fail)
}

#[tauri::command]
pub async fn set_locked(state: State<'_, AppState>, item_id: String, locked: bool) -> CmdResult<()> {
    state.core.set_locked(&item_id, locked).map_err(fail)
}

#[tauri::command]
pub async fn parental_status(state: State<'_, AppState>) -> CmdResult<ParentalStatus> {
    state.core.parental_status().map_err(fail)
}

/// Creates, changes or (no new PIN) removes the parental PIN. PINs are never logged.
#[tauri::command]
pub async fn set_pin(state: State<'_, AppState>, current_pin: Option<String>, new_pin: Option<String>) -> CmdResult<()> {
    state.core.set_pin(current_pin.as_deref(), new_pin.as_deref()).map_err(fail)
}

/// Enters the PIN for this session; false when it's wrong.
#[tauri::command]
pub async fn unlock_parental(state: State<'_, AppState>, pin: String) -> CmdResult<bool> {
    state.core.unlock(&pin).map_err(fail)
}

#[tauri::command]
pub async fn lock_parental(state: State<'_, AppState>) -> CmdResult<()> {
    state.core.lock();
    Ok(())
}

#[tauri::command]
pub async fn save_progress(state: State<'_, AppState>, progress: PlaybackProgress) -> CmdResult<()> {
    state.core.save_progress(progress).map_err(fail)
}

/// Marks films or episodes watched or unwatched (a season or series at once).
#[tauri::command]
pub async fn set_watched(state: State<'_, AppState>, items: Vec<PlaybackProgress>, watched: bool) -> CmdResult<()> {
    state.core.set_watched(&items, watched).map_err(fail)
}

/// Progress for many items at once (a series' episodes), in one call.
#[tauri::command]
pub async fn series_progress(state: State<'_, AppState>, item_ids: Vec<String>) -> CmdResult<Vec<PlaybackProgress>> {
    Ok(item_ids.iter().filter_map(|id| state.core.progress(id).ok().flatten()).collect())
}

#[tauri::command]
pub async fn progress(state: State<'_, AppState>, item_id: String) -> CmdResult<Option<PlaybackProgress>> {
    state.core.progress(&item_id).map_err(fail)
}

#[tauri::command]
pub async fn continue_watching(state: State<'_, AppState>, limit: usize) -> CmdResult<Vec<ContinueItem>> {
    let mut items = state.core.continue_watching(limit).map_err(fail)?;
    let addon = state.core.addon_progress(limit).map_err(fail)?;
    let cards = futures_util::future::join_all(addon.into_iter().map(|p| addon_continue_item(&state, p))).await;
    items.extend(cards.into_iter().flatten());
    state.core.without_dismissed(&mut items).map_err(fail)?;
    items.sort_by_key(|item| std::cmp::Reverse(item.updated_at));
    items.truncate(limit);
    Ok(items)
}

/// Home's New Releases: films from this year and last, one card each.
#[tauri::command]
pub async fn new_releases(state: State<'_, AppState>, limit: usize) -> CmdResult<Vec<Movie>> {
    state.core.new_releases(limit.min(100)).map_err(fail)
}

/// Home's New Episodes: followed series with episodes aired that haven't been watched.
#[tauri::command]
pub async fn new_episode_series(state: State<'_, AppState>, limit: usize) -> CmdResult<Vec<Series>> {
    // Add-on series aren't in the catalog: their episodes come from their add-on.
    let mut addon_shows = std::collections::HashMap::new();
    for id in state.core.pending_addon_series().map_err(fail)? {
        if let Some((media_type, media_id)) = addon_ref(&id)
            && let Ok(meta) = cached_meta(&state, media_type, media_id).await
            && let Some(series) = meta.series
        {
            addon_shows.insert(id, (series, meta.episodes));
        }
    }
    state.core.new_episode_series(limit.min(100), &addon_shows).map_err(fail)
}

/// Series with new episodes not looked at yet, and how many, for their badges.
#[tauri::command]
pub async fn new_episode_counts(state: State<'_, AppState>) -> CmdResult<std::collections::HashMap<String, i64>> {
    state.core.new_episode_counts().map_err(fail)
}

/// A series was opened: its "New Episode" badge goes. True when it had one.
#[tauri::command]
pub async fn mark_episodes_seen(state: State<'_, AppState>, series_id: String) -> CmdResult<bool> {
    state.core.mark_episodes_seen(&series_id).map_err(fail)
}

#[tauri::command]
pub async fn remove_from_continue_watching(state: State<'_, AppState>, item_id: String) -> CmdResult<()> {
    state.core.remove_from_continue_watching(&item_id).map_err(fail)
}

/// A Continue Watching card for a title from add-ons: the film or episode to
/// resume, or the next episode once one was finished. Details come from the
/// add-ons (once per session), so titles whose add-on is gone are left out.
async fn addon_continue_item(state: &AppState, p: PlaybackProgress) -> Option<ContinueItem> {
    let remaining = |p: &PlaybackProgress| (p.duration_seconds > 0.0).then(|| (p.duration_seconds - p.position_seconds).max(0.0));
    match p.kind {
        MediaKind::Movie if !p.finished => {
            let (media_type, media_id) = addon_ref(&p.item_id)?;
            let meta = cached_meta(state, media_type, media_id).await.ok()?;
            let movie = meta.movie?;
            Some(ContinueItem {
                item_id: p.item_id.clone(),
                kind: MediaKind::Movie,
                source_id: p.source_id.clone(),
                title: movie.title,
                subtitle: movie.year.map(|y| y.to_string()),
                artwork_url: meta.details.backdrop_urls.first().cloned().or(movie.backdrop_url).or(movie.poster_url),
                progress: (p.position_seconds / p.duration_seconds.max(1.0)).clamp(0.0, 1.0),
                remaining_seconds: remaining(&p),
                up_next: false,
                reason: None,
                updated_at: p.updated_at,
            })
        }
        MediaKind::Episode => {
            let series_id = p.series_id.clone()?;
            let (media_type, media_id) = addon_ref(&series_id)?;
            let meta = cached_meta(state, media_type, media_id).await.ok()?;
            let series = meta.series?;
            let (episode, up_next) = if p.finished {
                // After the furthest episode finished, not just the latest record:
                // a whole season marked watched at once shares one time.
                let finished: std::collections::HashSet<String> = state.core.finished_in_series(&series_id).ok()?.into_iter().collect();
                let last = meta.episodes.iter().rposition(|e| finished.contains(&e.id))?;
                (meta.episodes.get(last + 1)?, true)
            } else {
                (meta.episodes.iter().find(|e| e.id == p.item_id)?, false)
            };
            Some(ContinueItem {
                item_id: episode.id.clone(),
                kind: MediaKind::Episode,
                source_id: p.source_id.clone(),
                title: series.title,
                subtitle: Some(format!("S{:02} E{:02} · {}", episode.season, episode.number, episode.title)),
                artwork_url: episode.still_url.clone().or(series.backdrop_url).or(series.poster_url),
                progress: if up_next { 0.0 } else { (p.position_seconds / p.duration_seconds.max(1.0)).clamp(0.0, 1.0) },
                remaining_seconds: if up_next { None } else { remaining(&p) },
                up_next,
                reason: up_next.then(|| xorvio_core::next_episode_reason(episode.air_date, chrono::Utc::now().timestamp())),
                updated_at: p.updated_at,
            })
        }
        _ => None,
    }
}

#[tauri::command]
pub async fn recent_channels(state: State<'_, AppState>, limit: usize) -> CmdResult<Vec<Channel>> {
    state.core.recent_channels(limit).map_err(fail)
}

#[tauri::command]
pub async fn record_channel_watched(state: State<'_, AppState>, channel_id: String) -> CmdResult<()> {
    state.core.record_channel_watched(&channel_id).map_err(fail)
}

/// Notes a film or episode started playing, for Viewing History.
#[tauri::command]
pub async fn record_watched(state: State<'_, AppState>, item_id: String, kind: MediaKind, source_id: String, title: String, subtitle: Option<String>) -> CmdResult<()> {
    state.core.record_watched(&item_id, kind, &source_id, &title, subtitle.as_deref()).map_err(fail)
}

/// Viewing History (Settings › Privacy & Diagnostics), newest first.
#[tauri::command]
pub async fn history(state: State<'_, AppState>, limit: Option<usize>) -> CmdResult<Vec<HistoryEntry>> {
    state.core.history(limit.unwrap_or(500)).map_err(fail)
}

#[tauri::command]
pub async fn clear_history(state: State<'_, AppState>) -> CmdResult<()> {
    state.core.clear_history().map_err(fail)
}

// MARK: - Playback

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PlayRequest {
    Channel { channel_id: String },
    Replay { channel_id: String, start: Timestamp, end: Timestamp },
    Movie { movie_id: String },
    Episode { episode_id: String },
    Recording { recording_id: String },
    Target { target: StreamTarget },
}

fn extension_of(url: &str) -> Option<String> {
    StreamLocator::Url { url: url.to_string() }.extension()
}

/// Whether a request plays from the live buffer: a channel does when the
/// buffer is always on, unless it's asked to play `direct` (Multiview's
/// tiles, which are each a provider connection and an ffmpeg otherwise).
fn plays_from_buffer(channel: bool, buffer: LiveBuffer, direct: bool) -> bool {
    channel && buffer == LiveBuffer::Always && !direct
}

/// Resolves what to play into a concrete address, then lets the gateway
/// decide how the web view plays it. With the live buffer always on, a
/// channel plays from it (replays and recordings never do), unless `direct`.
#[tauri::command]
pub async fn play(state: State<'_, AppState>, request: PlayRequest, direct: Option<bool>) -> CmdResult<ResolvedStream> {
    let channel = matches!(request, PlayRequest::Channel { .. });
    let (target, hint) = resolve(&state, request)?;
    let (audio_language, buffer, window) = {
        let settings = state.settings.lock().unwrap();
        (settings.audio_language.clone(), settings.live_buffer, settings.live_buffer_seconds())
    };
    let hint = PlayHint { audio_language, ..hint };
    if plays_from_buffer(channel, buffer, direct.unwrap_or(false)) {
        match state.gateway.timeshift(target.clone(), hint.clone(), window).await {
            Ok(stream) => return Ok(stream),
            // Watching beats pausing: play it directly instead.
            Err(error) => tracing::warn!("the channel couldn't be buffered, so it plays directly: {error:#}"),
        }
    }
    let stream = state.gateway.prepare(target, hint).await.map_err(fail)?;
    // How it plays, for the diagnostic log (never where from).
    let notes = stream.info.as_ref().map(|info| info.notes.join("; ")).unwrap_or_default();
    tracing::info!("playing {} as {:?}{}{}", if channel { "a channel" } else { "a title" }, stream.mode, if notes.is_empty() { "" } else { ": " }, notes);
    Ok(stream)
}

/// Moves the live channel playing at `url` to a buffer that starts now, when
/// someone pauses it with the buffer set to start on pause.
#[tauri::command]
pub async fn start_timeshift(state: State<'_, AppState>, url: String) -> CmdResult<ResolvedStream> {
    let window = state.settings.lock().unwrap().live_buffer_seconds();
    state.gateway.timeshift_playing(&url, window).await.map_err(fail)
}

/// The address a request plays, and what's known about it beforehand.
pub(crate) fn resolve(state: &AppState, request: PlayRequest) -> CmdResult<(StreamTarget, PlayHint)> {
    let core = &state.core;
    Ok(match request {
        PlayRequest::Channel { channel_id } => {
            let channel = core.channel(&channel_id).map_err(fail)?.ok_or("That channel is no longer in your sources.")?;
            let live_format = core.list_sources().ok().and_then(|list| list.into_iter().find(|s| s.config.id == channel.source_id)).map(|s| s.config.live_format);
            let target = core.stream_target(&channel.source_id, &channel.stream, true).map_err(fail)?;
            (target, PlayHint { extension: channel.stream.extension(), live_format, duration_seconds: None, ..Default::default() })
        }
        PlayRequest::Replay { channel_id, start, end } => {
            let target = core.replay_target(&channel_id, start, end).map_err(fail)?;
            let extension = extension_of(&target.url);
            (target, PlayHint { extension, live_format: None, duration_seconds: Some((end - start) as f64), ..Default::default() })
        }
        PlayRequest::Movie { movie_id } => {
            let movie = core.movie(&movie_id).map_err(fail)?.ok_or("That film is no longer in your sources.")?;
            if matches!(movie.stream, StreamLocator::Addon { .. }) {
                return Err("Choose a copy of this film first.".into());
            }
            let target = core.stream_target(&movie.source_id, &movie.stream, false).map_err(fail)?;
            (target, PlayHint { extension: movie.stream.extension(), live_format: None, duration_seconds: movie.duration_seconds.map(|d| d as f64), ..Default::default() })
        }
        PlayRequest::Episode { episode_id } => {
            let episode = core.episode(&episode_id).map_err(fail)?.ok_or("That episode is no longer in your sources.")?;
            if matches!(episode.stream, StreamLocator::Addon { .. }) {
                return Err("Choose a copy of this episode first.".into());
            }
            let target = core.stream_target(&episode.source_id, &episode.stream, false).map_err(fail)?;
            (target, PlayHint { extension: episode.stream.extension(), live_format: None, duration_seconds: episode.duration_seconds.map(|d| d as f64), ..Default::default() })
        }
        PlayRequest::Recording { recording_id } => {
            let recording = state.recorder.list().map_err(fail)?.into_iter().find(|r| r.id == recording_id).ok_or("That recording is gone.")?;
            let path = recording.file_path.ok_or("This recording hasn't started yet.")?;
            let url = url::Url::from_file_path(&path).map_err(|_| "The recording's file path isn't valid.".to_string())?;
            let is_live = recording.status == RecordingStatus::Recording;
            let target = StreamTarget { url: url.to_string(), user_agent: None, headers: vec![], is_live };
            (target, PlayHint { extension: Some("ts".into()), live_format: None, duration_seconds: recording.recorded_seconds, ..Default::default() })
        }
        PlayRequest::Target { target } => {
            // Only add-on copies come this way, and they're web addresses. Files
            // on this computer play by their film, episode or recording, so the
            // interface can't name one.
            if !url::Url::parse(&target.url).is_ok_and(|url| matches!(url.scheme(), "http" | "https")) {
                return Err("That isn't a web address.".into());
            }
            let extension = extension_of(&target.url);
            (target, PlayHint { extension, live_format: None, duration_seconds: None, ..Default::default() })
        }
    })
}

#[tauri::command]
pub async fn seek_stream(state: State<'_, AppState>, url: String, offset_seconds: f64) -> CmdResult<ResolvedStream> {
    state.gateway.seek(&url, offset_seconds).await.map_err(fail)
}

/// Another audio track, or the audio moved, from where the player is.
#[tauri::command]
pub async fn restream(state: State<'_, AppState>, url: String, offset_seconds: f64, options: StreamOptions) -> CmdResult<ResolvedStream> {
    state.gateway.restream(&url, offset_seconds, options).await.map_err(fail)
}

/// Where the page that plays a title's YouTube trailer behind its page is.
#[tauri::command]
pub async fn trailer_page(state: State<'_, AppState>, youtube_id: String) -> CmdResult<String> {
    state.gateway.trailer_page(&youtube_id).ok_or_else(|| "That isn't a YouTube video.".to_string())
}

#[tauri::command]
pub async fn release_stream(state: State<'_, AppState>, url: String) -> CmdResult<()> {
    state.gateway.release(&url);
    Ok(())
}

/// The error status the player was last answered with for a stream (the
/// provider's 403 or 404, the gateway's 502), so a failure can say why.
#[tauri::command]
pub fn stream_failure(state: State<'_, AppState>, url: String) -> Option<u16> {
    state.gateway.failure_status(&url)
}

/// Where the player is in a remuxed stream, in the stream's own seconds, so
/// its ffmpeg keeps only a minute or so ahead.
#[tauri::command]
pub fn stream_position(state: State<'_, AppState>, url: String, seconds: f64) {
    state.gateway.report_position(&url, seconds);
}

/// The audio and subtitles chosen for a series, for its other episodes.
#[tauri::command]
pub async fn track_choice(state: State<'_, AppState>, series_id: String) -> CmdResult<Option<xorvio_core::TrackChoice>> {
    state.core.track_choice(&series_id).map_err(fail)
}

#[tauri::command]
pub async fn set_track_choice(state: State<'_, AppState>, series_id: String, choice: Option<xorvio_core::TrackChoice>) -> CmdResult<()> {
    state.core.set_track_choice(&series_id, choice.as_ref()).map_err(fail)
}

/// The seek bar's thumbnails for the film, episode or recording playing at
/// `url` (`item_id` and `source_id` are what's playing): ready, being made
/// (ask again in a few seconds), or none and why. A remote title's need a
/// connection beside the one playing, so they're made only where its source
/// allows that (see `thumbnail_allowance`), and stop when a recording from
/// the source starts or something else plays.
#[tauri::command]
pub async fn thumbnails(state: State<'_, AppState>, url: String, item_id: String, source_id: String) -> CmdResult<Thumbnails> {
    let (core, recorder) = (state.core.clone(), state.recorder.clone());
    let check: ConnectionCheck = Arc::new(move |host| {
        let sources = core.list_sources().map_err(fail)?;
        let limits: Vec<SourceLimit> = sources
            .iter()
            .map(|s| SourceLimit { id: &s.config.id, kind: s.config.kind, host: s.config.display_host.as_deref(), max_connections: s.status.max_connections })
            .collect();
        let (allowance, counted) = thumbnail_allowance(&limits, &source_id, host);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        // Running, or starting within a couple of minutes (with their padding).
        let recordings = match counted {
            Some(id) => recorder.streams_in_use(&id, now, now + 120).unwrap_or(1),
            None => 0,
        };
        xorvio_media::second_connection(allowance, recordings)
    });
    Ok(state.gateway.thumbnails(&url, &item_id, check))
}

/// A source's allowance of streams, as far as thumbnails need to know.
struct SourceLimit<'a> {
    id: &'a str,
    kind: SourceKind,
    host: Option<&'a str>,
    max_connections: Option<i64>,
}

/// How many streams a remote title's source allows, and whose recordings
/// count against them: a provider's reported limit (unknown when it doesn't
/// report one); none for the person's own media servers; and for an add-on's
/// link, the limit of a provider on the same host (an add-on can hand out a
/// provider's films), else none, since the services behind add-on links take
/// several connections at once.
fn thumbnail_allowance(sources: &[SourceLimit], source_id: &str, host: &str) -> (Allowance, Option<String>) {
    let limit = |source: &SourceLimit| match source.kind {
        SourceKind::Plex | SourceKind::Jellyfin | SourceKind::Emby | SourceKind::Folder => Allowance::Unlimited,
        _ => source.max_connections.map_or(Allowance::Unknown, Allowance::Streams),
    };
    if source_id == xorvio_addons::SOURCE_ID {
        let provider = sources.iter().find(|s| s.kind.is_provider() && !host.is_empty() && s.host.is_some_and(|h| h.eq_ignore_ascii_case(host)));
        return match provider {
            Some(provider) => (limit(provider), Some(provider.id.to_string())),
            None => (Allowance::Unlimited, None),
        };
    }
    match sources.iter().find(|s| s.id == source_id) {
        Some(source) => (limit(source), Some(source.id.to_string())),
        None => (Allowance::Unknown, None),
    }
}

/// What the web view decodes, reported once by the interface at launch.
#[tauri::command]
pub fn set_playback_support(state: State<'_, AppState>, support: PlaybackSupport) {
    state.gateway.set_support(support);
}

// MARK: - Other players

/// IINA, VLC and Infuse, those that are installed.
#[tauri::command]
pub async fn external_players() -> CmdResult<Vec<ExternalPlayer>> {
    tauri::async_runtime::spawn_blocking(players::installed).await.map_err(fail)
}

/// Hands a stream to another player on this computer: the one playing
/// (`stream_url`, its original rather than any remux), or what `request`
/// names. `title` is what that player shows; `position_seconds` is where a
/// film or episode starts, in the players that take a start time.
#[tauri::command]
pub async fn open_in_player(
    state: State<'_, AppState>,
    player: String,
    stream_url: Option<String>,
    request: Option<PlayRequest>,
    title: String,
    position_seconds: Option<f64>,
) -> CmdResult<()> {
    let address = match (stream_url, request) {
        (Some(url), _) => state.gateway.external(&url, &title).map_err(fail)?,
        (None, Some(request)) => {
            let (target, hint) = resolve(&state, request)?;
            state.gateway.external_target(target, &hint, &title).map_err(fail)?
        }
        (None, None) => return Err("There's nothing to open.".into()),
    };
    tauri::async_runtime::spawn_blocking(move || players::open(&player, &address, &title, position_seconds))
        .await
        .map_err(fail)?
        .map_err(|e| {
            tracing::warn!("opening in another player failed: {e:#}");
            fail(e)
        })
}

// MARK: - Recording

#[tauri::command]
pub async fn recordings(state: State<'_, AppState>) -> CmdResult<Vec<Recording>> {
    state.recorder.list().map_err(fail)
}

/// Providers limit simultaneous streams; say so before a recording would go over.
fn check_connections(state: &AppState, request: &ScheduleRequest) -> CmdResult<()> {
    let from = request.start - request.padding_before_seconds;
    let to = request.end + request.padding_after_seconds;
    check_streams(state, &request.source_id, from, to)
}

/// The same for a recording that holds a stream from `from` to `to`.
fn check_streams(state: &AppState, source_id: &str, from: Timestamp, to: Timestamp) -> CmdResult<()> {
    let Some((limit, server)) = stream_limit(state, source_id) else { return Ok(()) };
    // Saving from catch-up holds a stream now, whenever its programme aired.
    let overlapping = state.recorder.streams_in_use(source_id, from, to).unwrap_or(0) as i64;
    if overlapping + 1 > limit {
        return Err(format!(
            "{}, and {overlapping} other recording{} already overlap{} this one.",
            allowance(limit, server),
            if overlapping == 1 { "" } else { "s" },
            if overlapping == 1 { "s" } else { "" },
        ));
    }
    Ok(())
}

/// How many streams a source allows at once, when it says, and whether
/// that's a media server's tuners rather than a provider's connections.
fn stream_limit(state: &AppState, source_id: &str) -> Option<(i64, bool)> {
    let source = state.core.list_sources().ok()?.into_iter().find(|s| s.config.id == source_id)?;
    Some((source.status.max_connections?, source.config.kind.is_media_server()))
}

/// "Your provider allows 2 streams at a time", or a server's "Your server has 2 tuners".
fn allowance(limit: i64, server: bool) -> String {
    let plural = if limit == 1 { "" } else { "s" };
    if server { format!("Your server has {limit} tuner{plural}") } else { format!("Your provider allows {limit} stream{plural} at a time") }
}

/// What to say when one more stream from a source would go over its
/// provider's limit (or its server's tuners), with `watching` streams of it
/// playing and `recording` being recorded; nothing when it fits or there's no limit.
fn connections_warning(limit: Option<i64>, server: bool, watching: i64, recording: i64) -> Option<String> {
    let limit = limit.filter(|l| *l > 0)?;
    let in_use = watching + recording;
    if in_use < limit {
        return None;
    }
    let what = [(watching, "playing"), (recording, "recording")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, doing)| format!("{n} {doing}"))
        .collect::<Vec<_>>()
        .join(" and ");
    Some(format!("{}, and {in_use} {} already in use: {what}.", allowance(limit, server), if in_use == 1 { "is" } else { "are" }))
}

/// Before Multiview adds a tile: the warning when it would go over the
/// provider's limit, counting the `watching` streams of the source the
/// interface has playing and the recordings running on it now.
#[tauri::command]
pub async fn connection_warning(state: State<'_, AppState>, source_id: String, watching: i64) -> CmdResult<Option<String>> {
    let (limit, server) = stream_limit(&state, &source_id).map_or((None, false), |(limit, server)| (Some(limit), server));
    let recording = state
        .recorder
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.source_id == source_id && r.status == RecordingStatus::Recording)
        .count() as i64;
    Ok(connections_warning(limit, server, watching.max(0), recording))
}

#[tauri::command]
pub async fn schedule_recording(state: State<'_, AppState>, request: ScheduleRequest) -> CmdResult<Recording> {
    check_connections(&state, &request)?;
    state.recorder.schedule(request).map_err(fail)
}

#[tauri::command]
pub async fn record_now(state: State<'_, AppState>, request: ScheduleRequest) -> CmdResult<Recording> {
    check_connections(&state, &request)?;
    state.recorder.record_now(request).map_err(fail)
}

#[tauri::command]
pub async fn stop_recording(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.recorder.stop(&id).map_err(fail)
}

#[tauri::command]
pub async fn cancel_recording(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.recorder.cancel(&id).map_err(fail)
}

#[tauri::command]
pub async fn delete_recording(state: State<'_, AppState>, id: String, delete_file: bool) -> CmdResult<()> {
    state.recorder.delete(&id, delete_file).map_err(fail)
}

#[tauri::command]
pub async fn reveal_recording(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let recording = state.recorder.list().map_err(fail)?.into_iter().find(|r| r.id == id).ok_or("That recording is gone.")?;
    let path = recording.file_path.ok_or("This recording has no file yet.")?;
    app.opener().reveal_item_in_dir(path).map_err(fail)
}

/// Saves a programme that already aired from the channel's catch-up, with
/// as much of the padding asked for as the channel's archive still holds.
#[tauri::command]
pub async fn record_from_catchup(state: State<'_, AppState>, mut request: ScheduleRequest) -> CmdResult<Recording> {
    let channel = state.core.channel(&request.channel_id).map_err(fail)?.ok_or("That channel is no longer in your sources.")?;
    if channel.catchup_days <= 0 {
        return Err("This channel doesn't keep programmes to watch again.".into());
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    if request.end > now {
        return Err("This programme hasn't finished yet. Record it as it airs instead.".into());
    }
    // A few minutes' margin, so the start doesn't slip out of the archive while it saves.
    let oldest = now - channel.catchup_days * 86_400 + 5 * 60;
    if request.start < oldest {
        return Err("That's further back than the channel keeps programmes.".into());
    }
    request.padding_before_seconds = request.padding_before_seconds.min(request.start - oldest).max(0);
    request.padding_after_seconds = request.padding_after_seconds.min(now - request.end).max(0);
    let length = (request.end + request.padding_after_seconds) - (request.start - request.padding_before_seconds);
    check_streams(&state, &request.source_id, now, now + length)?;
    state.recorder.record_catchup(request).map_err(fail)
}

/// What's known of a recording's ad breaks: found (with them), being
/// looked for, never looked for, or why they couldn't be found.
#[tauri::command]
pub async fn ad_breaks(state: State<'_, AppState>, recording_id: String) -> CmdResult<AdBreaks> {
    state.recorder.ad_breaks(&recording_id).map_err(fail)
}

/// Looks for a finished recording's ad breaks now ("Find Ad Breaks"), in
/// the background; `ad_breaks` says when they're found.
#[tauri::command]
pub async fn find_ad_breaks(state: State<'_, AppState>, recording_id: String) -> CmdResult<AdBreaks> {
    state.recorder.find_ad_breaks(&recording_id).map_err(fail)
}

/// Every series recording, with its next airing and how many it has.
#[tauri::command]
pub async fn series_rules(state: State<'_, AppState>) -> CmdResult<Vec<SeriesRule>> {
    state.recorder.rules().map_err(fail)
}

/// Records every airing of a guide programme's title (or every new one) on
/// its channel or any channel. Looking through the guide can take a moment.
#[tauri::command]
pub async fn create_series_rule(state: State<'_, AppState>, program_id: String, channel_id: String, options: SeriesRuleOptions) -> CmdResult<SeriesRule> {
    let hit = state.core.program(&program_id).map_err(fail)?.ok_or("That programme is no longer in the guide.")?;
    let channel = state.core.channel(&channel_id).map_err(fail)?.ok_or("That channel is no longer in your sources.")?;
    let program = hit.program;
    // The programme chosen, so one that's on now records at once.
    let chosen = state.core.airings(&program.title, &channel.id, false, program.start, program.start + 1).map_err(fail)?.into_iter().next().map(crate::airing);
    let new = xorvio_media::NewSeriesRule {
        title: program.title.clone(),
        source_id: channel.source_id.clone(),
        channel_id: channel.id.clone(),
        channel_name: channel.name.clone(),
        channel_logo_url: channel.logo_url.clone(),
        artwork_url: program.image_url.clone(),
        options,
    };
    let recorder = state.recorder.clone();
    tokio::task::spawn_blocking(move || recorder.add_rule(new, chosen)).await.map_err(fail)?.map_err(fail)
}

#[tauri::command]
pub async fn update_series_rule(state: State<'_, AppState>, id: String, options: SeriesRuleOptions) -> CmdResult<SeriesRule> {
    let recorder = state.recorder.clone();
    tokio::task::spawn_blocking(move || recorder.update_rule(&id, options)).await.map_err(fail)?.map_err(fail)
}

/// Deletes a series and what it has scheduled. Its recordings go to the
/// Trash with `delete_recordings`, or stay as recordings of their own.
#[tauri::command]
pub async fn delete_series_rule(state: State<'_, AppState>, id: String, delete_recordings: bool) -> CmdResult<()> {
    let recorder = state.recorder.clone();
    tokio::task::spawn_blocking(move || recorder.delete_rule(&id, delete_recordings)).await.map_err(fail)?.map_err(fail)
}

// MARK: - Add-ons

#[tauri::command]
pub async fn addons(state: State<'_, AppState>) -> CmdResult<Vec<AddonInfo>> {
    Ok(state.addons.list())
}

#[tauri::command]
pub async fn install_addon(state: State<'_, AppState>, manifest_url: String) -> CmdResult<AddonInfo> {
    forget_discover(&state);
    state.addons.install(&manifest_url).await.map_err(fail)
}

#[tauri::command]
pub async fn remove_addon(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    forget_discover(&state);
    state.addons.remove(&id).map_err(fail)
}

#[tauri::command]
pub async fn set_addon_enabled(state: State<'_, AppState>, id: String, enabled: bool) -> CmdResult<()> {
    forget_discover(&state);
    state.addons.set_enabled(&id, enabled).map_err(fail)
}

/// How long Home reuses the add-on rows before asking the add-ons again.
const DISCOVER_FRESH: std::time::Duration = std::time::Duration::from_secs(10 * 60);

#[tauri::command]
pub async fn discover_rows(state: State<'_, AppState>) -> CmdResult<Vec<AddonCatalogRow>> {
    if state.addons.list().iter().all(|a| !a.enabled) {
        return Ok(vec![]);
    }
    if let Some((at, rows)) = state.discover_cache.lock().unwrap().as_ref()
        && at.elapsed() < DISCOVER_FRESH
    {
        let rows = rows.clone();
        return in_profile(&state, rows);
    }
    let rows = state.addons.discover_rows().await.map_err(fail)?;
    *state.discover_cache.lock().unwrap() = Some((std::time::Instant::now(), rows.clone()));
    in_profile(&state, rows)
}

/// Add-on changes show on Home at once.
fn forget_discover(state: &AppState) {
    *state.discover_cache.lock().unwrap() = None;
}

/// One page of an add-on catalogue, for the Films and Series grids when
/// add-ons lead.
#[tauri::command]
pub async fn addon_catalog(state: State<'_, AppState>, addon_id: String, media_type: String, catalog_id: String, genre: Option<String>, skip: usize) -> CmdResult<AddonCatalogRow> {
    let mut row = state.addons.catalog(&addon_id, &media_type, &catalog_id, genre.as_deref(), skip).await.map_err(fail)?;
    state.core.addon_titles_in_profile(&mut row.movies, &mut row.series).map_err(fail)?;
    Ok(row)
}

#[tauri::command]
pub async fn addon_meta(state: State<'_, AppState>, media_type: String, media_id: String) -> CmdResult<AddonMeta> {
    cached_meta(&state, &media_type, &media_id).await.map_err(fail)
}

#[tauri::command]
pub async fn addon_streams(state: State<'_, AppState>, media_type: String, media_id: String) -> CmdResult<Vec<AddonStream>> {
    state.addons.streams(&media_type, &media_id).await.map_err(fail)
}

#[tauri::command]
pub async fn addon_subtitle_text(state: State<'_, AppState>, url: String) -> CmdResult<String> {
    state.addons.subtitle_text(&url).await.map_err(fail)
}

/// The subtitle files beside a media folder's film or episode.
#[tauri::command]
pub async fn folder_subtitles(state: State<'_, AppState>, item_id: String) -> CmdResult<Vec<AddonSubtitle>> {
    state.core.folder_subtitles(&item_id).map_err(fail)
}

/// One of those subtitle files, as WebVTT.
#[tauri::command]
pub async fn folder_subtitle_text(state: State<'_, AppState>, url: String) -> CmdResult<String> {
    let bytes = state.core.folder_subtitle_file(&url).await.map_err(fail)?;
    Ok(xorvio_addons::to_webvtt(&bytes))
}

/// What to skip in an episode (from IntroDB), when someone has timed it.
#[tauri::command]
pub async fn episode_segments(state: State<'_, AppState>, imdb_id: String, season: i64, episode: i64) -> CmdResult<Option<EpisodeSegments>> {
    state.core.episode_segments(&imdb_id, season, episode).await.map_err(fail)
}

#[tauri::command]
pub async fn addon_subtitles(state: State<'_, AppState>, media_type: String, media_id: String) -> CmdResult<Vec<AddonSubtitle>> {
    state.addons.subtitles(&media_type, &media_id).await.map_err(fail)
}

#[derive(Debug, Serialize)]
pub struct AddonSearchResults {
    movies: Vec<Movie>,
    series: Vec<Series>,
}

#[tauri::command]
pub async fn addon_search(state: State<'_, AppState>, text: String) -> CmdResult<AddonSearchResults> {
    if text.trim().len() < 2 || state.addons.list().iter().all(|a| !a.enabled) {
        return Ok(AddonSearchResults { movies: vec![], series: vec![] });
    }
    let (mut movies, mut series) = state.addons.search(&text).await.map_err(fail)?;
    state.core.addon_titles_in_profile(&mut movies, &mut series).map_err(fail)?;
    Ok(AddonSearchResults { movies, series })
}

// MARK: - Settings

#[tauri::command]
pub async fn settings(state: State<'_, AppState>) -> CmdResult<Settings> {
    Ok(state.settings.lock().unwrap().clone())
}

#[tauri::command]
pub async fn save_settings(state: State<'_, AppState>, log: State<'_, Arc<DiagnosticsLog>>, settings: Settings) -> CmdResult<()> {
    {
        let current = state.settings.lock().unwrap();
        state.picked.check(settings.folders(), &current.folders().collect::<Vec<_>>())?;
    }
    let folder_changed = state.settings.lock().unwrap().recordings_folder != settings.recordings_folder;
    if folder_changed {
        state.recorder.set_recordings_dir(settings.recordings_dir()).map_err(fail)?;
    }
    state.recorder.set_folders(settings.recording_folders());
    state.recorder.set_default_padding(settings.record_padding_before * 60, settings.record_padding_after * 60);
    state.recorder.set_find_ad_breaks(settings.commercial_skip != CommercialSkip::Off);
    settings.save(&state.settings_path).map_err(fail)?;
    settings.apply_to(&state.core);
    log.set_detailed(settings.detailed_diagnostics);
    *state.settings.lock().unwrap() = settings;
    Ok(())
}

// MARK: - Storage

/// How much space Xorvio takes, for Settings › Storage.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageUsage {
    /// catalog.sqlite: the sources' channels, films, series, details and guide listings.
    catalog_bytes: u64,
    guide_listings: i64,
    /// library.sqlite: the watchlist, progress, favorites, groups and names.
    library_bytes: u64,
    /// Recordings whose files are still there, wherever they were saved.
    recordings_bytes: u64,
    recording_count: usize,
    temporary_bytes: u64,
}

/// Where the media gateway writes remuxed streams' subtitles while they play,
/// a folder per stream named by its id (`subtitle_root` in xorvio-media's
/// gateway.rs). The gateway clears it at launch and as streams are released.
fn temporary_dir() -> std::path::PathBuf {
    xorvio_media::temp_root().join("xorvio-subtitles")
}

/// Where live TV buffers are written, a folder per buffer (`timeshift::root`
/// in xorvio-media). Each goes when its channel stops; one left by a crash
/// goes at the next launch.
fn live_buffer_dir() -> std::path::PathBuf {
    xorvio_media::temp_root().join("xorvio-timeshift")
}

/// A buffer gets a segment every couple of seconds, so one nothing has
/// written to for a minute isn't running, here or in another copy of Xorvio.
fn is_stale(entry: &std::fs::DirEntry) -> bool {
    entry
        .metadata()
        .and_then(|meta| meta.modified())
        .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > std::time::Duration::from_secs(60)))
}

/// A database's size with its write-ahead log, which can hold a lot until
/// it's checkpointed.
fn database_bytes(path: &std::path::Path) -> u64 {
    ["", "-wal", "-shm"]
        .iter()
        .filter_map(|suffix| {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            std::fs::metadata(name).ok()
        })
        .map(|meta| meta.len())
        .sum()
}

/// Everything in a folder, however deep.
fn folder_bytes(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => folder_bytes(&entry.path()),
            Ok(_) => entry.metadata().map_or(0, |meta| meta.len()),
            Err(_) => 0,
        })
        .sum()
}

#[tauri::command]
pub async fn storage_usage(app: AppHandle, state: State<'_, AppState>) -> CmdResult<StorageUsage> {
    let data_dir = app.path().app_data_dir().map_err(fail)?;
    let core = state.core.clone();
    let recordings: Vec<String> = state.recorder.list().map_err(fail)?.into_iter().filter_map(|r| r.file_path).collect();
    // Counting listings and walking folders touches the disk; keep it off the async threads.
    tokio::task::spawn_blocking(move || {
        let sizes: Vec<u64> = recordings.iter().filter_map(|path| std::fs::metadata(path).ok()).map(|meta| meta.len()).collect();
        Ok(StorageUsage {
            catalog_bytes: database_bytes(&data_dir.join("catalog.sqlite")),
            guide_listings: core.guide_listing_count().map_err(fail)?,
            library_bytes: database_bytes(&data_dir.join("library.sqlite")),
            recordings_bytes: sizes.iter().sum(),
            recording_count: sizes.len(),
            temporary_bytes: folder_bytes(&temporary_dir()) + folder_bytes(&live_buffer_dir()),
        })
    })
    .await
    .map_err(fail)?
}

/// Deletes every guide listing and compacts the catalog, which can take a
/// while for a big guide. Refreshes bring the listings back.
#[tauri::command]
pub async fn clear_guide_listings(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    let core = state.core.clone();
    tokio::task::spawn_blocking(move || core.clear_guide_listings()).await.map_err(fail)?.map_err(fail)?;
    // The guide and Home's "on now" rows reload.
    let _ = app.emit("library-changed", ());
    Ok(())
}

/// Deletes subtitles left by remuxed streams, except those of the streams
/// still in use (what's playing), whose folder is named by the id in their
/// address, and live buffers no longer running.
#[tauri::command]
pub async fn clear_temporary_files(in_use: Vec<String>) -> CmdResult<()> {
    tokio::task::spawn_blocking(move || {
        for entry in std::fs::read_dir(live_buffer_dir()).into_iter().flatten().flatten() {
            if is_stale(&entry)
                && let Err(error) = std::fs::remove_dir_all(entry.path())
            {
                tracing::warn!("a live buffer couldn't be deleted: {error}");
            }
        }
        let Ok(entries) = std::fs::read_dir(temporary_dir()) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if in_use.iter().any(|url| url.split('/').any(|segment| segment == name)) {
                continue;
            }
            let path = entry.path();
            let removed = if entry.file_type().is_ok_and(|kind| kind.is_dir()) { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
            if let Err(error) = removed {
                tracing::warn!("a temporary file couldn't be deleted: {error}");
            }
        }
    })
    .await
    .map_err(fail)
}

/// Opens the folder new recordings go to. Recordings are only ever deleted
/// from Recordings, one by one.
#[tauri::command]
pub async fn show_recordings_folder(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    let dir = state.settings.lock().unwrap().recordings_dir();
    // The recorder makes it at the first recording; choosing a folder makes it too.
    std::fs::create_dir_all(&dir).map_err(|e| format!("The recordings folder couldn't be made: {e}"))?;
    app.opener().open_path(dir.to_string_lossy(), None::<&str>).map_err(fail)
}

#[tauri::command]
pub async fn choose_folder(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |folder| {
        let _ = tx.send(folder.and_then(|f| f.into_path().ok()).map(|p| p.to_string_lossy().into_owned()));
    });
    let chosen = rx.await.map_err(fail)?;
    chosen.iter().for_each(|path| state.picked.add(path));
    Ok(chosen)
}

#[tauri::command]
pub async fn choose_file(app: AppHandle, state: State<'_, AppState>, extensions: Vec<String>) -> CmdResult<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let extensions: Vec<&str> = extensions.iter().map(String::as_str).collect();
    app.dialog().file().add_filter("Playlist", &extensions).pick_file(move |file| {
        let _ = tx.send(file.and_then(|f| f.into_path().ok()).map(|p| p.to_string_lossy().into_owned()));
    });
    let chosen = rx.await.map_err(fail)?;
    chosen.iter().for_each(|path| state.picked.add(path));
    Ok(chosen)
}

/// Asks for one or more folders of films or series; empty when none were chosen.
#[tauri::command]
pub async fn choose_media_folders(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Vec<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title("Choose folders with films or series").set_can_create_directories(false).pick_folders(move |folders| {
        let paths = folders.unwrap_or_default().into_iter().filter_map(|f| f.into_path().ok()).map(|p| p.to_string_lossy().into_owned()).collect();
        let _ = tx.send(paths);
    });
    let chosen: Vec<String> = rx.await.map_err(fail)?;
    chosen.iter().for_each(|path| state.picked.add(path));
    Ok(chosen)
}

#[tauri::command]
pub async fn open_external(app: AppHandle, url: String) -> CmdResult<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Only web links can be opened.".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(fail)
}

/// Lets the interface put a line in the app's log (player errors, mostly),
/// since the web view's console isn't visible in a release build.
#[tauri::command]
pub async fn client_log(level: String, message: String) -> CmdResult<()> {
    let message: String = message.chars().take(2000).collect();
    match level.as_str() {
        "error" => tracing::error!(target: "interface", "{message}"),
        "warn" => tracing::warn!(target: "interface", "{message}"),
        _ => tracing::info!(target: "interface", "{message}"),
    }
    Ok(())
}

// MARK: - Diagnostics

/// The diagnostic log (Settings › Diagnostics), newest first.
#[tauri::command]
pub async fn diagnostics(log: State<'_, Arc<DiagnosticsLog>>) -> CmdResult<Vec<DiagnosticEvent>> {
    Ok(log.events())
}

#[tauri::command]
pub async fn clear_diagnostics(log: State<'_, Arc<DiagnosticsLog>>) -> CmdResult<()> {
    log.clear();
    Ok(())
}

/// Saves the diagnostic log where someone chooses, with every source's login
/// masked once more. `None` when they cancel.
#[tauri::command]
pub async fn export_diagnostics(app: AppHandle, state: State<'_, AppState>, log: State<'_, Arc<DiagnosticsLog>>) -> CmdResult<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_file_name("Xorvio Diagnostics.txt").add_filter("Text", &["txt"]).save_file(move |file| {
        let _ = tx.send(file.and_then(|f| f.into_path().ok()));
    });
    let Some(path) = rx.await.map_err(fail)? else { return Ok(None) };
    let text = state.core.redact_known(&log.export_text());
    std::fs::write(&path, text).map_err(|e| format!("The log couldn't be saved: {e}"))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_never_play_from_the_buffer() {
        assert!(plays_from_buffer(true, LiveBuffer::Always, false));
        assert!(!plays_from_buffer(true, LiveBuffer::Always, true));
        assert!(!plays_from_buffer(true, LiveBuffer::Paused, false));
        assert!(!plays_from_buffer(true, LiveBuffer::Off, false));
        // Replays, films and recordings never do.
        assert!(!plays_from_buffer(false, LiveBuffer::Always, false));
    }

    #[test]
    fn thumbnails_take_a_second_connection_only_where_a_source_allows_it() {
        let source = |id, kind, host, max_connections| SourceLimit { id, kind, host, max_connections };
        let sources = [
            source("one", SourceKind::Xtream, Some("panel.example:8080"), Some(1)),
            source("two", SourceKind::Xtream, Some("big.example"), Some(2)),
            source("m3u", SourceKind::M3uUrl, Some("lists.example"), None),
            source("plex", SourceKind::Plex, Some("192.168.1.20:32400"), None),
        ];
        let rule = |source_id: &str, host: &str| {
            let (allowance, counted) = thumbnail_allowance(&sources, source_id, host);
            (xorvio_media::second_connection(allowance, 0).is_ok(), counted)
        };
        assert_eq!(rule("one", "panel.example:8080"), (false, Some("one".into())), "one stream allowed: never a second");
        assert_eq!(rule("two", "big.example"), (true, Some("two".into())));
        assert_eq!(rule("m3u", "cdn.example"), (false, Some("m3u".into())), "a limit nobody reported isn't room to spare");
        assert_eq!(rule("plex", "192.168.1.20:32400"), (true, Some("plex".into())));
        assert_eq!(rule("gone", "x.example"), (false, None));
        // An add-on's link on a provider's host is that provider's stream.
        assert_eq!(rule("addon", "PANEL.example:8080"), (false, Some("one".into())));
        assert_eq!(rule("addon", "big.example"), (true, Some("two".into())));
        assert_eq!(rule("addon", "download.debrid.example"), (true, None));
    }

    #[test]
    fn warns_only_past_the_limit() {
        assert_eq!(connections_warning(None, false, 3, 1), None);
        assert_eq!(connections_warning(Some(0), false, 3, 1), None);
        assert_eq!(connections_warning(Some(2), false, 1, 0), None);
        assert_eq!(
            connections_warning(Some(2), false, 1, 1).as_deref(),
            Some("Your provider allows 2 streams at a time, and 2 are already in use: 1 playing and 1 recording.")
        );
        assert_eq!(
            connections_warning(Some(1), false, 1, 0).as_deref(),
            Some("Your provider allows 1 stream at a time, and 1 is already in use: 1 playing.")
        );
        assert_eq!(
            connections_warning(Some(3), false, 0, 4).as_deref(),
            Some("Your provider allows 3 streams at a time, and 4 are already in use: 4 recording.")
        );
        // A media server's limit is its tuners.
        assert_eq!(
            connections_warning(Some(2), true, 1, 1).as_deref(),
            Some("Your server has 2 tuners, and 2 are already in use: 1 playing and 1 recording.")
        );
        assert_eq!(connections_warning(Some(2), true, 1, 0), None);
    }
}

// MARK: - Library profiles and Home

/// Every library profile, and the one in use.
#[tauri::command]
pub async fn profiles(state: State<'_, AppState>) -> CmdResult<Profiles> {
    state.core.profiles().map_err(fail)
}

/// Switches profile; every list follows, so the interface reloads what it shows.
#[tauri::command]
pub async fn set_active_profile(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.core.set_active_profile(&id).map_err(fail)
}

/// Adds a profile (without an id) or saves changes to one; returns it as kept.
#[tauri::command]
pub async fn save_profile(state: State<'_, AppState>, profile: LibraryProfile) -> CmdResult<LibraryProfile> {
    state.core.save_profile(profile).map_err(fail)
}

#[tauri::command]
pub async fn delete_profile(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.core.delete_profile(&id).map_err(fail)
}

/// Every category with what its name says and how much it holds, for the profile editor.
#[tauri::command]
pub async fn profile_categories(state: State<'_, AppState>) -> CmdResult<Vec<ProfileCategory>> {
    state.core.profile_categories().map_err(fail)
}

/// The active profile's Home layout for `mode` ("sources" or "addons"); null for the standard one.
#[tauri::command]
pub async fn home_layout(state: State<'_, AppState>, mode: String) -> CmdResult<Option<serde_json::Value>> {
    state.core.home_layout(&mode).map_err(fail)
}

/// Saves the active profile's Home layout; null goes back to the standard one.
#[tauri::command]
pub async fn save_home_layout(state: State<'_, AppState>, mode: String, layout: Option<serde_json::Value>) -> CmdResult<()> {
    state.core.save_home_layout(&mode, layout).map_err(fail)
}

/// What's airing now for Home's On Now row: favourite and recent channels first.
#[tauri::command]
pub async fn on_now(state: State<'_, AppState>, limit: usize) -> CmdResult<Vec<ProgramHit>> {
    state.core.on_now(limit.min(60)).map_err(fail)
}

/// Add-on rows as the active profile shows them: rows it empties are left out.
fn in_profile(state: &AppState, rows: Vec<AddonCatalogRow>) -> CmdResult<Vec<AddonCatalogRow>> {
    let mut shown = Vec::with_capacity(rows.len());
    for mut row in rows {
        let had_titles = !row.movies.is_empty() || !row.series.is_empty();
        state.core.addon_titles_in_profile(&mut row.movies, &mut row.series).map_err(fail)?;
        if !had_titles || !row.movies.is_empty() || !row.series.is_empty() {
            shown.push(row);
        }
    }
    Ok(shown)
}
