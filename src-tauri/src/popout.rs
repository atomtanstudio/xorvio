//! The pop-out player: video in a window of its own, as the Mac app had,
//! docked back into the main window with a button. The pop-out is a second
//! web view running the interface in player-only mode, so what's playing is
//! handed across: the main window releases its stream and passes the item
//! and position here; the pop-out takes them when it starts (or receives
//! `popout-play` when it's already open), and docking passes them back to
//! the main window as `dock`. One stream at a time, so a provider that
//! allows one connection never sees two.

use std::sync::Mutex;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// The pop-out window's label (the interface checks it to start in player-only mode).
pub const LABEL: &str = "popout";

/// What the main window handed over, until the pop-out takes it.
#[derive(Default)]
pub struct Handoff(Mutex<Option<Value>>);

/// Plays `handoff` (the item and where it was) in the pop-out window,
/// opening it if it isn't open.
#[tauri::command]
pub async fn pop_out(app: AppHandle, handoff: Value, on_top: bool) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(LABEL) {
        app.emit_to(LABEL, "popout-play", &handoff).map_err(|e| e.to_string())?;
        let _ = window.set_focus();
        return Ok(());
    }
    *app.state::<Handoff>().0.lock().unwrap() = Some(handoff);
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut builder = WebviewWindowBuilder::new(&app, LABEL, WebviewUrl::App("index.html".into()))
        .title("Xorvio")
        .inner_size(960.0, 540.0)
        .min_inner_size(480.0, 270.0)
        .always_on_top(on_top)
        .background_color(tauri::window::Color(17, 20, 24, 255))
        .focused(true);
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            .traffic_light_position(tauri::LogicalPosition::new(19.0, 28.0));
    }
    let window = builder.build().map_err(|e| format!("The video couldn't open in its own window: {e}"))?;
    let events = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            let _ = events.emit_to("main", "popout-closed", ());
        }
    });
    Ok(())
}

/// What the main window handed over, for the pop-out when it starts.
#[tauri::command]
pub fn take_pop_out(app: AppHandle) -> Option<Value> {
    app.state::<Handoff>().0.lock().unwrap().take()
}

/// Back into the main window: it plays `handoff`, and the pop-out closes.
#[tauri::command]
pub async fn dock(app: AppHandle, handoff: Value) -> Result<(), String> {
    app.emit_to("main", "dock", &handoff).map_err(|e| e.to_string())?;
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.set_focus();
    }
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.destroy();
    }
    Ok(())
}

#[tauri::command]
pub fn set_pop_out_on_top(app: AppHandle, on: bool) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.set_always_on_top(on);
    }
}
