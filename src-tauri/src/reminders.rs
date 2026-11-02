//! Reminder notifications for programmes on the watchlist, and notifications
//! of new episodes of followed series. A loop in the app (not the window)
//! checks every half minute, so reminders still go out while Xorvio is only
//! in the menu bar. The library remembers which went out, so a relaunch never
//! repeats one. Clicking one brings the window forward on the programme's
//! channel, or on the series (its Play button plays the first new episode).
//!
//! Settings › Reminders shows whether macOS lets Xorvio notify, sends a test
//! reminder, and opens Xorvio's page in System Settings.

use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::{Action, ActionType, NotificationExt, PermissionState};
use tauri_plugin_opener::OpenerExt;

use crate::{AppState, show_main_window};

/// Often enough that "5 minutes before" goes out at 4½ minutes at the latest.
const CHECK_EVERY: Duration = Duration::from_secs(30);

/// How often new episodes are looked for: the check itself runs when six
/// hours have passed since the last (xorvio-core keeps the time).
const NEW_EPISODES_EVERY: Duration = Duration::from_secs(30 * 60);

/// Where a reminder carries its channel, for the click.
const CHANNEL_KEY: &str = "channelId";

/// Where a new-episode notification carries its series and first new episode.
const SERIES_KEY: &str = "seriesId";
const EPISODE_KEY: &str = "episodeId";

/// New-episode notifications have a Play button.
const NEW_EPISODE_ACTIONS: &str = "new-episode";
const PLAY_ACTION: &str = "play";
const PLAY_TITLE: &str = "Play";

/// Clicking a reminder shows the window and asks the interface to tune its
/// channel (`play-channel`); clicking a new-episode notification opens the
/// series (`open-series`), and its Play button plays the episode
/// (`play-episode`). Set up before the first notification: the plugin only
/// reports clicks on notifications shown after a handler exists.
pub fn handle_clicks(app: &AppHandle) {
    let notifications = app.notification();
    let play = Action::builder(PLAY_ACTION, PLAY_TITLE).foreground(true).build();
    let _ = notifications.register_action_types(vec![ActionType::builder(NEW_EPISODE_ACTIONS).actions(vec![play]).build()]);
    let events = app.clone();
    let _ = notifications.on_action(move |performed| {
        let action = performed.action_id();
        let extra = performed.notification().map(|shown| shown.extra());
        let text = |key: &str| extra.and_then(|extra| extra.get(key)).and_then(|value| value.as_str()).map(str::to_string);
        if let Some(series_id) = text(SERIES_KEY) {
            // macOS names an action by its title.
            let playing = action == PLAY_ACTION || action == PLAY_TITLE;
            if !playing && action != "tap" {
                return;
            }
            show_main_window(&events);
            let _ = match text(EPISODE_KEY).filter(|_| playing) {
                Some(episode_id) => events.emit("play-episode", json!({ "seriesId": series_id, "episodeId": episode_id })),
                None => events.emit("open-series", series_id),
            };
            return;
        }
        if action != "tap" {
            return;
        }
        show_main_window(&events);
        if let Some(channel_id) = text(CHANNEL_KEY) {
            let _ = events.emit("play-channel", channel_id);
        }
    });
}

/// Sends reminders as they fall due, from launch on.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut asked = false;
        loop {
            send_due(&app, &mut asked);
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn send_due(app: &AppHandle, asked: &mut bool) {
    let state = app.state::<AppState>();
    let lead = state.settings.lock().unwrap().reminder_minutes.clamp(0, 60) * 60;
    let now = unix_now();
    let due = match state.core.due_reminders(now, lead) {
        Ok(due) if !due.is_empty() => due,
        Ok(_) => return,
        Err(e) => {
            tracing::warn!("reminders couldn't be checked: {e}");
            return;
        }
    };
    // Not allowed: nothing is marked sent, and each reminder lapses a couple
    // of minutes after its programme starts.
    if !allowed(app, asked) {
        return;
    }
    for reminder in &due {
        let mut notification = app.notification().builder().title(&reminder.title).body(&reminder.body);
        if let Some(channel_id) = &reminder.channel_id {
            notification = notification.extra(CHANNEL_KEY, channel_id);
        }
        if let Err(e) = notification.show() {
            tracing::warn!("a reminder couldn't be shown: {e}");
        }
    }
    let sent: Vec<String> = due.into_iter().map(|reminder| reminder.item_id).collect();
    if let Err(e) = state.core.mark_reminders_sent(&sent, now) {
        tracing::warn!("sent reminders couldn't be noted: {e}");
    }
}

/// Whether notifications may be shown, asking (once a launch) the first time
/// one is needed rather than at launch. On desktop the plugin reports them
/// allowed without asking, and the system's own notification settings decide.
fn allowed(app: &AppHandle, asked: &mut bool) -> bool {
    let notifications = app.notification();
    match notifications.permission_state() {
        Ok(PermissionState::Granted) => true,
        Ok(PermissionState::Prompt | PermissionState::PromptWithRationale) if !*asked => {
            *asked = true;
            matches!(notifications.request_permission(), Ok(PermissionState::Granted))
        }
        _ => false,
    }
}

// MARK: New episodes

/// Looks for new episodes of followed series a little after launch (once the
/// first refreshes are under way) and then whenever six hours have passed,
/// as Xorvio for Mac does. Badges are always kept; notifications go out
/// while "Tell me when new episodes arrive" is on.
pub fn spawn_new_episode_checks(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            check_new_episodes(&app).await;
            tokio::time::sleep(NEW_EPISODES_EVERY).await;
        }
    });
}

async fn check_new_episodes(app: &AppHandle) {
    let core = app.state::<AppState>().core.clone();
    let now = unix_now();
    if !core.new_episodes_due(now).unwrap_or(false) {
        return;
    }
    let found = match core.check_new_episodes(now).await {
        Ok(found) if !found.is_empty() => found,
        Ok(_) => return,
        Err(e) => {
            tracing::warn!("new episodes couldn't be looked for: {e}");
            return;
        }
    };
    // Badges and Home's New Episodes row.
    let _ = app.emit("library-changed", ());
    let alerts = app.state::<AppState>().settings.lock().unwrap().new_episode_alerts;
    if !alerts || permission() == NotificationPermission::Denied {
        return;
    }
    for new in found {
        let shown = app
            .notification()
            .builder()
            .title(&new.title)
            .body(&new.body)
            .action_type_id(NEW_EPISODE_ACTIONS)
            .extra(SERIES_KEY, &new.series_id)
            .extra(EPISODE_KEY, &new.episode_id)
            .show();
        if let Err(e) = shown {
            tracing::warn!("a new-episode notification couldn't be shown: {e}");
        }
    }
}

// MARK: Settings › Reminders

/// Whether Xorvio may show notifications, as the system says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(target_os = "macos"), allow(dead_code, reason = "only macOS reports Allowed and NotDetermined"))]
pub enum NotificationPermission {
    Allowed,
    /// Turned off in the system's settings.
    Denied,
    /// Not asked yet: the system asks the first time.
    NotDetermined,
    /// This system doesn't say (or a development build, which isn't an app bundle).
    Unknown,
}

#[cfg(target_os = "macos")]
use macos::{ask_if_never_asked, permission};

#[cfg(not(target_os = "macos"))]
fn permission() -> NotificationPermission {
    NotificationPermission::Unknown
}

#[cfg(not(target_os = "macos"))]
fn ask_if_never_asked() {}

#[tauri::command]
pub async fn notification_permission() -> Result<NotificationPermission, String> {
    Ok(tauri::async_runtime::spawn_blocking(permission).await.unwrap_or(NotificationPermission::Unknown))
}

/// A sample reminder a few seconds from now, so people can see what one
/// looks like (and so macOS asks, if it never has).
#[tauri::command]
pub async fn send_test_reminder(app: AppHandle) -> Result<(), String> {
    let status = tauri::async_runtime::spawn_blocking(|| {
        ask_if_never_asked();
        permission()
    })
    .await
    .unwrap_or(NotificationPermission::Unknown);
    if status == NotificationPermission::Denied {
        return Err("Notifications are off for Xorvio. Turn them on in System Settings › Notifications.".into());
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let shown = app
            .notification()
            .builder()
            .title("Your game is about to start")
            .body("This is how Xorvio reminds you about programmes on your watchlist.")
            .show();
        if let Err(e) = shown {
            tracing::warn!("the test reminder couldn't be shown: {e}");
        }
    });
    Ok(())
}

/// Opens Xorvio's notification settings in the system's settings.
#[tauri::command]
pub async fn open_notification_settings(app: AppHandle) -> Result<(), String> {
    let link = if cfg!(target_os = "macos") {
        format!("x-apple.systempreferences:com.apple.Notifications-Settings.extension?id={}", app.config().identifier)
    } else if cfg!(target_os = "windows") {
        "ms-settings:notifications".to_string()
    } else {
        return Err("Open your system's settings to choose how Xorvio's notifications appear.".into());
    };
    app.opener().open_url(link, None::<&str>).map_err(|e| format!("The notification settings couldn't be opened. {e}"))
}

#[cfg(target_os = "macos")]
mod macos {
    //! The notification center's view of Xorvio. Notifications themselves go
    //! out through the plugin; this only reads (and once asks for) the
    //! permission, which is the same switch in System Settings.

    use std::ptr::NonNull;
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSBundle, NSError};
    use objc2_user_notifications::{UNAuthorizationOptions, UNAuthorizationStatus, UNNotificationSettings, UNUserNotificationCenter};

    use super::NotificationPermission;

    /// The notification center works only inside an app bundle; a
    /// development build run from a terminal has none, and asking would stop it.
    fn center() -> Option<Retained<UNUserNotificationCenter>> {
        NSBundle::mainBundle().bundleIdentifier()?;
        Some(UNUserNotificationCenter::currentNotificationCenter())
    }

    pub fn permission() -> NotificationPermission {
        let Some(center) = center() else { return NotificationPermission::Unknown };
        let (sender, answer) = mpsc::channel();
        let handler = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
            // The center hands over settings that live for the call.
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            let _ = sender.send(status);
        });
        center.getNotificationSettingsWithCompletionHandler(&handler);
        match answer.recv_timeout(Duration::from_secs(3)) {
            Ok(UNAuthorizationStatus::Denied) => NotificationPermission::Denied,
            Ok(UNAuthorizationStatus::NotDetermined) => NotificationPermission::NotDetermined,
            Ok(_) => NotificationPermission::Allowed,
            Err(_) => NotificationPermission::Unknown,
        }
    }

    /// Asks once, with macOS's own prompt; afterwards the answer lives in System Settings.
    pub fn ask_if_never_asked() {
        if permission() != NotificationPermission::NotDetermined {
            return;
        }
        let Some(center) = center() else { return };
        let (sender, answer) = mpsc::channel();
        let handler = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
            let _ = sender.send(granted.as_bool());
        });
        center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &handler);
        // Waits for the person to answer, but not forever.
        let _ = answer.recv_timeout(Duration::from_secs(120));
    }
}
