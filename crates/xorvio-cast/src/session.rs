//! Playing on a receiver with Google's Default Media Receiver: open it (or
//! join it when it's already showing), load an address with its metadata,
//! then play, pause, seek, change the volume and stop. What the receiver
//! reports (the player's state, where it is, the volume) is kept in a
//! [`SessionState`] that callers watch.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::CastError;
use crate::connection::{Connection, Incoming, NS_CONNECTION, PLATFORM};

/// Google's Default Media Receiver, which plays HLS, MP4 and WebM.
pub const DEFAULT_MEDIA_RECEIVER: &str = "CC1AD845";

pub(crate) const NS_RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
pub(crate) const NS_MEDIA: &str = "urn:x-cast:com.google.cast.media";

const ANSWER_WAIT: Duration = Duration::from_secs(10);
/// Stopping doesn't keep anyone waiting on a TV that has gone quiet.
const CLOSE_WAIT: Duration = Duration::from_secs(4);
/// A TV that was asleep can take a while to open the receiver.
const LAUNCH_WAIT: Duration = Duration::from_secs(25);
/// LOAD is answered once the receiver has the media's start, which for a
/// stream ffmpeg is starting can take a while.
const LOAD_WAIT: Duration = Duration::from_secs(45);
/// Where the media is, asked this often: the receiver only says so by
/// itself when something changes.
const POLL_EVERY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayerState {
    /// Nothing loaded, or it finished, was stopped or failed (see the reason).
    #[default]
    Idle,
    /// Loading or waiting for more of the stream.
    Buffering,
    Playing,
    Paused,
}

/// Why the receiver's player went idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleReason {
    Finished,
    /// Someone stopped it (the TV's remote, another phone).
    Cancelled,
    /// Something else was loaded in its place.
    Interrupted,
    Error,
}

/// What the receiver last said about itself and its media.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionState {
    /// False once the connection has gone, or the receiver app was closed
    /// (another app opened on the TV, or someone stopped casting there).
    pub connected: bool,
    pub player: PlayerState,
    pub idle_reason: Option<IdleReason>,
    /// The receiver's position in the media, in seconds, when reported...
    pub position: f64,
    /// ...which was then; it moves on by itself while playing.
    pub reported_at: Instant,
    /// The media's length, when the receiver knows it.
    pub duration: Option<f64>,
    /// The receiver's volume, 0 to 1.
    pub volume: f64,
    pub muted: bool,
}

impl Default for SessionState {
    fn default() -> SessionState {
        SessionState {
            connected: true,
            player: PlayerState::Idle,
            idle_reason: None,
            position: 0.0,
            reported_at: Instant::now(),
            duration: None,
            volume: 1.0,
            muted: false,
        }
    }
}

impl SessionState {
    /// Where the media is now: the reported position, moved on by the time
    /// since while it plays.
    pub fn position_now(&self) -> f64 {
        let moved = if self.player == PlayerState::Playing { self.reported_at.elapsed().as_secs_f64() } else { 0.0 };
        let position = self.position + moved;
        match self.duration {
            Some(duration) if duration > 0.0 => position.min(duration),
            _ => position,
        }
    }
}

/// What to load, and what the receiver shows about it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Media {
    /// An address the receiver can reach.
    pub url: String,
    pub content_type: String,
    /// A live stream (no end, no length).
    pub live: bool,
    pub duration: Option<f64>,
    /// HLS whose segments are fragmented MP4, which the receiver has to be
    /// told (it assumes MPEG-TS).
    pub hls_fmp4: bool,
    pub title: String,
    pub subtitle: Option<String>,
    /// Artwork the receiver can fetch by itself.
    pub image: Option<String>,
}

impl Media {
    /// The `media` of a LOAD request (a MediaInformation).
    pub fn to_json(&self) -> Value {
        let mut metadata = json!({ "metadataType": 0, "title": self.title });
        if let Some(subtitle) = self.subtitle.as_deref().filter(|s| !s.is_empty()) {
            metadata["subtitle"] = subtitle.into();
        }
        if let Some(image) = &self.image {
            metadata["images"] = json!([{ "url": image }]);
        }
        let mut media = json!({
            "contentId": self.url,
            "contentUrl": self.url,
            "contentType": self.content_type,
            "streamType": if self.live { "LIVE" } else { "BUFFERED" },
            "metadata": metadata,
        });
        if let Some(duration) = self.duration.filter(|d| d.is_finite() && *d > 0.0 && !self.live) {
            media["duration"] = duration.into();
        }
        if self.hls_fmp4 {
            media["hlsSegmentFormat"] = "fmp4".into();
            media["hlsVideoSegmentFormat"] = "fmp4".into();
        }
        media
    }
}

/// A connection to one receiver, playing in its Default Media Receiver.
/// Cheap to clone; every clone is the same session.
#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

struct Inner {
    connection: Connection,
    /// The receiver app's own end, which media messages go to.
    transport: String,
    app_session: String,
    /// The receiver's id for what's loaded, which media commands name. The
    /// listener notes it from every status.
    media_session: Arc<Mutex<Option<i64>>>,
    state: watch::Sender<SessionState>,
    tasks: Vec<tokio::task::AbortHandle>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// An app running on the receiver, from a RECEIVER_STATUS.
#[derive(Debug, Clone, PartialEq)]
struct App {
    app_id: String,
    session_id: String,
    transport_id: String,
}

fn apps(status: &Value) -> Vec<App> {
    status["status"]["applications"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|app| {
                    Some(App {
                        app_id: app["appId"].as_str()?.to_string(),
                        session_id: app["sessionId"].as_str()?.to_string(),
                        transport_id: app["transportId"].as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Session {
    /// Connects to the receiver at `address` and opens the Default Media
    /// Receiver on it, or joins it when it's already open.
    pub async fn open(address: SocketAddr) -> Result<Session, CastError> {
        let (connection, mut incoming) = Connection::open(address).await?;
        connection.send(NS_CONNECTION, PLATFORM, connect())?;
        let status = connection.request(NS_RECEIVER, PLATFORM, json!({ "type": "GET_STATUS" }), ANSWER_WAIT).await?;
        let app = match apps(&status).into_iter().find(|app| app.app_id == DEFAULT_MEDIA_RECEIVER) {
            Some(app) => app,
            None => launch(&connection, &mut incoming).await?,
        };
        connection.send(NS_CONNECTION, &app.transport_id, connect())?;
        let mut state = SessionState::default();
        read_volume(&status, &mut state);
        let (state, _) = watch::channel(state);
        let media_session = Arc::new(Mutex::new(None));
        let listening = tokio::spawn(listen(incoming, state.clone(), app.clone(), media_session.clone(), connection.closed()));
        let inner = Arc::new_cyclic(|weak: &std::sync::Weak<Inner>| {
            // The first poll comes after a while, by when the session exists.
            let polling = tokio::spawn(poll(weak.clone()));
            Inner {
                connection,
                transport: app.transport_id,
                app_session: app.session_id,
                media_session,
                state,
                tasks: vec![listening.abort_handle(), polling.abort_handle()],
            }
        });
        Ok(Session { inner })
    }

    /// What the receiver last reported, to watch for changes.
    pub fn state(&self) -> watch::Receiver<SessionState> {
        self.inner.state.subscribe()
    }

    pub fn is_connected(&self) -> bool {
        self.inner.state.borrow().connected && !self.inner.connection.is_closed()
    }

    /// Loads `media`, starting `start` seconds in, and plays it. Returns
    /// once the receiver has started it, or with why it couldn't.
    pub async fn load(&self, media: &Media, start: f64) -> Result<(), CastError> {
        let mut request = json!({
            "type": "LOAD",
            "sessionId": self.inner.app_session,
            "media": media.to_json(),
            "autoplay": true,
        });
        if start > 0.0 && start.is_finite() && !media.live {
            request["currentTime"] = start.into();
        }
        let reply = self.inner.connection.request(NS_MEDIA, &self.inner.transport, request, LOAD_WAIT).await?;
        match reply["type"].as_str() {
            Some("MEDIA_STATUS") => {
                self.inner.note_media_session(&reply);
                Ok(())
            }
            Some("LOAD_FAILED" | "LOAD_CANCELLED") => Err(CastError::LoadFailed {
                reason: reply["detailedErrorCode"].as_i64().map(|code| format!("error {code}")).or_else(|| reply["reason"].as_str().map(String::from)),
            }),
            _ => Err(refused(&reply)),
        }
    }

    pub async fn play(&self) -> Result<(), CastError> {
        self.media_command(json!({ "type": "PLAY" })).await
    }

    pub async fn pause(&self) -> Result<(), CastError> {
        self.media_command(json!({ "type": "PAUSE" })).await
    }

    /// Moves to `seconds` into the media, keeping it playing or paused.
    pub async fn seek(&self, seconds: f64) -> Result<(), CastError> {
        self.media_command(json!({ "type": "SEEK", "currentTime": seconds.max(0.0) })).await
    }

    /// Stops the media, leaving the receiver open (to load something else).
    pub async fn stop_media(&self) -> Result<(), CastError> {
        if self.inner.media_session().is_none() {
            return Ok(());
        }
        self.media_command(json!({ "type": "STOP" })).await
    }

    /// The receiver's volume, 0 to 1.
    pub async fn set_volume(&self, level: f64) -> Result<(), CastError> {
        let level = if level.is_finite() { level.clamp(0.0, 1.0) } else { 1.0 };
        self.receiver_command(json!({ "type": "SET_VOLUME", "volume": { "level": level } })).await
    }

    pub async fn set_muted(&self, muted: bool) -> Result<(), CastError> {
        self.receiver_command(json!({ "type": "SET_VOLUME", "volume": { "muted": muted } })).await
    }

    /// Stops playing and closes the receiver app, so the TV goes back to
    /// what it shows when idle, then disconnects. Problems on the way are
    /// ignored: the connection is going anyway.
    pub async fn close(&self) {
        let inner = &self.inner;
        if !inner.connection.is_closed() {
            let _ = tokio::time::timeout(CLOSE_WAIT, self.stop_media()).await;
            let stop = json!({ "type": "STOP", "sessionId": inner.app_session });
            let _ = inner.connection.request(NS_RECEIVER, PLATFORM, stop, CLOSE_WAIT).await;
            let _ = inner.connection.send(NS_CONNECTION, &inner.transport, json!({ "type": "CLOSE" }));
            let _ = inner.connection.send(NS_CONNECTION, PLATFORM, json!({ "type": "CLOSE" }));
        }
        inner.state.send_modify(|state| state.connected = false);
        inner.connection.shut().await;
    }

    async fn media_command(&self, mut request: Value) -> Result<(), CastError> {
        let Some(id) = self.inner.media_session() else {
            return Err(CastError::Refused("nothing is playing".into()));
        };
        request["mediaSessionId"] = id.into();
        let reply = self.inner.connection.request(NS_MEDIA, &self.inner.transport, request, ANSWER_WAIT).await?;
        match reply["type"].as_str() {
            Some("MEDIA_STATUS") => {
                self.inner.note_media_session(&reply);
                Ok(())
            }
            _ => Err(refused(&reply)),
        }
    }

    async fn receiver_command(&self, request: Value) -> Result<(), CastError> {
        let reply = self.inner.connection.request(NS_RECEIVER, PLATFORM, request, ANSWER_WAIT).await?;
        match reply["type"].as_str() {
            Some("RECEIVER_STATUS") => Ok(()),
            _ => Err(refused(&reply)),
        }
    }
}

impl Inner {
    fn media_session(&self) -> Option<i64> {
        *self.media_session.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The media session id in a MEDIA_STATUS, which later commands name.
    fn note_media_session(&self, status: &Value) {
        if let Some(id) = status["status"].as_array().and_then(|list| list.first()).and_then(|s| s["mediaSessionId"].as_i64()) {
            *self.media_session.lock().unwrap_or_else(|e| e.into_inner()) = Some(id);
        }
    }
}

/// A virtual connection's CONNECT, saying who's connecting as other senders do.
fn connect() -> Value {
    json!({ "type": "CONNECT", "origin": {}, "userAgent": concat!("Xorvio/", env!("CARGO_PKG_VERSION")) })
}

fn refused(reply: &Value) -> CastError {
    let kind = reply["type"].as_str().unwrap_or("an unknown reply");
    let reason = reply["reason"].as_str().map(|r| format!("{kind}: {r}")).unwrap_or_else(|| kind.to_string());
    CastError::Refused(reason)
}

/// Opens the Default Media Receiver and waits until the receiver lists it.
async fn launch(connection: &Connection, incoming: &mut mpsc::UnboundedReceiver<Incoming>) -> Result<App, CastError> {
    let deadline = Instant::now() + LAUNCH_WAIT;
    let reply = connection.request(NS_RECEIVER, PLATFORM, json!({ "type": "LAUNCH", "appId": DEFAULT_MEDIA_RECEIVER }), LAUNCH_WAIT).await?;
    match reply["type"].as_str() {
        Some("RECEIVER_STATUS") => {}
        Some("LAUNCH_ERROR") => return Err(CastError::LaunchFailed(reply["reason"].as_str().unwrap_or("unknown").to_string())),
        _ => return Err(refused(&reply)),
    }
    let found = |status: &Value| apps(status).into_iter().find(|app| app.app_id == DEFAULT_MEDIA_RECEIVER);
    if let Some(app) = found(&reply) {
        return Ok(app);
    }
    // Some receivers answer before the app is up, then say so in a status of its own.
    loop {
        let message = tokio::time::timeout_at(deadline, incoming.recv()).await.map_err(|_| CastError::NoAnswer)?.ok_or(CastError::Closed)?;
        if message.namespace == NS_RECEIVER
            && let Some(app) = found(&message.json)
        {
            return Ok(app);
        }
    }
}

fn read_volume(status: &Value, state: &mut SessionState) {
    let volume = &status["status"]["volume"];
    if let Some(level) = volume["level"].as_f64() {
        state.volume = level.clamp(0.0, 1.0);
    }
    if let Some(muted) = volume["muted"].as_bool() {
        state.muted = muted;
    }
}

/// Keeps the state up to date from what the receiver sends, until the
/// connection ends or the app is closed.
async fn listen(
    mut incoming: mpsc::UnboundedReceiver<Incoming>,
    state: watch::Sender<SessionState>,
    app: App,
    media_session: Arc<Mutex<Option<i64>>>,
    mut closed: watch::Receiver<bool>,
) {
    // Statuses from before the app was opened (the reply to the first
    // GET_STATUS) come through here too, so the app only counts as closed
    // once a status has listed it and a later one doesn't.
    let mut seen_open = false;
    loop {
        let message = tokio::select! {
            message = incoming.recv() => message,
            _ = closed.wait_for(|closed| *closed) => None,
        };
        let Some(message) = message else { break };
        match (message.namespace.as_str(), message.json["type"].as_str()) {
            (NS_RECEIVER, Some("RECEIVER_STATUS")) => {
                state.send_modify(|state| read_volume(&message.json, state));
                // A status that leaves out the applications says nothing about them.
                let Some(running) = message.json["status"]["applications"].as_array() else { continue };
                let open = running.iter().any(|other| other["sessionId"].as_str() == Some(app.session_id.as_str()));
                if open {
                    seen_open = true;
                } else if seen_open {
                    tracing::info!("the receiver app was closed on the device");
                    break;
                }
            }
            (NS_MEDIA, Some("MEDIA_STATUS")) if message.source == app.transport_id => {
                let Some(status) = message.json["status"].as_array() else { continue };
                if let Some(id) = status.first().and_then(|s| s["mediaSessionId"].as_i64()) {
                    *media_session.lock().unwrap_or_else(|e| e.into_inner()) = Some(id);
                }
                state.send_modify(|state| read_media(status.first(), state));
            }
            (NS_CONNECTION, Some("CLOSE")) if message.source == app.transport_id => {
                tracing::info!("the receiver app closed its connection");
                break;
            }
            _ => {}
        }
    }
    state.send_modify(|state| state.connected = false);
}

/// One entry of a MEDIA_STATUS. An empty status means nothing is loaded.
fn read_media(status: Option<&Value>, state: &mut SessionState) {
    let Some(status) = status else {
        state.player = PlayerState::Idle;
        return;
    };
    if let Some(player) = status["playerState"].as_str() {
        state.player = match player {
            "PLAYING" => PlayerState::Playing,
            "PAUSED" => PlayerState::Paused,
            "BUFFERING" | "LOADING" => PlayerState::Buffering,
            _ => PlayerState::Idle,
        };
        state.idle_reason = if state.player == PlayerState::Idle {
            status["idleReason"].as_str().map(|reason| match reason {
                "FINISHED" => IdleReason::Finished,
                "CANCELLED" => IdleReason::Cancelled,
                "INTERRUPTED" => IdleReason::Interrupted,
                _ => IdleReason::Error,
            })
        } else {
            None
        };
    }
    if let Some(position) = status["currentTime"].as_f64().filter(|p| p.is_finite()) {
        state.position = position.max(0.0);
        state.reported_at = Instant::now();
    }
    if let Some(duration) = status["media"]["duration"].as_f64().filter(|d| d.is_finite() && *d > 0.0) {
        state.duration = Some(duration);
    }
}

/// Asks where the media is every few seconds while something's loaded.
async fn poll(inner: std::sync::Weak<Inner>) {
    let mut every = tokio::time::interval(POLL_EVERY);
    every.tick().await;
    loop {
        every.tick().await;
        let Some(inner) = inner.upgrade() else { break };
        if inner.connection.is_closed() || !inner.state.borrow().connected {
            break;
        }
        let Some(id) = inner.media_session() else { continue };
        let request = json!({ "type": "GET_STATUS", "mediaSessionId": id });
        // The reply comes through the listener like any status.
        let _ = inner.connection.request(NS_MEDIA, &inner.transport, request, ANSWER_WAIT).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_requests_carry_what_the_receiver_needs() {
        let media = Media {
            url: "http://192.168.1.20:50000/t/l/id/index.m3u8".into(),
            content_type: "application/x-mpegurl".into(),
            live: true,
            duration: Some(10.0),
            hls_fmp4: true,
            title: "Channel One".into(),
            subtitle: Some("The News".into()),
            image: Some("https://image.example/logo.png".into()),
        };
        let json = media.to_json();
        assert_eq!(json["contentId"], media.url);
        assert_eq!(json["contentUrl"], media.url);
        assert_eq!(json["streamType"], "LIVE");
        assert_eq!(json["hlsSegmentFormat"], "fmp4");
        assert_eq!(json["hlsVideoSegmentFormat"], "fmp4");
        assert!(json.get("duration").is_none(), "live streams have no length");
        assert_eq!(json["metadata"], json!({ "metadataType": 0, "title": "Channel One", "subtitle": "The News", "images": [{ "url": "https://image.example/logo.png" }] }));

        let film = Media { live: false, hls_fmp4: false, subtitle: None, image: None, content_type: "video/mp4".into(), duration: Some(5400.5), ..media };
        let json = film.to_json();
        assert_eq!(json["streamType"], "BUFFERED");
        assert_eq!(json["duration"], 5400.5);
        assert!(json.get("hlsSegmentFormat").is_none());
        assert_eq!(json["metadata"], json!({ "metadataType": 0, "title": "Channel One" }));
    }

    #[test]
    fn media_statuses_are_read() {
        let mut state = SessionState::default();
        read_media(Some(&json!({ "mediaSessionId": 1, "playerState": "PLAYING", "currentTime": 12.5, "media": { "duration": 60.0 } })), &mut state);
        assert_eq!((state.player, state.position, state.duration), (PlayerState::Playing, 12.5, Some(60.0)));
        read_media(Some(&json!({ "playerState": "IDLE", "idleReason": "FINISHED" })), &mut state);
        assert_eq!((state.player, state.idle_reason), (PlayerState::Idle, Some(IdleReason::Finished)));
        assert_eq!(state.position, 12.5, "a status without a time keeps the last one");
        read_media(Some(&json!({ "playerState": "BUFFERING" })), &mut state);
        assert_eq!((state.player, state.idle_reason), (PlayerState::Buffering, None));
        read_media(None, &mut state);
        assert_eq!(state.player, PlayerState::Idle);
    }

    #[test]
    fn the_position_moves_on_only_while_playing() {
        let mut state = SessionState { position: 10.0, reported_at: Instant::now() - Duration::from_secs(3), ..SessionState::default() };
        assert_eq!(state.position_now(), 10.0);
        state.player = PlayerState::Playing;
        assert!((state.position_now() - 13.0).abs() < 0.5);
        state.duration = Some(11.0);
        assert_eq!(state.position_now(), 11.0, "never past the end");
    }
}
