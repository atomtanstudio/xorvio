//! The Tauri app: opens the stores, starts the media gateway and the recorder,
//! and exposes everything to the interface as commands (commands.rs).

mod awake;
mod backup;
mod cast;
mod commands;
mod diagnostics;
mod import;
mod media_library;
mod menu;
mod players;
mod popout;
mod reminders;
mod settings;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_window_state::StateFlags;
use xorvio_addons::Addons;
use xorvio_core::{Core, KeyringSecrets, SecretStore, TraktEvent};
use xorvio_media::{Airing, ChannelResolver, Gateway, Recorder, SeriesGuide, Stills, Tools};
use xorvio_model::{AddonCatalogRow, AddonMeta, RecordingStatus, StreamTarget};

pub use settings::Settings;

/// Sent with a source's id whenever a refresh of it ends, however it went,
/// so the interface's progress and badges follow background refreshes too.
pub(crate) const REFRESH_FINISHED: &str = "refresh-finished";
/// Room for seek-bar thumbnails (a couple of hundred films' worth); the
/// least recently used go first.
const THUMBNAIL_CACHE_BYTES: u64 = 300 * 1024 * 1024;

pub struct AppState {
    pub core: Arc<Core>,
    pub addons: Arc<Addons>,
    pub gateway: Arc<Gateway>,
    pub recorder: Arc<Recorder>,
    /// Frames of media folders' videos, for the cards of those without artwork.
    pub stills: Arc<Stills>,
    pub settings: Mutex<Settings>,
    pub settings_path: PathBuf,
    /// Add-on metas already fetched this session, so a title page's several
    /// requests (title, details, episodes) cost one network call.
    pub addon_metas: Mutex<HashMap<String, AddonMeta>>,
    /// Home's add-on rows, reused for a few minutes so Home opens quickly.
    pub discover_cache: Mutex<Option<(std::time::Instant, Vec<AddonCatalogRow>)>>,
}

/// Lets the recorder reopen a channel's stream (after a drop, or at the
/// start of a scheduled recording) without knowing about sources.
struct CoreResolver(Arc<Core>);

impl ChannelResolver for CoreResolver {
    fn resolve(&self, _source_id: &str, channel_id: &str) -> anyhow::Result<StreamTarget> {
        let channel = self
            .0
            .channel(channel_id)?
            .ok_or_else(|| anyhow::anyhow!("The channel is no longer in your sources."))?;
        self.0.stream_target(&channel.source_id, &channel.stream, true)
    }

    fn replay(&self, _source_id: &str, channel_id: &str, start: i64, end: i64) -> anyhow::Result<StreamTarget> {
        self.0.replay_target(channel_id, start, end)
    }

    fn max_connections(&self, source_id: &str) -> Option<i64> {
        let sources = self.0.list_sources().ok()?;
        sources.into_iter().find(|s| s.config.id == source_id)?.status.max_connections
    }
}

/// Lets series recordings find their programme in the guide.
struct CoreGuide(Arc<Core>);

impl SeriesGuide for CoreGuide {
    fn airings(&self, title: &str, channel_id: &str, any_channel: bool, from: i64, to: i64) -> anyhow::Result<Vec<Airing>> {
        Ok(self.0.airings(title, channel_id, any_channel, from, to)?.into_iter().map(airing).collect())
    }
}

/// The guide's airing as the recorder takes it.
pub(crate) fn airing(found: xorvio_core::Airing) -> Airing {
    Airing { program: found.program, channel: found.channel, family: found.family }
}

pub fn run() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{Layer, filter::Targets};

    // Everything goes to the usual output, logins masked; Settings ›
    // Diagnostics keeps Xorvio's own events and every library's warnings
    // (diagnostics.rs).
    let diagnostics = Arc::new(diagnostics::DiagnosticsLog::default());
    let ours = ["xorvio", "xorvio_lib", "xorvio_core", "xorvio_media", "xorvio_addons", "interface"];
    let kept = ours.into_iter().fold(Targets::new().with_default(tracing::Level::WARN), |targets, target| targets.with_target(target, tracing::Level::DEBUG));
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(diagnostics::RedactedStderr)
                .with_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())),
        )
        .with(diagnostics::Capture(diagnostics.clone()).with_filter(kept))
        .init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        // The menu bar (menu.rs): its items go to the window that acts on them.
        .menu(menu::build)
        .on_menu_event(menu::handle)
        // The window opens as it was left: its size and place, maximized or
        // full screen. Not hidden, though (it hides to keep running for
        // recordings), and the player leaves the full screen it made itself.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED | StateFlags::FULLSCREEN)
                .build(),
        )
        // Media servers' artwork, fetched with the server's token, which the
        // catalog and the interface never hold (xorvio-core's artwork_url).
        .register_asynchronous_uri_scheme_protocol(xorvio_core::ARTWORK_SCHEME, |context, request, responder| {
            let app = context.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                responder.respond(artwork_response(&app, request.uri().path()).await);
            });
        })
        // Artwork at plain-http addresses, which the web view would ask for
        // over https (xorvio-core's images module).
        .register_asynchronous_uri_scheme_protocol(xorvio_core::IMAGE_SCHEME, |context, request, responder| {
            let app = context.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                responder.respond(image_response(&app, request.uri().path()).await);
            });
        })
        // Artwork in media folders: only image files inside a folder source's
        // own folders (xorvio-core's folder module), since assetProtocol is off.
        .register_asynchronous_uri_scheme_protocol(xorvio_core::FOLDER_SCHEME, |context, request, responder| {
            let app = context.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                responder.respond(folder_image_response(&app, request.uri().path()).await);
            });
        })
        // Frames of media folders' videos for their cards (xorvio-media's stills).
        .register_asynchronous_uri_scheme_protocol(xorvio_media::STILL_SCHEME, |context, request, responder| {
            let app = context.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                responder.respond(still_response(&app, request.uri().path()));
            });
        })
        .setup(move |app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let secrets: Arc<dyn SecretStore> = Arc::new(KeyringSecrets);
            // A restore chosen before the restart goes in before anything opens the data.
            let restored = backup::apply_pending_restore(&data_dir, secrets.as_ref());
            let settings_path = data_dir.join("settings.json");
            let settings = Settings::load(&settings_path);
            diagnostics.set_detailed(settings.detailed_diagnostics);
            app.manage(diagnostics.clone());
            app.manage(popout::Handoff::default());

            let core = Core::open(&data_dir, secrets.clone())?;
            settings.apply_to(&core);
            let trakt_events = app.handle().clone();
            core.set_trakt_listener(Arc::new(move |event| match event {
                TraktEvent::Status(status) => {
                    let _ = trakt_events.emit("trakt-changed", &status);
                }
                TraktEvent::LibraryChanged => {
                    let _ = trakt_events.emit("library-changed", ());
                }
            }));
            let addons = Addons::open(&data_dir, commands::addon_secrets(secrets.clone()))?;
            let tools = Tools::locate();
            if tools.is_none() {
                tracing::warn!("ffmpeg wasn't found; recording and some formats won't work");
            }

            let stills = Arc::new(Stills::new(data_dir.join("Stills"), tools.clone()));
            let ffprobe = tools.as_ref().map(|tools| tools.ffprobe.clone());
            let handle = app.handle().clone();
            let keep_awake = Arc::new(awake::KeepAwake::default());
            let (gateway, recorder) = tauri::async_runtime::block_on(async {
                let gateway = Gateway::start(tools.clone()).await?;
                gateway.set_thumbnail_cache(data_dir.join("Thumbnails"), THUMBNAIL_CACHE_BYTES);
                let recorder = Recorder::open(&data_dir, settings.recordings_dir(), tools, Arc::new(CoreResolver(core.clone())))?;
                recorder.set_find_ad_breaks(settings.commercial_skip != settings::CommercialSkip::Off);
                recorder.set_folders(settings.recording_folders());
                recorder.set_default_padding(settings.record_padding_before * 60, settings.record_padding_after * 60);
                recorder.set_guide(Arc::new(CoreGuide(core.clone())));
                // A series scheduled some, or old ones were cleared out.
                let list_events = handle.clone();
                recorder.on_list_change(Arc::new(move || {
                    let _ = list_events.emit("recordings-changed", ());
                    update_tray(&list_events);
                }));
                let events = handle.clone();
                let awake = keep_awake.clone();
                let weak = Arc::downgrade(&recorder);
                recorder.start(Arc::new(move |recording| {
                    let _ = events.emit("recording-changed", &recording);
                    if let Some(recorder) = weak.upgrade() {
                        awake.set(recorder.active_count() > 0);
                    }
                    update_tray(&events);
                }));
                anyhow::Ok((gateway, recorder))
            })?;

            app.manage(cast::Casting::default());
            app.manage(AppState {
                core,
                addons,
                gateway,
                recorder,
                stills,
                settings: Mutex::new(settings),
                settings_path,
                addon_metas: Mutex::new(HashMap::new()),
                discover_cache: Mutex::new(None),
            });
            app.manage(backup::BackupService::new(data_dir.clone(), secrets.clone(), restored));

            build_tray(app.handle())?;
            spawn_background_refresh(app.handle().clone());
            spawn_trakt_schedule(app.handle().clone());
            reminders::handle_clicks(app.handle());
            reminders::spawn(app.handle().clone());
            backup::spawn(app.handle().clone());
            reminders::spawn_new_episode_checks(app.handle().clone());
            media_library::spawn(app.handle().clone(), ffprobe);
            Ok(())
        })
        .on_window_event(|window, event| {
            // With recordings or reminders still to come, closing the window
            // keeps Xorvio running in the menu bar so they happen.
            // (The pop-out player's window just closes.)
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
            {
                let state = window.state::<AppState>();
                let keep = state.settings.lock().unwrap().keep_running_for_recordings;
                if keep && (pending_recordings(&state) > 0 || pending_reminders(&state)) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(commands::handler())
        .build(tauri::generate_context!())
        .expect("Xorvio couldn't start");

    app.run(|handle, event| {
        // Live buffers' ffmpegs would otherwise keep reading the provider.
        if let RunEvent::Exit = event {
            // A TV playing from this computer is told to stop first.
            cast::stop_on_quit(handle);
            if let Some(state) = handle.try_state::<AppState>() {
                state.gateway.release_all();
            }
        }
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen { .. } = event {
            show_main_window(handle);
        }
    });
}

/// The reply to a `xorvio-server` request: the image, or 404 for anything
/// that isn't a media server's artwork.
async fn artwork_response(app: &AppHandle, path: &str) -> tauri::http::Response<Vec<u8>> {
    let empty = |status: u16| tauri::http::Response::builder().status(status).body(Vec::new()).unwrap_or_default();
    let Some((source_id, server_path)) = xorvio_core::parse_artwork_path(path) else { return empty(404) };
    let Some(state) = app.try_state::<AppState>() else { return empty(503) };
    match state.core.artwork(&source_id, &server_path).await {
        Ok((bytes, content_type)) => tauri::http::Response::builder()
            .status(200)
            .header("Content-Type", content_type.unwrap_or_else(|| "image/jpeg".into()))
            // Artwork paths carry the image's version; a week is safe.
            .header("Cache-Control", "max-age=604800")
            .body(bytes)
            .unwrap_or_default(),
        Err(error) => {
            tracing::debug!("artwork couldn't be fetched: {error}");
            empty(404)
        }
    }
}

/// The reply to an `xorvio-image` request: the image at the plain-http
/// address it names, or 404 when there isn't one.
async fn image_response(app: &AppHandle, path: &str) -> tauri::http::Response<Vec<u8>> {
    let empty = |status: u16| tauri::http::Response::builder().status(status).body(Vec::new()).unwrap_or_default();
    let Some(link) = xorvio_core::parse_image_path(path) else { return empty(404) };
    let Some(state) = app.try_state::<AppState>() else { return empty(503) };
    match state.core.plain_http_image(&link).await {
        Ok((bytes, content_type)) => tauri::http::Response::builder()
            .status(200)
            .header("Content-Type", content_type)
            .header("Cache-Control", "max-age=86400")
            .body(bytes)
            .unwrap_or_default(),
        Err(error) => {
            tracing::debug!("an image couldn't be fetched: {error}");
            empty(404)
        }
    }
}

/// The reply to an `xorvio-folder` request: an image file inside a media
/// folder source's folders, or 404 for anything else.
async fn folder_image_response(app: &AppHandle, path: &str) -> tauri::http::Response<Vec<u8>> {
    let empty = |status: u16| tauri::http::Response::builder().status(status).body(Vec::new()).unwrap_or_default();
    let Some((source_id, file)) = xorvio_core::parse_folder_path(path) else { return empty(404) };
    let Some(state) = app.try_state::<AppState>() else { return empty(503) };
    match state.core.folder_image(&source_id, &file).await {
        Ok((bytes, content_type)) => tauri::http::Response::builder()
            .status(200)
            .header("Content-Type", content_type)
            // Addresses carry the file's modification time, so a changed image is a new address.
            .header("Cache-Control", "max-age=604800")
            .body(bytes)
            .unwrap_or_default(),
        Err(error) => {
            tracing::debug!("a media folder's image couldn't be served: {error}");
            empty(404)
        }
    }
}

fn still_response(app: &AppHandle, path: &str) -> tauri::http::Response<Vec<u8>> {
    let empty = |status: u16| tauri::http::Response::builder().status(status).body(Vec::new()).unwrap_or_default();
    let Some(state) = app.try_state::<AppState>() else { return empty(503) };
    let Some(file) = state.stills.path(path.trim_start_matches('/')) else { return empty(404) };
    match std::fs::read(file) {
        Ok(bytes) => tauri::http::Response::builder()
            .status(200)
            .header("Content-Type", "image/jpeg")
            // Named for the video as it was, so a changed video is a new address.
            .header("Cache-Control", "max-age=31536000, immutable")
            .body(bytes)
            .unwrap_or_default(),
        Err(_) => empty(404),
    }
}

fn pending_recordings(state: &AppState) -> usize {
    state
        .recorder
        .list()
        .map(|list| list.iter().filter(|r| matches!(r.status, RecordingStatus::Scheduled | RecordingStatus::Recording)).count())
        .unwrap_or(0)
}

/// A programme on the watchlist with a reminder that hasn't started yet.
fn pending_reminders(state: &AppState) -> bool {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    state
        .core
        .watchlist()
        .is_ok_and(|list| list.iter().any(|entry| entry.remind && entry.start.is_some_and(|start| start > now)))
}

pub(crate) fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Xorvio", true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", "No recordings scheduled", false, None::<&str>)?;
    let recordings = MenuItem::with_id(app, "recordings", "Show Recordings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Xorvio", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &status, &recordings, &PredefinedMenuItem::separator(app)?, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main").menu(&menu).tooltip("Xorvio").show_menu_on_left_click(true);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone()).icon_as_template(false);
    }
    tray.on_menu_event(|app, event| match event.id.as_ref() {
        "open" => show_main_window(app),
        "recordings" => {
            show_main_window(app);
            let _ = app.emit("navigate", "recordings");
        }
        "quit" => app.exit(0),
        _ => {}
    })
    .build(app)?;
    app.manage(TrayStatus(status));
    update_tray(app);
    Ok(())
}

struct TrayStatus(MenuItem<tauri::Wry>);

/// The tray menu's status line: what's recording and what's next.
fn update_tray(app: &AppHandle) {
    let (Some(state), Some(status)) = (app.try_state::<AppState>(), app.try_state::<TrayStatus>()) else { return };
    let list = state.recorder.list().unwrap_or_default();
    let active = list.iter().filter(|r| r.status == RecordingStatus::Recording).count();
    let scheduled = list.iter().filter(|r| r.status == RecordingStatus::Scheduled).count();
    let text = match (active, scheduled) {
        (0, 0) => "No recordings scheduled".to_string(),
        (0, n) => format!("{n} recording{} scheduled", if n == 1 { "" } else { "s" }),
        (a, 0) => format!("Recording {a} now"),
        (a, n) => format!("Recording {a} now, {n} scheduled"),
    };
    let _ = status.0.set_text(text);
}

/// Refreshes sources whose interval has passed, and added guides a day old,
/// a minute after launch and then every fifteen minutes.
fn spawn_background_refresh(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            let state = app.state::<AppState>();
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
            let mut refreshed = false;
            if let Ok(due) = state.core.sources_due_for_refresh(now) {
                for id in due {
                    let events = app.clone();
                    let progress = move |p: xorvio_model::RefreshProgress| {
                        let _ = events.emit("refresh-progress", &p);
                    };
                    let outcome = state.core.refresh_source(&id, &progress).await;
                    let _ = app.emit(REFRESH_FINISHED, &id);
                    match outcome {
                        Ok(_) => {
                            refreshed = true;
                            let _ = app.emit("library-changed", ());
                        }
                        Err(e) => tracing::warn!("background refresh failed: {e}"),
                    }
                }
            }
            if let Ok(due) = state.core.guides_due_for_refresh(now) {
                for id in due {
                    match state.core.refresh_guide_source(&id).await {
                        Ok(_) => {
                            refreshed = true;
                            let _ = app.emit("library-changed", ());
                        }
                        Err(e) => tracing::warn!("background guide refresh failed: {e}"),
                    }
                }
            }
            // New listings can hold new episodes for series recordings.
            if refreshed {
                state.recorder.evaluate_rules_soon();
            }
            tokio::time::sleep(Duration::from_secs(15 * 60)).await;
        }
    });
}

/// Syncs with Trakt shortly after launch and then every fifteen minutes,
/// while an account is connected. Core runs one sync at a time, so one
/// started by hand meanwhile isn't repeated.
fn spawn_trakt_schedule(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            let core = app.state::<AppState>().core.clone();
            if core.trakt_status().connected
                && let Err(error) = core.trakt_sync().await
            {
                tracing::warn!("the Trakt sync couldn't run: {error}");
            }
            tokio::time::sleep(Duration::from_secs(15 * 60)).await;
        }
    });
}

/// A Trakt sync in the background, as after connecting.
pub(crate) fn spawn_trakt_sync(core: Arc<Core>) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = core.trakt_sync().await {
            tracing::warn!("the Trakt sync couldn't run: {error}");
        }
    });
}
