//! The menu bar. Xorvio's own items are built here and, when chosen (by
//! click or shortcut), sent as a `menu` event naming the item to the window
//! that acts on it: the main window, or for Playback the window the video is
//! in. The interface performs it there (src/lib/commands.ts), with the same
//! functions as its buttons and keys. The standard items (Edit's copy and
//! paste, full screen, the Window menu, Quit) are the system's own.
//!
//! On a Mac the web page sees a shortcut before the menu does, and the menu
//! acts on it only when the page lets it pass; the interface leaves these
//! shortcuts alone, so each one acts once, through here. The exceptions are
//! the ⌘-arrows and ⌥Space, which the web view would take for scrolling
//! first: the interface does those itself, outside text fields, and the
//! menu doesn't see them. On Windows and Linux the menu takes them first.
//!
//! The interface reports what can act now (`set_menu_state`), and items
//! that can't are disabled.

use std::collections::HashMap;

use serde::Deserialize;
use tauri::menu::{AboutMetadata, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Window, Wry};
use tauri_plugin_opener::OpenerExt;

/// Help › Xorvio Documentation: the README on GitHub.
pub const DOCUMENTATION_URL: &str = "https://github.com/atomtanstudio/xorvio#readme";

/// What each of Xorvio's own items sends, with the item's id as its payload.
const MENU_EVENT: &str = "menu";

const HELP_DOCUMENTATION: &str = "help.documentation";
/// Full screen on Windows and Linux, where there's no standard item for it.
#[cfg(not(target_os = "macos"))]
const VIEW_FULL_SCREEN: &str = "view.fullScreen";

/// One of Xorvio's own items: its id (the command the interface performs),
/// its title and its shortcut on a Mac and elsewhere. The interface's
/// commands.ts lists the same shortcuts for the browser preview.
struct Item {
    id: &'static str,
    title: &'static str,
    mac: Option<&'static str>,
    other: Option<&'static str>,
}

const fn item(id: &'static str, title: &'static str, mac: Option<&'static str>, other: Option<&'static str>) -> Item {
    Item { id, title, mac, other }
}

// Text fields keep their own word and line moves on Windows and Linux
// (Ctrl+arrows), so skipping and changing channel there are the player's
// own keys (arrows, Page Up and Page Down) rather than menu shortcuts.
const SETTINGS: Item = item("app.settings", "Settings…", Some("Cmd+,"), Some("Ctrl+,"));
const ADD_SOURCE: Item = item("file.addSource", "Add Source…", Some("Cmd+N"), Some("Ctrl+N"));
const REFRESH_ALL: Item = item("file.refreshAll", "Refresh All Sources", Some("Cmd+Shift+R"), Some("Ctrl+Shift+R"));
const SIDEBAR: Item = item("view.sidebar", "Hide Sidebar", Some("Ctrl+Cmd+S"), Some("Ctrl+Alt+S"));
const GO: [Item; 8] = [
    item("go.home", "Home", Some("Cmd+1"), Some("Ctrl+1")),
    item("go.watchlist", "Watchlist", Some("Cmd+2"), Some("Ctrl+2")),
    item("go.live", "Live TV", Some("Cmd+3"), Some("Ctrl+3")),
    item("go.movies", "Movies", Some("Cmd+4"), Some("Ctrl+4")),
    item("go.series", "Series", Some("Cmd+5"), Some("Ctrl+5")),
    item("go.recordings", "Recordings", Some("Cmd+6"), Some("Ctrl+6")),
    item("go.sources", "Sources", Some("Cmd+7"), Some("Ctrl+7")),
    item("go.settings", "Settings", None, None),
];
const SEARCH: Item = item("go.search", "Search", Some("Cmd+F"), Some("Ctrl+F"));
const BACK: Item = item("go.back", "Back", Some("Cmd+["), Some("Alt+Left"));
const PLAY_PAUSE: Item = item("playback.playPause", "Play", Some("Alt+Space"), None);
const PAUSE_ALL: Item = item("playback.pauseAll", "Pause All Tiles", Some("Alt+Shift+Space"), None);
const STOP: Item = item("playback.stop", "Stop", None, None);
const SKIP_BACK: Item = item("playback.skipBack", "Skip Back 10 Seconds", Some("Cmd+Left"), None);
const SKIP_FORWARD: Item = item("playback.skipForward", "Skip Forward 10 Seconds", Some("Cmd+Right"), None);
const GO_LIVE: Item = item("playback.goLive", "Go Live", Some("Cmd+Shift+L"), Some("Ctrl+Shift+L"));
// Up the channel list and down it, as the player's channel arrows go.
// Next is channel up, as ↑ is in the player and on a remote (and in the Swift app).
const PREVIOUS_CHANNEL: Item = item("playback.previousChannel", "Previous Channel", Some("Cmd+Down"), None);
const NEXT_CHANNEL: Item = item("playback.nextChannel", "Next Channel", Some("Cmd+Up"), None);
const LAST_CHANNEL: Item = item("playback.lastChannel", "Last Channel", Some("Cmd+L"), Some("Ctrl+L"));
const MUTE: Item = item("playback.mute", "Mute", None, None);
const MULTIVIEW: Item = item("playback.multiview", "Multiview", Some("Cmd+Shift+M"), Some("Ctrl+Shift+M"));
const POP_OUT: Item = item("playback.popOut", "Pop Out Video", Some("Alt+Cmd+P"), Some("Ctrl+Alt+P"));
// Something small in a corner of what's playing. X does both in the window;
// a letter alone can't be a menu shortcut, as text fields need it.
const INSET: Item = item("playback.inset", "Add an Inset…", None, None);
const SWAP_INSET: Item = item("playback.swapInset", "Swap Inset", None, None);
/// What's in Multiview, which is in the main window whatever's popped out.
const MULTIVIEW_ITEMS: [&str; 3] = [MULTIVIEW.id, INSET.id, SWAP_INSET.id];

/// Xorvio's own items, by id, to enable, disable and retitle.
struct MenuItems(HashMap<&'static str, MenuItem<Wry>>);

/// What the interface says can act now. The main window sends `app`; the
/// window the video is in sends `playback`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MenuState {
    app: Option<AppMenuState>,
    playback: Option<PlaybackMenuState>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct AppMenuState {
    has_sources: bool,
    can_go_back: bool,
    sidebar_hidden: bool,
    multiview: bool,
    /// Something plays in the main window's player, so an inset can be added to it.
    can_add_inset: bool,
    /// Multiview has two tiles, which can be an inset.
    can_inset: bool,
    /// The two are an inset: one small in a corner of the other.
    inset: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct PlaybackMenuState {
    /// Something is in the player.
    playing: bool,
    paused: bool,
    muted: bool,
    /// It can be skipped through: a film, an episode or a buffered channel.
    seekable: bool,
    /// A live channel playing from its buffer, behind live.
    behind_live: bool,
    /// A live channel, so there are channels either side.
    channel: bool,
    /// There's a channel to go back to.
    last_channel: bool,
    /// Multiview is open: Play/Pause, the skips and Go Live are for the tile
    /// heard, and the channel items move the sound between tiles.
    multiview: bool,
    /// Every Multiview tile is paused, so Pause All Tiles plays them.
    all_paused: bool,
    /// This is the pop-out player's window.
    popped_out: bool,
}

/// The menu bar, built once at launch.
pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let mut items = HashMap::new();
    let mut own = |item: &Item, enabled: bool| -> tauri::Result<MenuItem<Wry>> {
        let shortcut = if cfg!(target_os = "macos") { item.mac } else { item.other };
        let made = MenuItem::with_id(app, item.id, item.title, enabled, shortcut)?;
        items.insert(item.id, made.clone());
        Ok(made)
    };
    let separator = || PredefinedMenuItem::separator(app);
    let info = app.package_info();
    let about = AboutMetadata {
        name: Some(info.name.clone()),
        version: Some(info.version.to_string()),
        copyright: app.config().bundle.copyright.clone(),
        license: Some("GPL-3.0-or-later".into()),
        website: Some(DOCUMENTATION_URL.into()),
        comments: Some("Live TV, films and series from your own sources. Xorvio ships no TV content of its own.".into()),
        credits: Some("Film and series details from TMDB and TVmaze. This product uses the TMDB API but is not endorsed or certified by TMDB.".into()),
        ..Default::default()
    };

    #[cfg(target_os = "macos")]
    let app_menu = Submenu::with_items(
        app,
        info.name.clone(),
        true,
        &[
            &PredefinedMenuItem::about(app, None, Some(about))?,
            &separator()?,
            &own(&SETTINGS, true)?,
            &separator()?,
            &PredefinedMenuItem::services(app, None)?,
            &separator()?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &separator()?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    let add_source = own(&ADD_SOURCE, true)?;
    let refresh_all = own(&REFRESH_ALL, true)?;
    #[cfg(target_os = "macos")]
    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[&add_source, &refresh_all, &separator()?, &PredefinedMenuItem::close_window(app, None)?],
    )?;
    #[cfg(not(target_os = "macos"))]
    let file_menu = Submenu::with_items(
        app,
        "&File",
        true,
        &[&add_source, &refresh_all, &separator()?, &own(&SETTINGS, true)?, &separator()?, &PredefinedMenuItem::quit(app, None)?],
    )?;

    let edit_menu = Submenu::with_items(
        app,
        if cfg!(target_os = "macos") { "Edit" } else { "&Edit" },
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &separator()?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let sidebar = own(&SIDEBAR, true)?;
    #[cfg(target_os = "macos")]
    let full_screen = PredefinedMenuItem::fullscreen(app, None)?;
    #[cfg(not(target_os = "macos"))]
    let full_screen = MenuItem::with_id(app, VIEW_FULL_SCREEN, "Full Screen", true, Some("F11"))?;
    let view_menu = Submenu::with_items(app, if cfg!(target_os = "macos") { "View" } else { "&View" }, true, &[&sidebar, &separator()?, &full_screen])?;

    let mut go_items: Vec<MenuItem<Wry>> = Vec::new();
    for item in &GO {
        go_items.push(own(item, true)?);
    }
    let search = own(&SEARCH, true)?;
    let back = own(&BACK, false)?;
    let mut go_entries: Vec<&dyn IsMenuItem<Wry>> = go_items.iter().map(|item| item as &dyn IsMenuItem<Wry>).collect();
    let go_separator = separator()?;
    go_entries.extend([&go_separator as &dyn IsMenuItem<Wry>, &search, &back]);
    let go_menu = Submenu::with_items(app, if cfg!(target_os = "macos") { "Go" } else { "&Go" }, true, &go_entries)?;

    let playback_menu = Submenu::with_items(
        app,
        if cfg!(target_os = "macos") { "Playback" } else { "&Playback" },
        true,
        &[
            &own(&PLAY_PAUSE, false)?,
            &own(&PAUSE_ALL, false)?,
            &own(&STOP, false)?,
            &separator()?,
            &own(&SKIP_BACK, false)?,
            &own(&SKIP_FORWARD, false)?,
            &own(&GO_LIVE, false)?,
            &separator()?,
            &own(&PREVIOUS_CHANNEL, false)?,
            &own(&NEXT_CHANNEL, false)?,
            &own(&LAST_CHANNEL, false)?,
            &separator()?,
            &own(&MUTE, false)?,
            &separator()?,
            &own(&MULTIVIEW, true)?,
            &own(&INSET, false)?,
            &own(&SWAP_INSET, false)?,
            &own(&POP_OUT, false)?,
        ],
    )?;

    #[cfg(target_os = "macos")]
    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &separator()?,
            &PredefinedMenuItem::bring_all_to_front(app, None)?,
        ],
    )?;
    #[cfg(not(target_os = "macos"))]
    let window_menu = Submenu::with_items(
        app,
        "&Window",
        true,
        &[&PredefinedMenuItem::minimize(app, None)?, &PredefinedMenuItem::maximize(app, None)?, &PredefinedMenuItem::close_window(app, None)?],
    )?;

    let documentation = MenuItem::with_id(app, HELP_DOCUMENTATION, "Xorvio Documentation", true, None::<&str>)?;
    #[cfg(target_os = "macos")]
    let help_menu = Submenu::with_items(app, "Help", true, &[&documentation])?;
    #[cfg(not(target_os = "macos"))]
    let help_menu = Submenu::with_items(app, "&Help", true, &[&documentation, &separator()?, &PredefinedMenuItem::about(app, None, Some(about))?])?;

    #[cfg(target_os = "macos")]
    {
        // The Window menu lists the open windows, and Help gets the search field.
        window_menu.set_as_windows_menu_for_nsapp()?;
        help_menu.set_as_help_menu_for_nsapp()?;
    }

    let menu = Menu::with_items(
        app,
        &[
            #[cfg(target_os = "macos")]
            &app_menu,
            &file_menu,
            &edit_menu,
            &view_menu,
            &go_menu,
            &playback_menu,
            &window_menu,
            &help_menu,
        ],
    )?;
    app.manage(MenuItems(items));
    Ok(menu)
}

/// An item was chosen: Xorvio's own go to the window that acts on them.
pub fn handle(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref();
    match id {
        HELP_DOCUMENTATION => {
            if let Err(error) = app.opener().open_url(DOCUMENTATION_URL, None::<&str>) {
                tracing::warn!("the documentation couldn't be opened: {error}");
            }
        }
        #[cfg(not(target_os = "macos"))]
        VIEW_FULL_SCREEN => {
            if let Some(window) = focused_window(app) {
                let full = window.is_fullscreen().unwrap_or(false);
                let _ = window.set_fullscreen(!full);
            }
        }
        _ if id.starts_with("playback.") && !MULTIVIEW_ITEMS.contains(&id) => {
            let _ = app.emit_to(playback_window(app), MENU_EVENT, id);
        }
        // Multiview is in the main window whatever's popped out.
        _ if MULTIVIEW_ITEMS.contains(&id) || ["app.", "file.", "view.", "go."].iter().any(|prefix| id.starts_with(prefix)) => {
            // Settings, sources and going somewhere are the main window's,
            // which shows again if it was closed to the menu bar.
            crate::show_main_window(app);
            let _ = app.emit_to("main", MENU_EVENT, id);
        }
        // The tray's own items, handled with the tray.
        _ => {}
    }
}

/// Where Playback's items go: the pop-out player while it's open (the
/// video is there), otherwise the main window.
fn playback_window(app: &AppHandle) -> &'static str {
    if app.get_webview_window(crate::popout::LABEL).is_some() { crate::popout::LABEL } else { "main" }
}

#[cfg(not(target_os = "macos"))]
fn focused_window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    app.webview_windows().into_values().find(|window| window.is_focused().unwrap_or(false))
}

/// The inset item for what's on: adding one to what's playing, or showing
/// Multiview's two as one or side by side.
fn inset_item(state: &AppMenuState) -> (bool, &'static str) {
    if state.inset {
        (true, "Show Side by Side")
    } else if state.multiview {
        (state.can_inset, "Show as Inset")
    } else {
        (state.can_add_inset, "Add an Inset…")
    }
}

/// Enables and retitles items for what can act now, as the interface says.
#[tauri::command]
pub fn set_menu_state(app: AppHandle, window: Window, state: MenuState) {
    let Some(items) = app.try_state::<MenuItems>() else { return };
    let set = |id: &str, enabled: bool, title: Option<&str>| {
        if let Some(item) = items.0.get(id) {
            let _ = item.set_enabled(enabled);
            if let Some(title) = title {
                let _ = item.set_text(title);
            }
        }
    };
    if let Some(app_state) = state.app.filter(|_| window.label() == "main") {
        set(REFRESH_ALL.id, app_state.has_sources, None);
        set(BACK.id, app_state.can_go_back, None);
        set(SIDEBAR.id, true, Some(if app_state.sidebar_hidden { "Show Sidebar" } else { "Hide Sidebar" }));
        set(MULTIVIEW.id, true, Some(if app_state.multiview { "Close Multiview" } else { "Multiview" }));
        let (inset_ok, inset_title) = inset_item(&app_state);
        set(INSET.id, inset_ok, Some(inset_title));
        set(SWAP_INSET.id, app_state.inset, None);
    }
    // Only the window the video is in says what Playback can do.
    if let Some(playback) = state.playback.filter(|_| window.label() == playback_window(&app)) {
        let playing = playback.playing;
        // In Multiview the tile heard plays, pauses, skips and goes live.
        let active = playing || playback.multiview;
        set(PLAY_PAUSE.id, active, Some(if active && !playback.paused { "Pause" } else { "Play" }));
        set(PAUSE_ALL.id, playback.multiview, Some(if playback.all_paused { "Play All Tiles" } else { "Pause All Tiles" }));
        set(STOP.id, playing, None);
        set(SKIP_BACK.id, active && playback.seekable, None);
        set(SKIP_FORWARD.id, active && playback.seekable, None);
        set(GO_LIVE.id, active && playback.behind_live, None);
        set(PREVIOUS_CHANNEL.id, playback.channel || playback.multiview, None);
        set(NEXT_CHANNEL.id, playback.channel || playback.multiview, None);
        set(LAST_CHANNEL.id, playback.last_channel, None);
        set(MUTE.id, playing, Some(if playback.muted { "Unmute" } else { "Mute" }));
        set(POP_OUT.id, playing, Some(if playback.popped_out { "Return Video to Main Window" } else { "Pop Out Video" }));
    }
}

#[cfg(test)]
mod tests {
    use super::{inset_item, AppMenuState};

    #[test]
    fn the_inset_item_follows_whats_on() {
        let state = |multiview, can_add_inset, can_inset, inset| AppMenuState { multiview, can_add_inset, can_inset, inset, ..AppMenuState::default() };
        assert_eq!(inset_item(&state(false, true, false, false)), (true, "Add an Inset…"));
        assert_eq!(inset_item(&state(false, false, false, false)), (false, "Add an Inset…"), "nothing playing");
        assert_eq!(inset_item(&state(true, false, true, false)), (true, "Show as Inset"));
        assert_eq!(inset_item(&state(true, false, false, false)), (false, "Show as Inset"), "three or four tiles");
        assert_eq!(inset_item(&state(true, false, true, true)), (true, "Show Side by Side"));
    }
}
