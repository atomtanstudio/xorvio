//! Casting to a Chromecast or Google TV, the way AirPlay is offered: the
//! devices found on the network (xorvio-cast), a stream the TV can fetch
//! from this computer (the gateway's listener on the network), and the
//! receiver's state passed to the interface as `cast-changed` events.
//!
//! The interface connects first (the TV wakes and opens Google's Default
//! Media Receiver), then lets its own stream go and loads the title on the
//! TV, so a provider never sees two connections. Stopping closes the
//! receiver, ends the listener and says where the TV was, for the player
//! to carry on from there.

use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use xorvio_cast::{CastError, Device, Discovery, IdleReason, Media, PlayerState, Session, SessionState};
use xorvio_media::CastMedia;

use crate::AppState;
use crate::commands::{PlayRequest, resolve};

type CmdResult<T> = Result<T, String>;

/// Devices, and the session with one of them.
#[derive(Default)]
pub struct Casting {
    discovery: Mutex<Option<Discovery>>,
    current: tokio::sync::Mutex<Option<Current>>,
}

struct Current {
    device: Device,
    session: Session,
    /// What the status events need, shared with the task sending them.
    info: Arc<Mutex<Info>>,
    watcher: tauri::async_runtime::JoinHandle<()>,
}

impl Drop for Current {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

#[derive(Clone, Default)]
struct Info {
    device_id: String,
    device_name: String,
    /// What's on the TV, once loaded.
    media: Option<CastMedia>,
    details: CastDetails,
    /// Someone here stopped casting: the end isn't news.
    stopping: bool,
}

/// What the TV shows about the title, from the interface.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CastDetails {
    pub title: String,
    pub subtitle: Option<String>,
    pub artwork_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CastState {
    /// Connected, with nothing loaded yet.
    Ready,
    Buffering,
    Playing,
    Paused,
    /// The title played to its end.
    Finished,
    /// Someone stopped it on the TV, something else was cast, or the TV went away.
    Stopped,
    /// The TV couldn't play it.
    Failed,
}

/// The receiver's state, in the title's own time.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CastStatus {
    pub device_id: String,
    pub device_name: String,
    pub state: CastState,
    /// Where the title is; it moves on by itself while playing.
    pub position_seconds: f64,
    pub duration_seconds: Option<f64>,
    pub live: bool,
    pub can_seek: bool,
    /// The TV's volume, 0 to 1.
    pub volume: f64,
    pub muted: bool,
    /// Why it stopped or failed, for people.
    pub message: Option<String>,
}

fn status_of(info: &Info, state: &SessionState) -> CastStatus {
    let name = &info.device_name;
    let media = info.media.as_ref();
    let offset = media.map_or(0.0, |m| m.offset_seconds);
    let (state_now, message) = match (state.connected, state.player, state.idle_reason) {
        (false, ..) => (CastState::Stopped, Some(format!("{name} stopped playing."))),
        (true, PlayerState::Playing, _) => (CastState::Playing, None),
        (true, PlayerState::Paused, _) => (CastState::Paused, None),
        (true, PlayerState::Buffering, _) => (CastState::Buffering, None),
        (true, PlayerState::Idle, Some(IdleReason::Finished)) => (CastState::Finished, None),
        (true, PlayerState::Idle, Some(IdleReason::Error)) => (CastState::Failed, Some(format!("{name} couldn't play this."))),
        (true, PlayerState::Idle, Some(IdleReason::Cancelled | IdleReason::Interrupted)) => (CastState::Stopped, Some(format!("{name} stopped playing."))),
        (true, PlayerState::Idle, None) => (if media.is_some() { CastState::Buffering } else { CastState::Ready }, None),
    };
    let live = media.is_some_and(|m| m.live);
    CastStatus {
        device_id: info.device_id.clone(),
        device_name: name.clone(),
        state: state_now,
        position_seconds: if live { 0.0 } else { offset + state.position_now() },
        duration_seconds: media.and_then(|m| m.duration_seconds).map(|d| d + offset),
        live,
        can_seek: media.is_some_and(|m| !m.live),
        volume: state.volume,
        muted: state.muted,
        message,
    }
}

/// Artwork the TV can fetch by itself, and that says nothing it shouldn't:
/// web addresses only (not the app's own schemes, which need a media
/// server's token), with no login in them.
fn tv_artwork(url: Option<&str>) -> Option<String> {
    let parsed = url::Url::parse(url?.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let secret = |key: &str| {
        let key = key.to_ascii_lowercase();
        ["user", "pass", "password", "token", "key", "auth", "secret", "sig"].iter().any(|word| key.contains(word))
    };
    if parsed.query_pairs().any(|(key, _)| secret(&key)) {
        return None;
    }
    Some(parsed.to_string())
}

/// What to say when the TV couldn't load what it was given.
fn load_failure(name: &str, reached_us: bool) -> String {
    if reached_us {
        return format!("{name} couldn't play this.");
    }
    if cfg!(target_os = "macos") {
        format!("{name} couldn't reach this Mac. If macOS asked whether Xorvio may accept incoming connections, allow it in System Settings › Network › Firewall.")
    } else if cfg!(windows) {
        format!("{name} couldn't reach this computer. Allow Xorvio through Windows Defender Firewall on private networks.")
    } else {
        format!("{name} couldn't reach this computer. A firewall may be blocking incoming connections to Xorvio.")
    }
}

fn connect_failure(name: &str, error: &CastError) -> String {
    match error {
        CastError::Unreachable(_) => format!("{name} couldn't be reached. Check it's on, and on the same network as this computer."),
        CastError::NoAnswer => format!("{name} didn't answer. Check it's on, and on the same network as this computer."),
        CastError::LaunchFailed(_) => format!("{name} couldn't open its player."),
        CastError::Closed => format!("{name} closed the connection."),
        other => format!("{name} refused: {other}."),
    }
}

/// Sends the session's state to the interface whenever it changes, and
/// tidies up when the TV ends the session by itself.
fn watch(app: AppHandle, session: &Session, info: Arc<Mutex<Info>>) -> tauri::async_runtime::JoinHandle<()> {
    let mut changes = session.state();
    tauri::async_runtime::spawn(async move {
        loop {
            let state = changes.borrow_and_update().clone();
            let info_now = info.lock().unwrap().clone();
            if info_now.stopping {
                break;
            }
            let _ = app.emit("cast-changed", status_of(&info_now, &state));
            if !state.connected {
                // The TV went away or was given something else: nothing to serve.
                if let Some(gateway) = app.try_state::<AppState>().map(|s| s.gateway.clone()) {
                    gateway.cast_end();
                }
                break;
            }
            if changes.changed().await.is_err() {
                break;
            }
        }
    })
}

/// The devices found so far, looking for them from now on (the first call
/// starts mDNS, and with it the system's question about the local network).
#[tauri::command]
pub async fn cast_devices(app: AppHandle, casting: State<'_, Casting>) -> CmdResult<Vec<Device>> {
    let mut discovery = casting.discovery.lock().unwrap();
    if discovery.is_none() {
        let started = Discovery::start().map_err(|e| e.to_string())?;
        let mut changes = started.changes();
        let events = app.clone();
        tauri::async_runtime::spawn(async move {
            while changes.changed().await.is_ok() {
                let list = changes.borrow_and_update().clone();
                let _ = events.emit("cast-devices", &list);
            }
        });
        *discovery = Some(started);
    }
    Ok(discovery.as_ref().map(Discovery::devices).unwrap_or_default())
}

/// Why what `request` names can't go to a TV, or nothing when it can.
#[tauri::command]
pub async fn cast_check(state: State<'_, AppState>, request: PlayRequest) -> CmdResult<Option<String>> {
    let (target, hint) = resolve(&state, request)?;
    Ok(state.gateway.cast_check(&target, &hint))
}

/// Connects to a device and opens its media player. Whatever another
/// device was playing stops.
#[tauri::command]
pub async fn cast_connect(app: AppHandle, state: State<'_, AppState>, casting: State<'_, Casting>, device_id: String) -> CmdResult<CastStatus> {
    let device = casting.discovery.lock().unwrap().as_ref().and_then(|d| d.device(&device_id)).ok_or("That TV isn't on the network any more.")?;
    let mut current = casting.current.lock().await;
    if let Some(open) = current.as_ref()
        && open.device.id == device.id
        && open.session.is_connected()
    {
        let info = open.info.lock().unwrap().clone();
        return Ok(status_of(&info, &open.session.state().borrow()));
    }
    if let Some(old) = current.take() {
        old.info.lock().unwrap().stopping = true;
        old.session.close().await;
        state.gateway.cast_end();
    }
    let address = device.address().ok_or("Xorvio doesn't know where that TV is on the network.")?;
    tracing::info!(name = %device.name, model = ?device.model, "connecting to a TV");
    let session = Session::open(address).await.map_err(|error| {
        tracing::warn!(name = %device.name, "couldn't connect to a TV: {error}");
        connect_failure(&device.name, &error)
    })?;
    let info = Arc::new(Mutex::new(Info { device_id: device.id.clone(), device_name: device.name.clone(), ..Info::default() }));
    let status = status_of(&info.lock().unwrap(), &session.state().borrow());
    let watcher = watch(app, &session, info.clone());
    *current = Some(Current { device, session, info, watcher });
    Ok(status)
}

/// Plays what `request` names on the connected TV, from `position_seconds`
/// for a film or an episode. The interface lets its own stream go first.
#[tauri::command]
pub async fn cast_load(state: State<'_, AppState>, casting: State<'_, Casting>, request: PlayRequest, details: CastDetails, position_seconds: Option<f64>) -> CmdResult<CastStatus> {
    let current = casting.current.lock().await;
    let current = current.as_ref().filter(|c| c.session.is_connected()).ok_or("The TV isn't connected any more.")?;
    let name = current.device.name.clone();
    let (target, hint) = resolve(&state, request)?;
    let tv: Vec<IpAddr> = current.device.addresses.clone();
    let media = state.gateway.cast(target, hint, position_seconds.unwrap_or(0.0), &tv).await.map_err(|e| e.to_string())?;
    let before = state.gateway.cast_requests();
    if let Err(error) = load(current, &media, &details).await {
        // Whether the TV asked for the stream at all tells a firewall from a format.
        let reached_us = state.gateway.cast_requests() > before;
        state.gateway.cast_end();
        return Err(match error {
            CastError::LoadFailed { .. } => load_failure(&name, reached_us),
            CastError::NoAnswer => format!("{name} took too long to start playing."),
            other => format!("{name} couldn't play this ({other})."),
        });
    }
    let mut info = current.info.lock().unwrap();
    info.media = Some(media);
    info.details = details;
    Ok(status_of(&info, &current.session.state().borrow()))
}

async fn load(current: &Current, media: &CastMedia, details: &CastDetails) -> Result<(), CastError> {
    let tv = Media {
        url: media.url.clone(),
        content_type: media.content_type.clone(),
        live: media.live,
        duration: media.duration_seconds,
        hls_fmp4: media.hls_fmp4,
        title: details.title.clone(),
        subtitle: details.subtitle.clone(),
        image: tv_artwork(details.artwork_url.as_deref()),
    };
    let result = current.session.load(&tv, media.start_seconds).await;
    if let Err(error) = &result {
        tracing::warn!("the TV couldn't load the stream: {error}");
    }
    result
}

fn session_error(error: CastError) -> String {
    match error {
        CastError::Closed => "The TV isn't connected any more.".to_string(),
        CastError::NoAnswer => "The TV didn't answer.".to_string(),
        other => format!("The TV refused ({other})."),
    }
}

async fn with_session<T>(casting: &Casting, act: impl AsyncFnOnce(&Current) -> Result<T, CastError>) -> CmdResult<T> {
    let current = casting.current.lock().await;
    let current = current.as_ref().ok_or("Nothing is playing on a TV.")?;
    act(current).await.map_err(session_error)
}

#[tauri::command]
pub async fn cast_play(casting: State<'_, Casting>) -> CmdResult<()> {
    with_session(&casting, async |c| c.session.play().await).await
}

#[tauri::command]
pub async fn cast_pause(casting: State<'_, Casting>) -> CmdResult<()> {
    with_session(&casting, async |c| c.session.pause().await).await
}

/// Moves a film or an episode on the TV to `position_seconds` in the title.
/// A stream ffmpeg makes starts again from there; the TV seeks in others.
#[tauri::command]
pub async fn cast_seek(state: State<'_, AppState>, casting: State<'_, Casting>, position_seconds: f64) -> CmdResult<()> {
    let current = casting.current.lock().await;
    let current = current.as_ref().ok_or("Nothing is playing on a TV.")?;
    let (media, details) = {
        let info = current.info.lock().unwrap();
        (info.media.clone().ok_or("Nothing is playing on a TV.")?, info.details.clone())
    };
    if media.live {
        return Err("A live channel can't be moved through on the TV.".into());
    }
    if !media.seeks_by_restart {
        return current.session.seek(position_seconds - media.offset_seconds).await.map_err(session_error);
    }
    let fresh = state.gateway.cast_seek(&media.url, position_seconds).await.map_err(|e| e.to_string())?;
    if let Err(error) = load(current, &fresh, &details).await {
        state.gateway.cast_end();
        return Err(session_error(error));
    }
    current.info.lock().unwrap().media = Some(fresh);
    Ok(())
}

/// The TV's volume, 0 to 1.
#[tauri::command]
pub async fn cast_volume(casting: State<'_, Casting>, level: f64) -> CmdResult<()> {
    with_session(&casting, async |c| c.session.set_volume(level).await).await
}

#[tauri::command]
pub async fn cast_mute(casting: State<'_, Casting>, muted: bool) -> CmdResult<()> {
    with_session(&casting, async |c| c.session.set_muted(muted).await).await
}

/// Stops casting: the TV's player closes and the listener on the network
/// with it. Says where the title was on the TV, for the player to carry on.
#[tauri::command]
pub async fn cast_stop(state: State<'_, AppState>, casting: State<'_, Casting>) -> CmdResult<Option<f64>> {
    let Some(current) = casting.current.lock().await.take() else {
        state.gateway.cast_end();
        return Ok(None);
    };
    let position = {
        let mut info = current.info.lock().unwrap();
        info.stopping = true;
        let status = status_of(&info, &current.session.state().borrow());
        (!status.live && status.state != CastState::Finished).then_some(status.position_seconds)
    };
    current.session.close().await;
    state.gateway.cast_end();
    tracing::info!("stopped casting to a TV");
    Ok(position)
}

/// Closes the TV's player when Xorvio quits, without waiting long for it.
pub fn stop_on_quit(app: &AppHandle) {
    let Some(casting) = app.try_state::<Casting>() else { return };
    tauri::async_runtime::block_on(async {
        // A command still talking to the TV holds the session; quitting doesn't wait for it.
        let current = casting.current.try_lock().ok().and_then(|mut current| current.take());
        if let Some(current) = current {
            current.info.lock().unwrap().stopping = true;
            let _ = tokio::time::timeout(Duration::from_secs(2), current.session.close()).await;
        }
    });
    if let Some(state) = app.try_state::<AppState>() {
        state.gateway.cast_end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_goes_only_when_the_tv_can_fetch_it_and_it_holds_no_login() {
        assert_eq!(tv_artwork(Some("https://image.tmdb.org/t/p/w780/poster.jpg")).as_deref(), Some("https://image.tmdb.org/t/p/w780/poster.jpg"));
        assert_eq!(tv_artwork(Some("http://logos.example/one.png")).as_deref(), Some("http://logos.example/one.png"));
        assert_eq!(tv_artwork(Some("xorvio-server://localhost/plex-1/library/metadata/5/thumb")), None);
        assert_eq!(tv_artwork(Some("http://user:pass@provider.example/logo.png")), None);
        assert_eq!(tv_artwork(Some("http://provider.example/image.php?username=me&password=x")), None);
        assert_eq!(tv_artwork(Some("http://server.example/thumb?X-Plex-Token=abc")), None);
        assert_eq!(tv_artwork(Some("http://cdn.example/logo.png?w=300")).as_deref(), Some("http://cdn.example/logo.png?w=300"));
        assert_eq!(tv_artwork(None), None);
    }

    fn info(media: Option<CastMedia>) -> Info {
        Info { device_id: "tv".into(), device_name: "Living Room TV".into(), media, ..Info::default() }
    }

    fn remuxed() -> CastMedia {
        CastMedia {
            url: "http://192.168.1.2:5000/t/m/id/video.mp4".into(),
            content_type: "video/mp4".into(),
            live: false,
            hls_fmp4: false,
            duration_seconds: Some(4000.0),
            offset_seconds: 1400.0,
            start_seconds: 0.0,
            seeks_by_restart: true,
            notes: vec![],
        }
    }

    #[test]
    fn statuses_are_in_the_titles_own_time() {
        let state = SessionState { player: PlayerState::Paused, position: 100.0, ..SessionState::default() };
        let status = status_of(&info(Some(remuxed())), &state);
        assert_eq!(status.state, CastState::Paused);
        assert_eq!(status.position_seconds, 1500.0, "the stream started 1400 s in");
        assert_eq!(status.duration_seconds, Some(5400.0));
        assert!(status.can_seek && !status.live);
        let ready = status_of(&info(None), &SessionState::default());
        assert_eq!(ready.state, CastState::Ready);
    }

    #[test]
    fn endings_say_what_happened() {
        let media = Some(remuxed());
        let idle = |reason| SessionState { player: PlayerState::Idle, idle_reason: Some(reason), ..SessionState::default() };
        assert_eq!(status_of(&info(media.clone()), &idle(IdleReason::Finished)).state, CastState::Finished);
        let failed = status_of(&info(media.clone()), &idle(IdleReason::Error));
        assert_eq!((failed.state, failed.message.as_deref()), (CastState::Failed, Some("Living Room TV couldn't play this.")));
        assert_eq!(status_of(&info(media.clone()), &idle(IdleReason::Interrupted)).state, CastState::Stopped);
        let gone = status_of(&info(media), &SessionState { connected: false, ..SessionState::default() });
        assert_eq!((gone.state, gone.message.as_deref()), (CastState::Stopped, Some("Living Room TV stopped playing.")));
    }

    #[test]
    fn a_tv_that_never_asked_is_probably_behind_a_firewall() {
        assert_eq!(load_failure("Living Room TV", true), "Living Room TV couldn't play this.");
        assert!(load_failure("Living Room TV", false).contains("couldn't reach this"));
    }
}
