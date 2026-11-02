//! Media servers' live channels. A server holds one of its tuners for a
//! channel from the moment it's asked to open it until it's told to let go,
//! so these channels aren't played from the server's address: each gets a
//! fixed address on this computer, served by a small relay on 127.0.0.1.
//! Reading that address tunes the channel on the server and passes its
//! stream on as MPEG-TS; a few seconds after the last reader leaves, the
//! tuner is let go. The gateway, the live buffer, Multiview and the recorder
//! read these addresses like any other live channel's, and whatever stops
//! reading (a closed player, a finished recording, Xorvio quitting) frees the
//! tuner.
//!
//! Readers of one channel share one tuning, as the server's own apps do, and
//! a reader back within a few seconds (the gateway opens a channel again to
//! move it into the live buffer) finds it still tuned.
//!
//! The token stays in here: the relay's addresses are random paths, and the
//! server's address and headers are used only for reading from the server.
//! Tunings are noted in the library while they're held, so any a crash left
//! behind are let go the next time Xorvio starts ([`Core::let_go_of_abandoned_tuners`]),
//! and all of them as it quits ([`Core::let_go_of_tuners`]).

use std::collections::{HashMap, VecDeque};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::error::SourceError;
use crate::http::{ApiCall, Http, IDLE_TIMEOUT};
use crate::sources::server::{self, Release, ServerSession, Tuned};
use crate::store::lock;
use crate::{Core, Result};

/// How long a channel stays tuned after its last reader leaves, and how
/// often a server that wants to hear it is told a channel is still watched.
const TIMING: Timing = Timing { linger: Duration::from_secs(10), keepalive: Duration::from_secs(10) };
/// The longest request head the relay reads.
const MAX_HEAD: usize = 16 * 1024;
/// Where the relay notes the tunings it holds.
pub(crate) const HELD_PREFERENCE: &str = "held_tuners";
/// How long Xorvio waits, as it quits, for servers to let their tuners go.
const QUITTING_WAIT: Duration = Duration::from_secs(3);

/// A tuning held on a server, as noted in the library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Held {
    pub source_id: String,
    pub release: Release,
}

/// What the relay knows of one channel it has given an address.
struct Channel {
    route: String,
    source_id: String,
    key: String,
    dvr: Option<String>,
    /// The latest sign-in, which a source signed in again replaces.
    session: Mutex<ServerSession>,
    /// The channel as tuned on the server, while it is.
    tuning: tokio::sync::Mutex<Option<Tuning>>,
    readers: AtomicUsize,
    /// Moves on with every reader, so a letting-go planned before one came stands down.
    visits: AtomicU64,
}

struct Tuning {
    tuned: Tuned,
    /// A reader lost the server's stream, so the next one tunes afresh.
    broken: bool,
    /// Telling the server the channel is still watched, until this goes.
    keepalive: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for Tuning {
    fn drop(&mut self) {
        if let Some(keepalive) = self.keepalive.take() {
            keepalive.abort();
        }
    }
}

#[derive(Default)]
struct Table {
    by_route: HashMap<String, Arc<Channel>>,
    /// (source, channel key) → route.
    routes: HashMap<(String, String), String>,
}

type Noter = Arc<dyn Fn(Vec<Held>) + Send + Sync>;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    pub linger: Duration,
    pub keepalive: Duration,
}

pub(crate) struct TunerRelay {
    origin: String,
    http: Http,
    table: Mutex<Table>,
    /// The tunings held now, by route, and where they're noted.
    held: Mutex<HashMap<String, Held>>,
    note: Noter,
    timing: Timing,
}

impl TunerRelay {
    /// Starts the relay on a free port of 127.0.0.1, on the async runtime
    /// this is called from. `note` is given every tuning held whenever that changes.
    pub(crate) fn start(http: Http, note: Noter, timing: Timing) -> anyhow::Result<Arc<Self>> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| anyhow::anyhow!("A media server's channel can only be opened while Xorvio is running."))?;
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let origin = format!("http://{}", listener.local_addr()?);
        let relay = Arc::new(Self {
            origin,
            http,
            table: Mutex::default(),
            held: Mutex::default(),
            note,
            timing,
        });
        let serving = Arc::downgrade(&relay);
        runtime.spawn(async move {
            let Ok(listener) = tokio::net::TcpListener::from_std(listener) else { return };
            while let Ok((socket, _)) = listener.accept().await {
                let Some(relay) = serving.upgrade() else { break };
                tokio::spawn(relay.serve(socket));
            }
        });
        Ok(relay)
    }

    /// The channel's address on the relay, the same each time it's asked
    /// for. `session` replaces the one kept, so a renewed sign-in is used
    /// from the next reader on.
    pub(crate) fn address(&self, source_id: &str, session: ServerSession, key: &str, dvr: Option<&str>) -> String {
        let mut table = lock(&self.table);
        let index = (source_id.to_string(), key.to_string());
        let existing = table.routes.get(&index).and_then(|route| table.by_route.get(route)).cloned();
        let channel = match existing {
            Some(channel) if channel.dvr.as_deref() == dvr => {
                *lock(&channel.session) = session;
                channel
            }
            _ => {
                let route = uuid::Uuid::new_v4().simple().to_string();
                let channel = Arc::new(Channel {
                    route: route.clone(),
                    source_id: source_id.to_string(),
                    key: key.to_string(),
                    dvr: dvr.map(str::to_string),
                    session: Mutex::new(session),
                    tuning: tokio::sync::Mutex::new(None),
                    readers: AtomicUsize::new(0),
                    visits: AtomicU64::new(0),
                });
                table.routes.insert(index, route.clone());
                table.by_route.insert(route, channel.clone());
                channel
            }
        };
        format!("{}/{}/live.ts", self.origin, channel.route)
    }

    /// Lets every tuner go now, for when Xorvio quits.
    pub(crate) async fn let_go_of_everything(&self) {
        let channels: Vec<Arc<Channel>> = lock(&self.table).by_route.values().cloned().collect();
        let letting_go = channels.iter().map(|channel| async move {
            let taken = channel.tuning.lock().await.take();
            if let Some(tuning) = taken {
                self.release(channel, &tuning.tuned).await;
            }
        });
        futures_util::future::join_all(letting_go).await;
    }

    async fn serve(self: Arc<Self>, mut socket: TcpStream) {
        let Some((method, path)) = read_head(&mut socket).await else { return };
        let route = path.trim_start_matches('/').split('/').next().unwrap_or_default().to_string();
        let channel = lock(&self.table).by_route.get(&route).cloned();
        let Some(channel) = channel else {
            return respond(&mut socket, 404, "There's no channel at this address.").await;
        };
        match method.as_str() {
            // Asking what's there doesn't tune anything.
            "HEAD" => {
                let _ = socket.write_all(stream_head().as_bytes()).await;
            }
            "GET" => self.pass_on(&channel, &mut socket).await,
            _ => respond(&mut socket, 405, "Only GET and HEAD are answered here.").await,
        }
        let _ = socket.shutdown().await;
    }

    /// Tunes the channel (or joins its tuning) and passes the server's
    /// stream to `socket` until either end stops.
    async fn pass_on(self: &Arc<Self>, channel: &Arc<Channel>, socket: &mut TcpStream) {
        channel.readers.fetch_add(1, Ordering::SeqCst);
        channel.visits.fetch_add(1, Ordering::SeqCst);
        if let Err(error) = self.read_into(channel, socket).await {
            tracing::info!("a server's channel couldn't be passed on: {error}");
            respond(socket, status_for(&error), &error.to_string()).await;
        }
        if channel.readers.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.let_go_later(channel.clone());
        }
    }

    /// The server's stream into `socket`. Errors are those before anything
    /// was sent, which can still be answered with a status.
    async fn read_into(&self, channel: &Channel, socket: &mut TcpStream) -> std::result::Result<(), SourceError> {
        let mut upstream = None;
        // A tuning shared from before may have gone on the server (it restarted, or let it go itself): tune afresh once.
        for _ in 0..2 {
            let (tuned, fresh) = self.tuning(channel).await?;
            match Upstream::open(&self.http, &tuned).await {
                Ok(opened) => {
                    upstream = Some(opened);
                    break;
                }
                Err(error) if fresh => return Err(error),
                Err(_) => self.mark_broken(channel).await,
            }
        }
        let Some(mut upstream) = upstream else {
            return Err(SourceError::Network("The server's stream for this channel couldn't be opened.".into()));
        };
        if socket.write_all(stream_head().as_bytes()).await.is_err() {
            return Ok(());
        }
        // The reader closing is heard at once, not at the next write (an HLS
        // segment can be seconds away), so the tuner is let go on time.
        let (mut incoming, mut outgoing) = socket.split();
        let mut ignored = [0u8; 1024];
        loop {
            tokio::select! {
                next = upstream.next() => match next {
                    Some(Ok(bytes)) => {
                        if outgoing.write_all(&bytes).await.is_err() {
                            return Ok(());
                        }
                    }
                    Some(Err(error)) => {
                        tracing::info!("a server's channel stopped sending: {error}");
                        self.mark_broken(channel).await;
                        return Ok(());
                    }
                    None => {
                        self.mark_broken(channel).await;
                        return Ok(());
                    }
                },
                read = incoming.read(&mut ignored) => {
                    if matches!(read, Ok(0) | Err(_)) {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// The channel's tuning, and whether it was made just now. A broken one
    /// is let go and replaced.
    async fn tuning(&self, channel: &Channel) -> std::result::Result<(Tuned, bool), SourceError> {
        let mut slot = channel.tuning.lock().await;
        if let Some(tuning) = slot.as_ref().filter(|tuning| !tuning.broken) {
            return Ok((tuning.tuned.clone(), false));
        }
        if let Some(old) = slot.take() {
            self.release(channel, &old.tuned).await;
        }
        let session = lock(&channel.session).clone();
        let tuned = server::tune(&self.http, &session, &channel.key, channel.dvr.as_deref()).await?;
        self.noted(&channel.route, Some(Held { source_id: channel.source_id.clone(), release: tuned.release.clone() }));
        let keepalive = tuned.release.wants_keepalive().then(|| {
            let (http, release, every) = (self.http.clone(), tuned.release.clone(), self.timing.keepalive);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(every).await;
                    if let Err(error) = server::keep_alive(&http, &session, &release).await {
                        tracing::info!("a server wasn't told its channel is still watched: {error}");
                    }
                }
            })
        });
        *slot = Some(Tuning { tuned: tuned.clone(), broken: false, keepalive });
        Ok((tuned, true))
    }

    async fn mark_broken(&self, channel: &Channel) {
        if let Some(tuning) = channel.tuning.lock().await.as_mut() {
            tuning.broken = true;
        }
    }

    /// Lets the channel's tuner go once it has had no readers for a while.
    fn let_go_later(self: &Arc<Self>, channel: Arc<Channel>) {
        let visit = channel.visits.load(Ordering::SeqCst);
        let relay = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(relay.timing.linger).await;
            let mut slot = channel.tuning.lock().await;
            if channel.readers.load(Ordering::SeqCst) > 0 || channel.visits.load(Ordering::SeqCst) != visit {
                return;
            }
            if let Some(tuning) = slot.take() {
                relay.release(&channel, &tuning.tuned).await;
            }
        });
    }

    async fn release(&self, channel: &Channel, tuned: &Tuned) {
        let session = lock(&channel.session).clone();
        if let Err(error) = server::release(&self.http, &session, &tuned.release).await {
            tracing::info!("a server's tuner couldn't be let go (it will be once the server notices): {error}");
        }
        self.noted(&channel.route, None);
    }

    /// Notes a tuning held for `route`, or none.
    fn noted(&self, route: &str, held: Option<Held>) {
        let all: Vec<Held> = {
            let mut list = lock(&self.held);
            match held {
                Some(held) => list.insert(route.to_string(), held),
                None => list.remove(route),
            };
            list.values().cloned().collect()
        };
        (self.note)(all);
    }
}

// MARK: - Reading from the server

/// The server's stream: a transport stream read as it comes, or an HLS
/// playlist whose segments are read in turn and passed on one after another
/// (MPEG-TS segments make one transport stream joined end to end).
enum Upstream {
    Transport(futures_util::stream::BoxStream<'static, reqwest::Result<Vec<u8>>>),
    Playlist(Box<Follower>),
}

impl Upstream {
    async fn open(http: &Http, tuned: &Tuned) -> std::result::Result<Self, SourceError> {
        if tuned.playlist {
            let mut follower = Follower::new(http.clone(), &tuned.url, tuned.headers.clone())?;
            follower.load().await?;
            return Ok(Self::Playlist(Box::new(follower)));
        }
        let response = http.open_live(&tuned.url, &tuned.headers).await?;
        Ok(Self::Transport(response.bytes_stream().map(|chunk| chunk.map(Vec::from)).boxed()))
    }

    async fn next(&mut self) -> Option<std::result::Result<Vec<u8>, SourceError>> {
        match self {
            Self::Transport(stream) => match tokio::time::timeout(IDLE_TIMEOUT, stream.next()).await {
                Err(_) => Some(Err(SourceError::Network("The server stopped sending this channel.".into()))),
                Ok(None) => None,
                Ok(Some(Err(_))) => Some(Err(SourceError::Network("The connection to the server was interrupted.".into()))),
                Ok(Some(Ok(bytes))) => Some(Ok(bytes)),
            },
            Self::Playlist(follower) => follower.next().await.transpose(),
        }
    }
}

/// Follows a live HLS playlist: each new segment once, in order, from near
/// the live edge.
struct Follower {
    http: Http,
    playlist: url::Url,
    headers: Vec<(String, String)>,
    /// Segments listed but not yet read.
    queue: VecDeque<url::Url>,
    /// The sequence number of the last segment queued.
    last: Option<u64>,
    target: Duration,
    ended: bool,
    loaded: bool,
}

/// Segments read from the end of a live playlist when following starts, as players do.
const SEGMENTS_FROM_EDGE: usize = 3;
/// Playlist reads in a row that may bring nothing new (each half a segment apart).
const MAX_WAITS: usize = 30;

impl Follower {
    fn new(http: Http, playlist: &str, headers: Vec<(String, String)>) -> std::result::Result<Self, SourceError> {
        let playlist = url::Url::parse(playlist).map_err(|_| SourceError::Network("The server gave an address Xorvio can't read.".into()))?;
        Ok(Self { http, playlist, headers, queue: VecDeque::new(), last: None, target: Duration::from_secs(2), ended: false, loaded: false })
    }

    /// The headers for `url`: the server's own, only for the server.
    fn headers_for(&self, url: &url::Url) -> &[(String, String)] {
        if url.origin() == self.playlist.origin() { &self.headers } else { &[] }
    }

    async fn get(&self, url: &url::Url) -> std::result::Result<Vec<u8>, SourceError> {
        let reply = self.http.api(ApiCall::get(url.as_str(), self.headers_for(url))).await?;
        Ok(reply.bytes)
    }

    /// Reads the playlist again and queues the segments new since the last read.
    async fn load(&mut self) -> std::result::Result<(), SourceError> {
        let mut listed = self.list().await?;
        if let Some(variant) = listed.variant.take() {
            // A master playlist: follow its first variant from now on.
            self.playlist = self.playlist.join(&variant).map_err(|_| SourceError::Network("The server's playlist couldn't be read.".into()))?;
            listed = self.list().await?;
        }
        if !listed.is_playlist || listed.variant.is_some() {
            return Err(SourceError::Network("The server didn't send a playlist for this channel.".into()));
        }
        self.target = Duration::from_secs(listed.target_duration.clamp(1, 10));
        self.ended = listed.ended;
        let count = listed.segments.len() as u64;
        let mut fresh: Vec<(u64, String)> =
            listed.segments.into_iter().enumerate().map(|(index, uri)| (listed.media_sequence + index as u64, uri)).collect();
        match self.last {
            Some(last) => fresh.retain(|(sequence, _)| *sequence > last),
            None if !self.ended => {
                let skip = fresh.len().saturating_sub(SEGMENTS_FROM_EDGE);
                fresh.drain(..skip);
            }
            None => {}
        }
        for (sequence, uri) in fresh {
            if let Ok(url) = self.playlist.join(&uri) {
                self.queue.push_back(url);
                self.last = Some(sequence);
            }
        }
        if self.last.is_none() && count > 0 {
            self.last = Some(listed.media_sequence + count - 1);
        }
        self.loaded = true;
        Ok(())
    }

    async fn list(&self) -> std::result::Result<Listed, SourceError> {
        let text = String::from_utf8_lossy(&self.get(&self.playlist).await?).into_owned();
        Ok(parse_playlist(&text))
    }

    /// The next segment's bytes; `None` once a playlist that ends has been read.
    async fn next(&mut self) -> std::result::Result<Option<Vec<u8>>, SourceError> {
        let mut waits = 0;
        loop {
            if let Some(segment) = self.queue.pop_front() {
                return self.get(&segment).await.map(Some);
            }
            if self.ended {
                return Ok(None);
            }
            // Nothing new for a good while (a server's transcoder can take
            // several seconds to start): the stream has stalled.
            if waits > MAX_WAITS {
                return Err(SourceError::Network("The server stopped sending this channel.".into()));
            }
            if self.loaded {
                tokio::time::sleep(self.target / 2).await;
            }
            self.load().await?;
            waits += 1;
        }
    }
}

/// What a playlist lists.
#[derive(Debug, Default, PartialEq)]
struct Listed {
    is_playlist: bool,
    /// A master playlist's first variant.
    variant: Option<String>,
    media_sequence: u64,
    target_duration: u64,
    segments: Vec<String>,
    ended: bool,
}

fn parse_playlist(text: &str) -> Listed {
    let mut listed = Listed { target_duration: 2, ..Listed::default() };
    let mut variant_next = false;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if line.starts_with("#EXTM3U") {
            listed.is_playlist = true;
        } else if line.starts_with("#EXT-X-STREAM-INF") {
            variant_next = true;
        } else if let Some(value) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            listed.media_sequence = value.trim().parse().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            listed.target_duration = value.trim().parse::<f64>().map(|seconds| seconds.ceil() as u64).unwrap_or(2);
        } else if line.starts_with("#EXT-X-ENDLIST") {
            listed.ended = true;
        } else if !line.starts_with('#') {
            if variant_next {
                listed.variant.get_or_insert_with(|| line.to_string());
                variant_next = false;
            } else {
                listed.segments.push(line.to_string());
            }
        }
    }
    listed
}

// MARK: - Answering

/// The method and path of a request, once its head has arrived.
async fn read_head(socket: &mut TcpStream) -> Option<(String, String)> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 2048];
    while !data.windows(4).any(|window| window == b"\r\n\r\n") {
        if data.len() > MAX_HEAD {
            return None;
        }
        let count = tokio::time::timeout(IDLE_TIMEOUT, socket.read(&mut buffer)).await.ok()?.ok()?;
        if count == 0 {
            return None;
        }
        data.extend_from_slice(&buffer[..count]);
    }
    let head = String::from_utf8_lossy(&data);
    let mut first = head.lines().next()?.split_whitespace();
    Some((first.next()?.to_string(), first.next()?.to_string()))
}

/// The head of a stream reply: MPEG-TS for as long as it lasts.
fn stream_head() -> String {
    "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n".to_string()
}

async fn respond(socket: &mut TcpStream, status: u16, message: &str) {
    let reason = match status {
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        503 => "Service Unavailable",
        _ => "Bad Gateway",
    };
    let reply = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{message}",
        message.len()
    );
    let _ = socket.write_all(reply.as_bytes()).await;
}

/// The status a reader is answered with when the channel can't be tuned:
/// refused (the sign-in), busy (no tuner free), or the server's trouble.
fn status_for(error: &SourceError) -> u16 {
    match error {
        error if error.is_authentication() => 403,
        SourceError::Http(401 | 403) => 403,
        SourceError::Unavailable(_) => 503,
        _ => 502,
    }
}

// MARK: - Core

impl Core {
    /// The relay, started the first time a server's channel is played.
    pub(crate) fn tuner_relay(&self) -> Result<Arc<TunerRelay>> {
        let mut slot = lock(&self.tuners);
        if let Some(relay) = slot.as_ref() {
            return Ok(relay.clone());
        }
        let library = self.library.clone();
        let note: Noter = Arc::new(move |held: Vec<Held>| {
            let value = (!held.is_empty()).then(|| serde_json::to_string(&held).unwrap_or_default());
            if let Err(error) = library.set_preference(HELD_PREFERENCE, value.as_deref()) {
                tracing::warn!("the tuners held on servers couldn't be noted: {error}");
            }
        });
        let relay = TunerRelay::start(self.http.clone(), note, TIMING)?;
        // Taken before this run notes its own tunings over them.
        let abandoned = self.take_abandoned();
        let http = self.http.clone();
        tokio::spawn(async move { let_go_of(&http, abandoned).await });
        *slot = Some(relay.clone());
        Ok(relay)
    }

    /// Lets every server tuner Xorvio holds go, for when it quits. Waits a
    /// few seconds at most.
    pub async fn let_go_of_tuners(&self) {
        let relay = lock(&self.tuners).clone();
        if let Some(relay) = relay {
            let _ = tokio::time::timeout(QUITTING_WAIT, relay.let_go_of_everything()).await;
        }
    }

    /// Lets go of tunings noted as held when Xorvio last ran, which it
    /// didn't get to let go itself (it was closed abruptly). The relay does
    /// this itself when it starts first.
    pub async fn let_go_of_abandoned_tuners(&self) {
        let abandoned = {
            let slot = lock(&self.tuners);
            if slot.is_some() {
                return;
            }
            self.take_abandoned()
        };
        let_go_of(&self.http, abandoned).await;
    }

    /// The tunings noted as held, with their sources' sign-ins, forgotten as
    /// they're taken (so none is let go twice: a server counts each). Called
    /// with `tuners` locked.
    fn take_abandoned(&self) -> Vec<(ServerSession, Release)> {
        let Ok(Some(noted)) = self.library.preference(HELD_PREFERENCE) else { return Vec::new() };
        if let Err(error) = self.library.set_preference(HELD_PREFERENCE, None) {
            tracing::warn!("the tuners held on servers couldn't be noted: {error}");
            return Vec::new();
        }
        let held: Vec<Held> = serde_json::from_str(&noted).unwrap_or_default();
        held.into_iter().filter_map(|item| Some((self.server_session(&item.source_id).ok()?, item.release))).collect()
    }
}

async fn let_go_of(http: &Http, abandoned: Vec<(ServerSession, Release)>) {
    for (session, release) in abandoned {
        match server::release(http, &session, &release).await {
            Ok(()) => tracing::info!("let go of a server's tuner left held when Xorvio last closed"),
            Err(error) => tracing::info!("a tuner left from before couldn't be let go: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use xorvio_model::SourceKind;

    use super::*;

    #[test]
    fn playlists_are_read() {
        let media = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:120\n\
            #EXTINF:4.0,\nseg120.ts\n#EXTINF:4.0,\nseg121.ts\n";
        let listed = parse_playlist(media);
        assert!(listed.is_playlist && !listed.ended && listed.variant.is_none());
        assert_eq!((listed.media_sequence, listed.target_duration), (120, 4));
        assert_eq!(listed.segments, ["seg120.ts", "seg121.ts"]);
        let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=4000000\nlow/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=8000000\nhigh/index.m3u8\n";
        assert_eq!(parse_playlist(master).variant.as_deref(), Some("low/index.m3u8"));
        assert!(!parse_playlist("<html>busy</html>").is_playlist);
        assert!(parse_playlist("#EXTM3U\n#EXTINF:2,\na.ts\n#EXT-X-ENDLIST\n").ended);
    }

    #[test]
    fn refusals_become_statuses() {
        assert_eq!(status_for(&SourceError::SignedOut), 403);
        assert_eq!(status_for(&SourceError::Unavailable("All tuners are in use.".into())), 503);
        assert_eq!(status_for(&SourceError::Http(500)), 502);
    }

    // MARK: Against a stand-in server

    const TOKEN: &str = "c3t0ken9a1";

    /// What a stand-in server was asked, in order ("POST /Items/ch1/PlaybackInfo").
    type Seen = Arc<Mutex<Vec<String>>>;

    /// A media server on 127.0.0.1: Jellyfin's live TV routes, and Plex's
    /// tune, transcoder playlists, segments, timeline and stops. Streams send
    /// a packet every few milliseconds for as long as they're read, or end
    /// at once when `ending` is set; tuning fails while `busy` is.
    struct Stand {
        base: String,
        seen: Seen,
        busy: Arc<AtomicBool>,
        ending: Arc<AtomicBool>,
    }

    impl Stand {
        async fn start() -> Stand {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (seen, busy, ending): (Seen, Arc<AtomicBool>, Arc<AtomicBool>) = Default::default();
            let stand = Stand { base, seen: seen.clone(), busy: busy.clone(), ending: ending.clone() };
            tokio::spawn(async move {
                while let Ok((socket, _)) = listener.accept().await {
                    tokio::spawn(answer(socket, seen.clone(), busy.clone(), ending.clone()));
                }
            });
            stand
        }

        fn seen(&self) -> Vec<String> {
            lock(&self.seen).clone()
        }

        fn count(&self, start: &str) -> usize {
            self.seen().iter().filter(|line| line.starts_with(start)).count()
        }
    }

    async fn answer(mut socket: TcpStream, seen: Seen, busy: Arc<AtomicBool>, ending: Arc<AtomicBool>) {
        let mut head = Vec::new();
        let mut buffer = [0u8; 4096];
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            match socket.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(count) => head.extend_from_slice(&buffer[..count]),
            }
        }
        let text = String::from_utf8_lossy(&head).to_string();
        let mut first = text.lines().next().unwrap_or_default().split_whitespace();
        let (method, target) = (first.next().unwrap_or_default().to_string(), first.next().unwrap_or("/").to_string());
        let signed = text.to_lowercase().contains(&TOKEN.to_lowercase());
        lock(&seen).push(format!("{method} {target}"));
        let path = target.split('?').next().unwrap_or_default().to_string();
        let reply = |status: u16, body: &str| {
            format!("HTTP/1.1 {status} Stand\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
        };
        let text_reply = if !signed {
            reply(401, "")
        } else if path.ends_with("/PlaybackInfo") || path.ends_with("/tune") {
            if busy.load(Ordering::SeqCst) {
                reply(500, "Error processing request.")
            } else if path.ends_with("/tune") {
                reply(200, include_str!("../tests/fixtures/plex/livetv_tune.json"))
            } else {
                reply(200, r#"{"PlaySessionId":"ps-1","MediaSources":[{"Id":"ms-1","Path":"http://10.0.0.2:8096/LiveTv/LiveStreamFiles/u1/stream.ts","LiveStreamId":"ls-1","Container":"ts"}]}"#)
            }
        } else if path.starts_with("/Videos/") {
            if ending.load(Ordering::SeqCst) {
                reply(200, "")
            } else {
                // A transport stream that runs until the reader leaves.
                if socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nConnection: close\r\n\r\n").await.is_err() {
                    return;
                }
                let packet = [0x47u8; 188];
                while socket.write_all(&packet).await.is_ok() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                return;
            }
        } else if path.ends_with("/start.m3u8") {
            reply(200, "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=2562000\nsession/t/base/index.m3u8\n")
        } else if path.ends_with("/base/index.m3u8") {
            // A live playlist whose window moves on each second.
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
            let segments: String = (now..now + 4).map(|n| format!("#EXTINF:1.0,\n{n}.ts\n")).collect();
            reply(200, &format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:{now}\n{segments}"))
        } else if path.ends_with(".ts") {
            reply(200, &format!("segment {}|", path.rsplit('/').next().unwrap_or_default()))
        } else if path == "/LiveStreams/Close" || path == "/:/timeline" || path.ends_with("/stop") || path.starts_with("/media/grabbers/") {
            reply(200, "")
        } else {
            reply(404, "")
        };
        let _ = socket.write_all(text_reply.as_bytes()).await;
        let _ = socket.shutdown().await;
    }

    fn session(kind: SourceKind, base: &str) -> ServerSession {
        ServerSession { kind, base_url: base.into(), token: Some(TOKEN.into()), user_id: Some("u1".into()), device_id: "device-1".into() }
    }

    /// Every list of tunings a relay noted as held, in turn.
    type Notes = Arc<Mutex<Vec<Vec<Held>>>>;

    /// A relay that lets go quickly, and what it has noted as held.
    fn relay(keepalive: Duration) -> (Arc<TunerRelay>, Notes) {
        let notes: Notes = Arc::default();
        let kept = notes.clone();
        let note: Noter = Arc::new(move |held| lock(&kept).push(held));
        let timing = Timing { linger: Duration::from_millis(300), keepalive };
        (TunerRelay::start(Http::new().unwrap(), note, timing).unwrap(), notes)
    }

    /// The bytes a reader of `address` gets in `time` (its status first).
    async fn read(address: &str, time: Duration) -> (u16, Vec<u8>) {
        let mut response = reqwest::get(address).await.unwrap();
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        let until = tokio::time::Instant::now() + time;
        while let Ok(Ok(Some(chunk))) = tokio::time::timeout_at(until, response.chunk()).await {
            bytes.extend_from_slice(&chunk);
        }
        (status, bytes)
    }

    async fn settle() {
        tokio::time::sleep(Duration::from_millis(600)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn readers_share_a_tuning_that_is_let_go_after_the_last() {
        let stand = Stand::start().await;
        let (relay, notes) = relay(Duration::from_secs(60));
        let address = relay.address("src", session(SourceKind::Jellyfin, &stand.base), "ch1", None);
        assert!(address.starts_with("http://127.0.0.1:") && address.ends_with("/live.ts"));
        assert!(!address.contains(TOKEN) && !address.contains(&stand.base));
        assert_eq!(relay.address("src", session(SourceKind::Jellyfin, &stand.base), "ch1", None), address, "the same each time");

        let (one, two) = tokio::join!(read(&address, Duration::from_millis(400)), read(&address, Duration::from_millis(400)));
        assert_eq!((one.0, two.0), (200, 200));
        assert!(one.1.len() > 188 * 5 && one.1.iter().all(|byte| *byte == 0x47), "the server's stream, as it is");
        assert!(!two.1.is_empty());
        assert_eq!(stand.count("POST /Items/ch1/PlaybackInfo"), 1, "one tuning for both: {:?}", stand.seen());
        let stream = stand.seen().into_iter().find(|line| line.starts_with("GET /Videos/ch1/stream.ts")).unwrap();
        assert!(stream.contains("static=true") && stream.contains("LiveStreamId=ls-1") && stream.contains("MediaSourceId=ms-1"));
        assert_eq!(stand.count("POST /LiveStreams/Close"), 0, "still held while it lingers");

        // Back within the linger: the same tuning.
        let again = read(&address, Duration::from_millis(100)).await;
        assert_eq!(again.0, 200);
        assert_eq!(stand.count("POST /Items/ch1/PlaybackInfo"), 1);

        settle().await;
        let closes: Vec<String> = stand.seen().into_iter().filter(|line| line.starts_with("POST /LiveStreams/Close")).collect();
        assert_eq!(closes.len(), 1, "let go once: {closes:?}");
        assert!(closes[0].contains("LiveStreamId=ls-1") && closes[0].contains("PlaySessionId=ps-1"));
        let noted = lock(&notes).clone();
        assert_eq!(noted.first().map(Vec::len), Some(1), "noted as held while tuned");
        assert_eq!(noted.last(), Some(&Vec::new()), "and not once let go");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn asking_tunes_nothing_and_refusals_are_passed_on() {
        let stand = Stand::start().await;
        let (relay, _) = relay(Duration::from_secs(60));
        let address = relay.address("src", session(SourceKind::Emby, &stand.base), "ch2", None);
        let head = reqwest::Client::new().head(&address).send().await.unwrap();
        assert_eq!((head.status().as_u16(), head.headers()["content-type"].to_str().unwrap()), (200, "video/mp2t"));
        assert!(stand.seen().is_empty(), "nothing tuned to answer HEAD");
        let unknown = format!("{}/nothing/live.ts", address.split("/live.ts").next().unwrap().rsplit_once('/').unwrap().0);
        assert_eq!(reqwest::get(&unknown).await.unwrap().status().as_u16(), 404);

        stand.busy.store(true, Ordering::SeqCst);
        let refused = reqwest::get(&address).await.unwrap();
        assert_eq!(refused.status().as_u16(), 503);
        assert_eq!(refused.text().await.unwrap(), "The server couldn't open this channel. Its tuners may all be in use.");
        // A token the server no longer takes.
        let signed_out = relay.address("src", ServerSession { token: Some("old".into()), ..session(SourceKind::Emby, &stand.base) }, "ch2", None);
        assert_eq!(signed_out, address, "a new sign-in keeps the address");
        assert_eq!(reqwest::get(&address).await.unwrap().status().as_u16(), 403);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_tuning_the_server_dropped_is_made_again() {
        let stand = Stand::start().await;
        let (relay, _) = relay(Duration::from_secs(60));
        let address = relay.address("src", session(SourceKind::Jellyfin, &stand.base), "ch3", None);
        stand.ending.store(true, Ordering::SeqCst);
        let ended = read(&address, Duration::from_millis(300)).await;
        assert_eq!((ended.0, ended.1.len()), (200, 0));
        stand.ending.store(false, Ordering::SeqCst);
        let again = read(&address, Duration::from_millis(200)).await;
        assert!(!again.1.is_empty());
        assert_eq!(stand.count("POST /Items/ch3/PlaybackInfo"), 2, "tuned afresh: {:?}", stand.seen());
        assert_eq!(stand.count("POST /LiveStreams/Close"), 1, "the dropped one let go");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plex_channels_are_followed_kept_alive_and_let_go() {
        let stand = Stand::start().await;
        let (relay, notes) = relay(Duration::from_millis(250));
        let address = relay.address("src", session(SourceKind::Plex, &stand.base), "5.1", Some("2"));
        let (status, bytes) = read(&address, Duration::from_millis(2600)).await;
        assert_eq!(status, 200);
        let text = String::from_utf8_lossy(&bytes).to_string();
        let names: Vec<u64> = text.split('|').filter_map(|part| part.strip_prefix("segment ")?.strip_suffix(".ts")?.parse().ok()).collect();
        assert!(names.len() >= 3, "segments in turn: {text}");
        assert!(names.windows(2).all(|pair| pair[1] == pair[0] + 1), "each once, in order: {names:?}");

        let seen = stand.seen();
        assert_eq!(stand.count("POST /livetv/dvrs/2/channels/5.1/tune"), 1, "{seen:?}");
        let start = seen.iter().find(|line| line.contains("/video/:/transcode/universal/start.m3u8")).unwrap();
        assert!(start.contains("path=%2Flivetv%2Fsessions%2F6d0f3b52-1c4e-4a8b-9e27-5b3c9a1d0e44"), "{start}");
        assert!(start.contains("directStream=1") && start.contains("protocol=hls") && !start.contains(TOKEN));
        let pings = stand.count("GET /:/timeline");
        assert!(pings >= 2, "kept alive while watched: {seen:?}");

        settle().await;
        let after = stand.seen();
        let stopped = after.iter().filter(|line| line.starts_with("GET /:/timeline") && line.contains("state=stopped")).count();
        assert_eq!(stopped, 1, "{after:?}");
        assert_eq!(stand.count("GET /video/:/transcode/universal/stop"), 1);
        assert_eq!(stand.count("DELETE /media/grabbers/operations/5.1-xorviotestsession"), 1);
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(stand.count("GET /:/timeline"), after.iter().filter(|line| line.starts_with("GET /:/timeline")).count(), "no pings once let go");
        let noted = lock(&notes).clone();
        assert!(matches!(&noted[0][0].release, Release::PlexSession { grab: Some(grab), .. } if grab == "5.1-xorviotestsession"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn quitting_lets_everything_go_at_once() {
        let stand = Stand::start().await;
        let notes: Notes = Arc::default();
        let kept = notes.clone();
        let timing = Timing { linger: Duration::from_secs(3600), keepalive: Duration::from_secs(3600) };
        let relay = TunerRelay::start(Http::new().unwrap(), Arc::new(move |held| lock(&kept).push(held)), timing).unwrap();
        let address = relay.address("src", session(SourceKind::Jellyfin, &stand.base), "ch4", None);
        read(&address, Duration::from_millis(100)).await;
        assert_eq!(stand.count("POST /LiveStreams/Close"), 0, "lingering for an hour");
        relay.let_go_of_everything().await;
        assert_eq!(stand.count("POST /LiveStreams/Close"), 1);
        assert_eq!(lock(&notes).last(), Some(&Vec::new()));
    }
}
