//! Streams for a TV (Chromecast, Google TV). The TV fetches what it plays
//! by itself, so it needs an address on the network, while the gateway
//! answers on 127.0.0.1 only. A second listener is opened for casting:
//!
//! - It's bound to this computer's address on the network the TV is on (no
//!   other interface), on a random port, only while something is cast, and
//!   closed when casting stops.
//! - It serves only the streams registered for the TV, never the player's
//!   or other players' ones: its addresses carry a token of its own and a
//!   random id per stream, and requests from any address but the TV's are
//!   refused. Nothing else is routed.
//! - Addresses never hold a provider's address or login: the real target
//!   stays in memory, as for the player.
//!
//! What each kind of stream becomes is in the plan module.

mod plan;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use anyhow::{anyhow, bail};
use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path as UrlPath, State};
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::http::{Method, Response, StatusCode};
use axum::routing::any;
use serde::Serialize;
use tokio::sync::oneshot;
use xorvio_model::{PlaybackMode, StreamTarget};

use self::plan::{CastPlan, NEEDS_FFMPEG, Route};
use super::{Entry, FFMPEG_MISSING, Gateway, PlayHint, PlaybackSupport, STREAM_GONE, Shared, clean_extension, cors, extension_of, lock, pass_through, plain, serve_timeshift};
use crate::process::Input;
use crate::remux;
use crate::timeshift::{self, Session};

/// What every Chromecast decodes in HLS: H.264 and AAC. The rest is converted.
const TV_DECODES: PlaybackSupport = PlaybackSupport { hevc: false, ac3: false, eac3: false, vp9: false, av1: false };
/// A live channel's buffer for the TV: enough to pause for a while there.
const LIVE_WINDOW: f64 = 300.0;
/// How long starting a live channel waits for its first segment.
const LIVE_START_WAIT: Duration = Duration::from_secs(20);

const NOT_CASTABLE: &str = "Xorvio can't send this kind of stream to a TV.";
const GROWING: &str = "A recording still being made can go to a TV once it's finished.";
const LIVE_NEEDS_FFMPEG: &str = "Xorvio needs ffmpeg to send live channels to a TV.";

/// How a stream reaches the TV, for the request that sends it there.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CastMedia {
    /// What the TV fetches: this computer's network address, the
    /// listener's token and the stream's id. Nothing about the provider.
    pub url: String,
    pub content_type: String,
    pub live: bool,
    /// HLS made of fragmented MP4, which the TV has to be told.
    pub hls_fmp4: bool,
    /// The length of what the TV plays (from the offset on).
    pub duration_seconds: Option<f64>,
    /// Where the TV's stream starts in the title: one ffmpeg makes starts
    /// where it was asked for.
    pub offset_seconds: f64,
    /// Where the TV should start within its stream (a file that goes as it
    /// is starts where the film was).
    pub start_seconds: f64,
    /// Seeking means asking for the stream again from there
    /// ([`Gateway::cast_seek`]); otherwise the TV seeks by itself.
    pub seeks_by_restart: bool,
    /// What was done to it ("Video encoded to H.264").
    pub notes: Vec<String>,
}

/// A stream registered for the TV.
struct CastEntry {
    /// The target, the stop signal and (for a live channel) the buffer, as
    /// the player's streams have.
    entry: Entry,
    route: Route,
    offset: f64,
    /// The title's whole length, when known.
    duration: Option<f64>,
}

/// The listener on the network, while something is cast.
#[derive(Default)]
pub(crate) struct Casting {
    lan: Mutex<Option<Lan>>,
}

struct Lan {
    /// This computer's address the TV reaches it at.
    local: IpAddr,
    origin: String,
    serving: Arc<Serving>,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

/// What the listener's requests read.
struct Serving {
    token: String,
    /// The TV's addresses; nobody else is answered.
    peers: Mutex<Vec<IpAddr>>,
    entries: Mutex<HashMap<String, Arc<CastEntry>>>,
    /// Requests the TV has made, to tell "couldn't reach this computer"
    /// apart from "couldn't play it".
    requests: AtomicUsize,
    gateway: Weak<Shared>,
}

impl Serving {
    fn allows(&self, peer: IpAddr) -> bool {
        let peer = canonical(peer);
        lock(&self.peers).iter().any(|allowed| canonical(*allowed) == peer)
    }
}

/// An IPv4 address that came as IPv6 (`::ffff:192.168.1.30`) as itself.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

impl Gateway {
    /// Whether a stream can go to a TV, said before anything is opened:
    /// `None` when it can, otherwise why not, for people.
    pub fn cast_check(&self, target: &StreamTarget, hint: &PlayHint) -> Option<String> {
        let has_ffmpeg = self.shared.tools.is_some();
        let reason = match Input::of(target) {
            None => Some(NOT_CASTABLE),
            Some(Input::File(_)) if target.is_live => Some(GROWING),
            Some(_) if target.is_live => (!has_ffmpeg).then_some(LIVE_NEEDS_FFMPEG),
            Some(_) => {
                let extension = hint_extension(target, hint);
                let web = matches!(extension.as_deref(), Some("mp4" | "m4v" | "mov" | "webm"));
                (!has_ffmpeg && !web).then_some(NEEDS_FFMPEG)
            }
        };
        reason.map(String::from)
    }

    /// Gets `target` ready for the TV whose addresses are `tv`, from
    /// `start_seconds` in (films and episodes), and opens the listener on
    /// the network if it isn't already. Whatever was cast before stops: one
    /// TV plays one stream, and a live channel's provider allows one
    /// connection. Errors are for people.
    pub async fn cast(&self, target: StreamTarget, hint: PlayHint, start_seconds: f64, tv: &[IpAddr]) -> anyhow::Result<CastMedia> {
        let Some(&first) = tv.iter().find(|ip| ip.is_ipv4()).or(tv.first()) else {
            bail!("Xorvio doesn't know where that TV is on the network.");
        };
        if let Some(reason) = self.cast_check(&target, &hint) {
            bail!(reason);
        }
        self.cast_release();
        let start = if start_seconds.is_finite() { start_seconds.max(0.0) } else { 0.0 };
        let extension = hint_extension(&target, &hint);
        let (entry, route, offset) = if target.is_live {
            let Some(tools) = &self.shared.tools else { bail!(FFMPEG_MISSING) };
            // The provider allows one connection: the player's stream of
            // this channel goes, if it's still open.
            self.shared.release_live(&target.url);
            timeshift::sweep(&self.shared.timeshift_root, &self.shared.timeshift_dirs());
            let session = Session::start_decoding(&self.shared.http, &tools.ffmpeg, &self.shared.timeshift_root, &target, TV_DECODES, LIVE_WINDOW).await?;
            if let Err(unready) = session.wait_ready(LIVE_START_WAIT).await {
                session.stop();
                bail!("{}", unready.message());
            }
            let entry = Entry::new(target, PlaybackMode::Hls, None, 0.0, None).with_timeshift(session);
            (entry, Route::Live, 0.0)
        } else {
            // The player has usually probed it already; ffprobe can take seconds.
            let known = lock(&self.shared.streams).by_id.values().find(|e| e.target.url == target.url && e.probe.is_some()).and_then(|e| e.probe.clone());
            let probe = match known {
                Some(probe) => Some(probe),
                None => self.shared.probe(&target).await,
            };
            let route = plan::route(probe.as_ref(), extension.as_deref(), self.shared.tools.is_some()).map_err(|reason| anyhow!(reason))?;
            let duration = probe.as_ref().and_then(|p| p.duration).or(hint.duration_seconds);
            let offset = match route {
                Route::Mp4(_) => duration.map_or(start, |d| start.min((d - 1.0).max(0.0))),
                _ => 0.0,
            };
            let entry = Entry::new(target, PlaybackMode::Native, duration, offset, None).with_extension(extension);
            (entry, route, offset)
        };
        let duration = entry.duration;
        let entry = CastEntry { entry, route, offset, duration };
        let serving = self.shared.lan_for(first, tv).await?;
        Ok(self.shared.register_cast(&serving, entry, start))
    }

    /// The stream ffmpeg makes for the TV, again from `offset_seconds` in
    /// the title, under a fresh address. The old address stops.
    pub async fn cast_seek(&self, url: &str, offset_seconds: f64) -> anyhow::Result<CastMedia> {
        let serving = self.shared.serving().ok_or_else(|| anyhow!(STREAM_GONE))?;
        let id = id_in(&serving, url).ok_or_else(|| anyhow!(STREAM_GONE))?;
        let old = lock(&serving.entries).get(&id).cloned().ok_or_else(|| anyhow!(STREAM_GONE))?;
        if !matches!(old.route, Route::Mp4(_)) {
            bail!("This stream seeks on the TV itself.");
        }
        let mut offset = if offset_seconds.is_finite() { offset_seconds.max(0.0) } else { 0.0 };
        if let Some(duration) = old.duration {
            offset = offset.min((duration - 1.0).max(0.0));
        }
        let entry = Entry::new(old.entry.target.clone(), PlaybackMode::Native, old.duration, offset, None).with_extension(old.entry.extension.clone());
        let fresh = CastEntry { entry, route: old.route.clone(), offset, duration: old.duration };
        if let Some(gone) = lock(&serving.entries).remove(&id) {
            gone.entry.halt();
        }
        Ok(self.shared.register_cast(&serving, fresh, 0.0))
    }

    /// How many requests the TV has made since the listener opened.
    pub fn cast_requests(&self) -> usize {
        self.shared.serving().map_or(0, |s| s.requests.load(Ordering::Relaxed))
    }

    /// Stops what's cast and closes the listener on the network.
    pub fn cast_end(&self) {
        let lan = lock(&self.shared.cast.lan).take();
        let Some(mut lan) = lan else { return };
        let entries: Vec<Arc<CastEntry>> = lock(&lan.serving.entries).drain().map(|(_, entry)| entry).collect();
        for entry in entries {
            entry.entry.halt();
            if let Some(session) = &entry.entry.timeshift {
                session.stop_now();
            }
        }
        if let Some(shutdown) = lan.shutdown.take() {
            let _ = shutdown.send(());
        }
        // The TV may hold a connection open; it isn't waited for long. When
        // Xorvio quits there's no runtime left to wait in.
        let task = lan.task;
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    let abort = task.abort_handle();
                    if tokio::time::timeout(Duration::from_secs(3), task).await.is_err() {
                        abort.abort();
                    }
                });
            }
            Err(_) => task.abort(),
        }
        tracing::info!("stopped casting; the listener on the network is closed");
    }

    /// Stops what's cast, keeping the listener for what's cast next.
    fn cast_release(&self) {
        let Some(serving) = self.shared.serving() else { return };
        let entries: Vec<Arc<CastEntry>> = lock(&serving.entries).drain().map(|(_, entry)| entry).collect();
        for entry in entries {
            entry.entry.halt();
        }
    }

    /// The listener's address, for tests.
    #[doc(hidden)]
    pub fn cast_origin(&self) -> Option<String> {
        lock(&self.shared.cast.lan).as_ref().map(|lan| lan.origin.clone())
    }
}

/// The extension a hint or the address gives ("mkv").
fn hint_extension(target: &StreamTarget, hint: &PlayHint) -> Option<String> {
    hint.extension.as_deref().map(clean_extension).filter(|e| !e.is_empty()).or_else(|| extension_of(&target.url))
}

/// The stream id in one of the listener's addresses.
fn id_in(serving: &Serving, url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    let mut parts = url.path_segments()?;
    (parts.next()? == serving.token).then_some(())?;
    parts.next()?;
    parts.next().map(String::from)
}

/// This computer's address on the way to `peer`: connecting a UDP socket
/// picks the interface and sends nothing.
fn local_address_towards(peer: IpAddr) -> std::io::Result<IpAddr> {
    let any = if peer.is_ipv4() { IpAddr::V4(Ipv4Addr::UNSPECIFIED) } else { IpAddr::V6(Ipv6Addr::UNSPECIFIED) };
    let socket = std::net::UdpSocket::bind((any, 0))?;
    socket.connect((peer, 8009))?;
    Ok(socket.local_addr()?.ip())
}

impl Shared {
    fn serving(&self) -> Option<Arc<Serving>> {
        lock(&self.cast.lan).as_ref().map(|lan| lan.serving.clone())
    }

    /// Whether a TV has something from this computer.
    pub(super) fn is_casting(&self) -> bool {
        self.serving().is_some_and(|serving| !lock(&serving.entries).is_empty())
    }

    /// Releases the player's streams of the live channel at `url`.
    fn release_live(&self, url: &str) {
        let ids: Vec<String> = {
            let streams = lock(&self.streams);
            // Other players' addresses are theirs to close.
            streams.by_id.iter().filter(|(id, entry)| entry.target.is_live && entry.target.url == url && !streams.external.contains(id)).map(|(id, _)| id.clone()).collect()
        };
        for id in ids {
            tracing::info!("releasing the player's stream of the channel going to the TV");
            self.release(&id);
        }
    }

    /// The listener for a TV reached through `first`, opened on the
    /// interface facing it. One already open on that interface is kept,
    /// and answers this TV from now on.
    async fn lan_for(self: &Arc<Self>, first: IpAddr, tv: &[IpAddr]) -> anyhow::Result<Arc<Serving>> {
        let local = local_address_towards(first).map_err(|error| {
            tracing::warn!("no route to the TV: {error}");
            anyhow!("Xorvio can't reach that TV from this computer's network.")
        })?;
        {
            let lan = lock(&self.cast.lan);
            if let Some(lan) = lan.as_ref().filter(|lan| lan.local == local) {
                *lock(&lan.serving.peers) = tv.to_vec();
                return Ok(lan.serving.clone());
            }
        }
        if lock(&self.cast.lan).is_some() {
            // The TV is on another network than the last one.
            let lan = lock(&self.cast.lan).take();
            if let Some(mut lan) = lan
                && let Some(shutdown) = lan.shutdown.take()
            {
                let _ = shutdown.send(());
            }
        }
        let listener = tokio::net::TcpListener::bind((local, 0)).await.map_err(|error| {
            tracing::warn!("the listener for the TV couldn't open: {error}");
            anyhow!("Xorvio couldn't open a connection for the TV on this network.")
        })?;
        let port = listener.local_addr()?.port();
        let origin = format!("http://{}", SocketAddr::new(local, port));
        let serving = Arc::new(Serving {
            token: uuid::Uuid::new_v4().simple().to_string(),
            peers: Mutex::new(tv.to_vec()),
            entries: Mutex::default(),
            requests: AtomicUsize::new(0),
            gateway: Arc::downgrade(self),
        });
        let app = Router::new().route("/{token}/{kind}/{id}/{name}", any(handle)).with_state(serving.clone());
        let (shutdown, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let server = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(async {
                let _ = stopped.await;
            });
            if let Err(error) = server.await {
                tracing::warn!("the listener for the TV stopped: {error}");
            }
        });
        // The address is this computer's on the home network: fine to log.
        tracing::info!(%local, port, "opened the listener for the TV");
        *lock(&self.cast.lan) = Some(Lan { local, origin, serving: serving.clone(), shutdown: Some(shutdown), task });
        Ok(serving)
    }

    fn register_cast(&self, serving: &Serving, entry: CastEntry, start: f64) -> CastMedia {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let origin = lock(&self.cast.lan).as_ref().map(|lan| lan.origin.clone()).unwrap_or_default();
        let (name, content_type, hls_fmp4) = match &entry.route {
            Route::Direct { content_type } => (if *content_type == "video/webm" { "video.webm" } else { "video.mp4" }, *content_type, false),
            Route::Mp4(_) => ("video.mp4", "video/mp4", false),
            Route::Live => (timeshift::PLAYLIST, "application/x-mpegurl", true),
        };
        let media = CastMedia {
            url: format!("{origin}/{}/{}/{id}/{name}", serving.token, entry.route.kind()),
            content_type: content_type.to_string(),
            live: matches!(entry.route, Route::Live),
            hls_fmp4,
            duration_seconds: entry.duration.map(|d| (d - entry.offset).max(0.0)),
            offset_seconds: entry.offset,
            start_seconds: if matches!(entry.route, Route::Direct { .. }) { start } else { 0.0 },
            seeks_by_restart: matches!(entry.route, Route::Mp4(_)),
            notes: match &entry.route {
                Route::Mp4(plan) => plan.notes(),
                Route::Live => entry.entry.timeshift.as_ref().map(|s| s.info().notes).unwrap_or_default(),
                Route::Direct { .. } => Vec::new(),
            },
        };
        tracing::info!(kind = entry.route.kind(), offset = entry.offset, notes = ?media.notes, "a stream is ready for the TV");
        lock(&serving.entries).insert(id, Arc::new(entry));
        media
    }
}

// MARK: Serving

async fn handle(
    State(serving): State<Arc<Serving>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    UrlPath((token, kind, id, name)): UrlPath<(String, String, String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Response<Body> {
    // Everything refused looks the same: there's nothing here for anyone else.
    if !serving.allows(peer.ip()) {
        tracing::info!(peer = %peer.ip(), "the listener for the TV refused another device");
        return plain(StatusCode::NOT_FOUND, "Not found");
    }
    if token != serving.token {
        return plain(StatusCode::NOT_FOUND, "Not found");
    }
    if method == Method::OPTIONS {
        return cors(Response::builder().status(StatusCode::NO_CONTENT).body(Body::empty()).unwrap_or_default());
    }
    if method != Method::GET && method != Method::HEAD {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "Only GET and HEAD");
    }
    let Some(cast) = lock(&serving.entries).get(&id).cloned() else {
        return plain(StatusCode::NOT_FOUND, STREAM_GONE);
    };
    if cast.route.kind() != kind {
        return plain(StatusCode::NOT_FOUND, "Not found");
    }
    let Some(shared) = serving.gateway.upgrade() else {
        return plain(StatusCode::SERVICE_UNAVAILABLE, STREAM_GONE);
    };
    serving.requests.fetch_add(1, Ordering::Relaxed);
    tracing::debug!(%method, %kind, %name, range = ?headers.get(header::RANGE), "TV request");
    let response = match &cast.route {
        Route::Direct { content_type } => {
            let mut response = pass_through(&shared.http, &cast.entry.target, None, &method, &headers, Some(cast.entry.halted())).await;
            // Providers often call films application/octet-stream; the TV goes by the type.
            if response.status().is_success() {
                response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
            }
            response
        }
        Route::Mp4(plan) => serve_mp4(&shared, &cast, plan, &method),
        Route::Live => serve_timeshift(&cast.entry, &name, &method, &headers).await,
    };
    cors(response)
}

/// Starts ffmpeg for this request and streams fragmented MP4 back. A new
/// request stops the ffmpeg serving the one before.
fn serve_mp4(shared: &Shared, cast: &CastEntry, plan: &CastPlan, method: &Method) -> Response<Body> {
    let Some(tools) = &shared.tools else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, FFMPEG_MISSING);
    };
    let Some(input) = Input::of(&cast.entry.target) else {
        return plain(StatusCode::BAD_REQUEST, "Unsupported address");
    };
    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ACCEPT_RANGES, "none");
    if method == Method::HEAD {
        return builder.body(Body::empty()).unwrap_or_default();
    }
    cast.entry.halt();
    let generation = *cast.entry.stop.borrow();
    let args = plan.args(&cast.entry.target, &input, cast.offset);
    match remux::spawn(&tools.ffmpeg, args, cast.entry.stop.subscribe(), generation) {
        Ok(stream) => builder.body(Body::from_stream(stream)).unwrap_or_default(),
        Err(error) => {
            tracing::error!("ffmpeg couldn't be started for the TV: {error:#}");
            plain(StatusCode::INTERNAL_SERVER_ERROR, "ffmpeg couldn't be started.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_tv_is_answered() {
        let serving = Serving {
            token: "t".into(),
            peers: Mutex::new(vec!["192.168.1.30".parse().unwrap(), "fe80::1".parse().unwrap()]),
            entries: Mutex::default(),
            requests: AtomicUsize::new(0),
            gateway: Weak::new(),
        };
        assert!(serving.allows("192.168.1.30".parse().unwrap()));
        assert!(serving.allows("::ffff:192.168.1.30".parse().unwrap()), "the same address over IPv6");
        assert!(serving.allows("fe80::1".parse().unwrap()));
        assert!(!serving.allows("192.168.1.31".parse().unwrap()));
        assert!(!serving.allows("127.0.0.1".parse().unwrap()), "not even this computer");
    }

    #[test]
    fn the_address_facing_loopback_is_loopback() {
        assert_eq!(local_address_towards("127.0.0.1".parse().unwrap()).unwrap(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
}
