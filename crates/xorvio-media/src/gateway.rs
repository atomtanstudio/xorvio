//! The media gateway: a small HTTP server on 127.0.0.1 that everything the
//! player opens goes through. It adds the provider's headers, rewrites HLS
//! playlists so their segments come through it too, answers CORS, and remuxes
//! what the web view can't play with ffmpeg.
//!
//! Gateway addresses look like `http://127.0.0.1:<port>/<token>/<kind>/<id>/<name>`.
//! The token is random per launch, so other pages can't use the gateway, and
//! the id is random per stream: the real address (which can hold a login)
//! stays in memory and never appears in a gateway address or a log.
//!
//! Other players on this computer (IINA, VLC, Infuse) get addresses of their
//! own ([`Gateway::external`]): the original stream passed through with the
//! provider's headers, kept apart from the player's so they last until
//! Xorvio quits.
//!
//! Live channels can also play from a buffer on this computer, so they can
//! be paused and rewound ([`Gateway::timeshift`], kind `l`): see the
//! timeshift module.

use std::collections::{HashMap, VecDeque};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{anyhow, bail};
use axum::Router;
use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::{Method, Request, Response, StatusCode};
use axum::routing::any;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, watch};
use tower::ServiceExt;
use url::Url;
use xorvio_model::{LiveStreamFormat, MediaTrack, PlaybackMode, ResolvedStream, StreamInfo, StreamTarget, TimeshiftInfo};

use crate::hls::{self, ChildKind};
use crate::probe::{Probe, probe};
use crate::process::{Input, is_safe_header};
use crate::remux::{self, RemuxPlan};
use crate::timeshift::{self, Session, Unready};
use crate::tools::Tools;

// Streams for a Chromecast or Google TV, on a listener of their own on the network.
pub(crate) mod cast;
// Thumbnails for the seek bar, made in the background and served from here.
pub(crate) mod thumbnails;

/// What the caller knows about a stream before it's opened.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayHint {
    /// The container's extension when the address doesn't show it ("mkv").
    pub extension: Option<String>,
    /// The source's preferred live transport.
    pub live_format: Option<LiveStreamFormat>,
    /// A known length, used when ffprobe can't tell.
    pub duration_seconds: Option<f64>,
    /// Play the audio track in this language when the file has one (see
    /// [`xorvio_model::language_code`]).
    #[serde(default)]
    pub audio_language: Option<String>,
}

/// Changes to a playing stream, which restart it where it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamOptions {
    /// Another audio track (an index from `ResolvedStream::audio_tracks`).
    pub audio_index: Option<i64>,
    /// Seconds to move the audio by; positive plays it later.
    pub audio_delay_seconds: Option<f64>,
    /// A picture subtitle track (PGS, VobSub, DVB) to draw into the video,
    /// which encodes it; a negative index stops drawing one.
    #[serde(default)]
    pub burn_subtitle: Option<i64>,
}

/// What the web view's Media Source Extensions decode, as the interface
/// reports it at launch. Remuxing copies these as they are and converts
/// the rest.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackSupport {
    pub hevc: bool,
    /// Dolby Digital.
    pub ac3: bool,
    /// Dolby Digital Plus.
    pub eac3: bool,
    /// The `<video>` element plays VP9 (in MP4 or WebM) by itself. Remuxing
    /// never copies it: MPEG-TS has no place for VP9.
    #[serde(default)]
    pub vp9: bool,
    /// The same for AV1, which WebKit decodes only where the hardware does.
    /// ffmpeg writes AV1 into MPEG-TS in a form nothing reads, so remuxing
    /// never copies it either.
    #[serde(default)]
    pub av1: bool,
}

impl Default for PlaybackSupport {
    /// What to assume before the interface has said: HEVC, which every
    /// platform Xorvio ships on decodes, and no Dolby audio, VP9 or AV1.
    fn default() -> Self {
        PlaybackSupport {
            hevc: true,
            ac3: false,
            eac3: false,
            vp9: false,
            av1: false,
        }
    }
}

/// Streams kept registered at once; the oldest go first. Generous, since
/// the interface may not release every stream (multiview has several).
const MAX_STREAMS: usize = 64;
/// Addresses handed to other players, kept at once; the oldest go first.
/// Counted apart from the player's streams, so zapping doesn't end them.
const MAX_EXTERNAL: usize = 16;
/// Segment addresses remembered per HLS stream (playlists are never forgotten).
const MAX_SEGMENTS: usize = 4096;
/// Playlists bigger than this aren't playlists.
const MAX_PLAYLIST_BYTES: usize = 8 * 1024 * 1024;
const SNIFF_TIMEOUT: Duration = Duration::from_secs(8);
/// A transport-stream file written to this recently is a recording still being made.
const GROWING_WINDOW: Duration = Duration::from_secs(30);
const PLAYLIST_TIMEOUT: Duration = Duration::from_secs(15);
/// Live buffers at once (each is an ffmpeg, a provider connection and up to
/// a few gigabytes on disk); starting another stops the oldest.
const MAX_TIMESHIFTS: usize = 4;
/// How long starting a buffer waits for its first segment: a keyframe, then
/// a couple of seconds of stream (more for channels with long GOPs).
const START_WAIT: Duration = Duration::from_secs(15);
/// How long a buffer's playlist request waits for its first segment (when
/// it comes before there is one).
const FIRST_SEGMENT_WAIT: Duration = Duration::from_secs(10);
/// A buffer's window, however it's asked for: ten seconds to four hours.
const WINDOW_RANGE: (f64, f64) = (10.0, 4.0 * 3600.0);
/// After a stream playing directly is released, a moment for its
/// connection to close before the buffer opens its own.
const SETTLE: Duration = Duration::from_millis(300);

const FFMPEG_MISSING: &str = "This video needs ffmpeg to play, and Xorvio couldn't find it.";
const STREAM_GONE: &str = "That stream isn't playing any more.";

/// The running gateway. Shared as `Arc<Gateway>`; it stops when dropped.
pub struct Gateway {
    shared: Arc<Shared>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
}

struct Shared {
    token: String,
    origin: String,
    http: reqwest::Client,
    /// The same, keeping no connections for later, for thumbnails' feeds:
    /// a kept one lets the next request out the moment the last closes,
    /// and the pool sometimes opens a spare beside the one in use.
    feed_http: reqwest::Client,
    tools: Option<Tools>,
    support: Mutex<PlaybackSupport>,
    streams: Mutex<Streams>,
    /// Remuxed streams' subtitles, as WebVTT files in a folder per stream.
    subtitle_root: PathBuf,
    /// Live buffers, a folder per session.
    timeshift_root: PathBuf,
    /// What's cast to a TV, and the listener it's served from.
    cast: cast::Casting,
    /// Seek-bar thumbnails: where they're kept and the title being made.
    previews: thumbnails::Previews,
}

#[derive(Default)]
struct Streams {
    by_id: HashMap<String, Arc<Entry>>,
    order: VecDeque<String>,
    /// Streams handed to other players, oldest first.
    external: VecDeque<String>,
}

/// A registered stream.
struct Entry {
    target: StreamTarget,
    mode: PlaybackMode,
    /// The container's extension, as far as it's known ("mkv").
    extension: Option<String>,
    duration: Option<f64>,
    /// Where a remuxed stream starts, in seconds.
    offset: f64,
    remux: Option<RemuxPlan>,
    /// What ffprobe found, for the tracks the player lists.
    probe: Option<Probe>,
    audio_delay: f64,
    children: Mutex<Children>,
    /// Moving this on stops the ffmpeg serving the stream: a newer request,
    /// a seek or a release does that. It also ends a live stream passed
    /// through, so its provider connection closes when it's released.
    stop: watch::Sender<u64>,
    /// The live buffer this stream plays from.
    timeshift: Option<Arc<Session>>,
    /// The last error status the player got for this stream (0 for none):
    /// the provider's own, passed through, or the gateway's when it couldn't
    /// reach it. The web view's players don't say, so the interface asks
    /// ([`Gateway::failure_status`]) to tell a refused login from a drop.
    failure: AtomicU16,
    /// For a thumbnails job's own way to a remote title: one request to the
    /// provider at a time, a new one ending the one before first (ffmpeg
    /// opens the next before closing the last when it seeks).
    feed: Option<Arc<tokio::sync::Semaphore>>,
}

impl Entry {
    fn new(
        target: StreamTarget,
        mode: PlaybackMode,
        duration: Option<f64>,
        offset: f64,
        remux: Option<RemuxPlan>,
    ) -> Entry {
        Entry {
            target,
            mode,
            extension: None,
            duration,
            offset,
            remux,
            probe: None,
            audio_delay: 0.0,
            children: Mutex::default(),
            stop: watch::channel(0).0,
            timeshift: None,
            failure: AtomicU16::new(0),
            feed: None,
        }
    }

    fn with_probe(self, probe: Option<Probe>, audio_delay: f64) -> Entry {
        Entry {
            probe,
            audio_delay,
            ..self
        }
    }

    fn with_extension(self, extension: Option<String>) -> Entry {
        Entry { extension, ..self }
    }

    fn with_timeshift(self, session: Arc<Session>) -> Entry {
        Entry {
            timeshift: Some(session),
            ..self
        }
    }

    fn halt(&self) {
        self.stop.send_modify(|generation| *generation += 1);
        if let Some(session) = &self.timeshift {
            session.stop();
        }
    }

    /// Ends when the entry is next halted (or dropped).
    fn halted(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut stop = self.stop.subscribe();
        let generation = *stop.borrow();
        async move {
            let _ = stop.wait_for(|current| *current != generation).await;
        }
    }
}

/// Addresses found in an HLS stream's playlists, by the number the gateway
/// gave them.
#[derive(Default)]
struct Children {
    by_url: HashMap<String, u64>,
    by_id: HashMap<u64, Child>,
    segments: VecDeque<u64>,
    /// Playlists it lists, oldest first.
    others: VecDeque<u64>,
    next: u64,
}

#[derive(Clone)]
struct Child {
    url: Url,
    kind: ChildKind,
}

impl Children {
    fn register(&mut self, url: Url, kind: ChildKind) -> u64 {
        if let Some(&n) = self.by_url.get(url.as_str()) {
            return n;
        }
        let n = self.next;
        self.next += 1;
        self.by_url.insert(url.to_string(), n);
        self.by_id.insert(n, Child { url, kind });
        // A live stream lists new segments forever (and a hostile one could
        // list new playlists forever); the oldest are dropped.
        let kept = if kind == ChildKind::Media { &mut self.segments } else { &mut self.others };
        kept.push_back(n);
        while kept.len() > MAX_SEGMENTS {
            if let Some(old) = kept.pop_front()
                && let Some(child) = self.by_id.remove(&old)
            {
                self.by_url.remove(child.url.as_str());
            }
        }
        n
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Gateway {
    /// Binds 127.0.0.1 on a free port and starts serving.
    pub async fn start(tools: Option<Tools>) -> anyhow::Result<Arc<Gateway>> {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let client = || {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .user_agent(concat!("Xorvio/", env!("CARGO_PKG_VERSION")))
                // Media is passed through byte for byte; ranges need the real bytes.
                .no_gzip()
        };
        let shared = Arc::new(Shared {
            token: uuid::Uuid::new_v4().simple().to_string(),
            origin: format!("http://127.0.0.1:{port}"),
            http: client().build()?,
            feed_http: client().pool_max_idle_per_host(0).build()?,
            tools,
            support: Mutex::default(),
            streams: Mutex::default(),
            subtitle_root: subtitle_root(),
            timeshift_root: timeshift::root(),
            cast: cast::Casting::default(),
            previews: thumbnails::Previews::default(),
        });
        // Left from a previous run.
        let _ = std::fs::remove_dir_all(&shared.subtitle_root);
        timeshift::stop_orphans(&shared.timeshift_root);
        timeshift::sweep(&shared.timeshift_root, &[]);
        let app = Router::new()
            .route("/{token}/{kind}/{id}/{name}", any(handle))
            .with_state(shared.clone());
        let (shutdown, stopped) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let server = axum::serve(listener, app).with_graceful_shutdown(async {
                let _ = stopped.await;
            });
            if let Err(error) = server.await {
                tracing::error!("the media gateway stopped: {error}");
            }
        });
        Ok(Arc::new(Gateway {
            shared,
            shutdown: Mutex::new(Some(shutdown)),
        }))
    }

    /// Registers a stream and says how to play it: HLS for playlists,
    /// MPEG-TS for live transport streams, the `<video>` element for MP4 and
    /// WebM it can decode, and an ffmpeg remux into MPEG-TS (played with
    /// mpegts.js) for everything else.
    /// Records what the web view decodes, for the streams opened after.
    pub fn set_support(&self, support: PlaybackSupport) {
        tracing::info!(?support, "playback support");
        *self.shared.support.lock().unwrap() = support;
    }

    pub async fn prepare(
        &self,
        target: StreamTarget,
        hint: PlayHint,
    ) -> anyhow::Result<ResolvedStream> {
        let input =
            Input::of(&target).ok_or_else(|| anyhow!("Xorvio can't open this kind of address."))?;
        let extension = hint
            .extension
            .as_deref()
            .map(clean_extension)
            .filter(|e| !e.is_empty())
            .or_else(|| extension_of(&target.url));
        let choice = self
            .shared
            .choose(&target, &input, extension.as_deref(), &hint)
            .await?;
        let is_live = target.is_live;
        let (kind, name) = match choice.mode {
            PlaybackMode::Hls => ("h", "index.m3u8".to_string()),
            PlaybackMode::Remux => ("r", "video.ts".to_string()),
            PlaybackMode::Mpegts => (
                "s",
                format!(
                    "stream.{}",
                    extension.as_deref().filter(|e| *e == "flv").unwrap_or("ts")
                ),
            ),
            PlaybackMode::Native => (
                "s",
                format!("stream.{}", extension.as_deref().unwrap_or("mp4")),
            ),
        };
        let duration = if is_live { None } else { choice.duration };
        let entry = Entry::new(target, choice.mode, duration, 0.0, choice.remux)
            .with_probe(choice.probe, 0.0)
            .with_extension(extension);
        let id = self.shared.register(entry);
        Ok(self.shared.resolved(&id, kind, &name))
    }

    /// A remuxed stream from `offset_seconds` in, under a fresh address. The
    /// old address stops (and its ffmpeg with it).
    pub async fn seek(
        &self,
        stream_url: &str,
        offset_seconds: f64,
    ) -> anyhow::Result<ResolvedStream> {
        let id = self
            .shared
            .id_in(stream_url)
            .ok_or_else(|| anyhow!(STREAM_GONE))?;
        let entry = self.shared.entry(&id).ok_or_else(|| anyhow!(STREAM_GONE))?;
        if entry.mode != PlaybackMode::Remux || entry.target.is_live {
            bail!("Only remuxed films and episodes seek by restarting.");
        }
        self.restream(stream_url, offset_seconds, StreamOptions::default()).await
    }

    /// The stream again from `offset_seconds` (ignored for live streams), with
    /// another audio track or the audio moved, under a fresh address. ffmpeg
    /// does both, so a stream the web view was playing directly is remuxed
    /// from here on. The old address stops.
    pub async fn restream(
        &self,
        stream_url: &str,
        offset_seconds: f64,
        options: StreamOptions,
    ) -> anyhow::Result<ResolvedStream> {
        let id = self
            .shared
            .id_in(stream_url)
            .ok_or_else(|| anyhow!(STREAM_GONE))?;
        let entry = self.shared.entry(&id).ok_or_else(|| anyhow!(STREAM_GONE))?;
        if self.shared.tools.is_none() {
            bail!(FFMPEG_MISSING);
        }
        let live = entry.target.is_live;
        let mut offset = if live || !offset_seconds.is_finite() {
            0.0
        } else {
            offset_seconds.max(0.0)
        };
        if let Some(duration) = entry.duration {
            offset = offset.min((duration - 1.0).max(0.0));
        }
        let mut probe = entry.probe.clone();
        if probe.is_none() && !live && entry.mode != PlaybackMode::Remux {
            probe = self.shared.probe(&entry.target).await;
        }
        if let (Some(index), Some(found)) = (options.audio_index, &probe) {
            probe = Some(found.with_audio(index));
        }
        let delay = options.audio_delay_seconds.unwrap_or(entry.audio_delay).clamp(-10.0, 10.0);
        let burn = match options.burn_subtitle {
            Some(index) => (index >= 0).then_some(index),
            None => entry.remux.as_ref().and_then(RemuxPlan::burned_subtitle),
        };
        let plan = match (&entry.remux, options.audio_index, options.burn_subtitle) {
            // Same tracks: keep the plan (a growing recording keeps following).
            (Some(plan), None, None) => plan.clone(),
            _ => {
                let fresh = RemuxPlan::new(probe.as_ref(), self.shared.support()).burning(burn, probe.as_ref());
                if entry.remux.as_ref().is_some_and(RemuxPlan::is_following) { fresh.following() } else { fresh }
            }
        }
        .with_audio_delay(delay);
        let fresh = Entry::new(entry.target.clone(), PlaybackMode::Remux, entry.duration, offset, Some(plan))
            .with_probe(probe, delay)
            .with_extension(entry.extension.clone());
        let new_id = self.shared.register(fresh);
        self.shared.release(&id);
        Ok(self.shared.resolved(&new_id, "r", "video.ts"))
    }

    /// Where the page that plays a YouTube trailer is, for this video; `None`
    /// for anything that isn't a YouTube video id.
    pub fn trailer_page(&self, youtube_id: &str) -> Option<String> {
        let valid = (6..=20).contains(&youtube_id.len()) && youtube_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        valid.then(|| format!("{}/{}/y/trailer/player.html?v={youtube_id}", self.shared.origin, self.shared.token))
    }

    /// The last error status the player was answered with for a stream (a
    /// provider's 403 or 404, or 502 when the gateway couldn't reach it), so
    /// a failure can be put into words. None while all is well.
    pub fn failure_status(&self, stream_url: &str) -> Option<u16> {
        let entry = self.shared.entry(&self.shared.id_in(stream_url)?)?;
        Some(entry.failure.load(Ordering::Relaxed)).filter(|status| *status != 0)
    }

    /// Forgets a stream and stops any ffmpeg serving it.
    pub fn release(&self, stream_url: &str) {
        if let Some(id) = self.shared.id_in(stream_url) {
            self.shared.release(&id);
        }
    }

    /// Releases every stream, for when Xorvio quits. A buffer's ffmpeg reads
    /// the provider and writes files by itself, so nothing else would stop
    /// it; it's stopped here and now, since the tasks that would otherwise
    /// don't run again.
    pub fn release_all(&self) {
        let entries: Vec<Arc<Entry>> = {
            let mut streams = lock(&self.shared.streams);
            streams.order.clear();
            streams.external.clear();
            streams.by_id.drain().map(|(_, entry)| entry).collect()
        };
        for entry in entries {
            entry.halt();
            if let Some(session) = &entry.timeshift {
                session.stop_now();
            }
        }
        let _ = std::fs::remove_dir_all(&self.shared.subtitle_root);
    }

    /// Plays a live channel from a buffer on this computer, so it can be
    /// paused and rewound: ffmpeg writes the last `window_seconds` as HLS
    /// into a temporary folder, played as live HLS. The provider sees one
    /// connection. Returns once the first segment is written (two or three
    /// seconds for most channels), or with why the channel couldn't be buffered.
    pub async fn timeshift(
        &self,
        target: StreamTarget,
        hint: PlayHint,
        window_seconds: f64,
    ) -> anyhow::Result<ResolvedStream> {
        let Some(tools) = &self.shared.tools else { bail!(FFMPEG_MISSING) };
        if !target.is_live {
            bail!("Only live channels can be paused and rewound.");
        }
        let window = if window_seconds.is_finite() {
            window_seconds.clamp(WINDOW_RANGE.0, WINDOW_RANGE.1)
        } else {
            1800.0
        };
        // A provider allowing one connection would refuse this one while an
        // older buffer of the same channel holds it; and buffers past the
        // limit make room before this one opens.
        self.shared.release_timeshifts(|entry| entry.target.url == target.url);
        self.shared.limit_timeshifts(MAX_TIMESHIFTS - 1);
        timeshift::sweep(&self.shared.timeshift_root, &self.shared.timeshift_dirs());
        let session = Session::start(
            &self.shared.http,
            &tools.ffmpeg,
            &self.shared.timeshift_root,
            &target,
            self.shared.support(),
            window,
        )
        .await?;
        let extension = hint
            .extension
            .as_deref()
            .map(clean_extension)
            .filter(|e| !e.is_empty())
            .or_else(|| extension_of(&target.url));
        let entry = Entry::new(target, PlaybackMode::Hls, None, 0.0, None)
            .with_extension(extension)
            .with_timeshift(session.clone());
        let id = self.shared.register(entry);
        // The player's first playlist request is then answered at once:
        // AVFoundation (WebKit's HLS player on macOS) can stall for good
        // when that request is held while an encoded stream's first segment
        // is written.
        if let Err(unready) = session.wait_ready(START_WAIT).await {
            self.shared.release(&id);
            bail!("{}", unready.message());
        }
        Ok(self.shared.resolved(&id, "l", timeshift::PLAYLIST))
    }

    /// Switches a live channel playing directly to a buffer that starts now
    /// (when someone pauses it). The direct stream is released first, so the
    /// provider never sees two connections; if the buffer can't start, the
    /// channel has to be played again. A stream already buffered comes back
    /// as it is.
    pub async fn timeshift_playing(
        &self,
        stream_url: &str,
        window_seconds: f64,
    ) -> anyhow::Result<ResolvedStream> {
        let id = self
            .shared
            .id_in(stream_url)
            .ok_or_else(|| anyhow!(STREAM_GONE))?;
        let entry = self.shared.entry(&id).ok_or_else(|| anyhow!(STREAM_GONE))?;
        if entry.timeshift.is_some() {
            return Ok(self.shared.resolved(&id, "l", timeshift::PLAYLIST));
        }
        if !entry.target.is_live {
            bail!("Only live channels can be paused and rewound.");
        }
        let hint = PlayHint {
            extension: entry.extension.clone(),
            live_format: match entry.mode {
                PlaybackMode::Hls => Some(LiveStreamFormat::Hls),
                PlaybackMode::Mpegts => Some(LiveStreamFormat::Mpegts),
                _ => None,
            },
            ..PlayHint::default()
        };
        self.shared.release(&id);
        // HLS played directly holds no connection between segments; the others do.
        if entry.mode != PlaybackMode::Hls {
            tokio::time::sleep(SETTLE).await;
        }
        self.timeshift(entry.target.clone(), hint, window_seconds).await
    }

    /// The folder a buffered stream is written to, for tests.
    #[doc(hidden)]
    pub fn timeshift_folder(&self, stream_url: &str) -> Option<PathBuf> {
        let entry = self.shared.entry(&self.shared.id_in(stream_url)?)?;
        entry.timeshift.as_ref().map(|session| session.dir().to_path_buf())
    }

    /// An address another player on this computer (IINA, VLC, Infuse) can
    /// open by itself for the stream playing at `stream_url`: its original
    /// target, never the remux, since those players read MKV, Dolby audio
    /// and the rest themselves. It passes the bytes through with the
    /// provider's headers, ranges included, and rewrites HLS playlists as
    /// the player's own address does. It isn't released with the player's
    /// stream, only when Xorvio quits or many more have been handed out.
    ///
    /// `title` names the file at the end of the address, which the other
    /// player shows (and Infuse looks the title up by).
    pub fn external(&self, stream_url: &str, title: &str) -> anyhow::Result<String> {
        let id = self
            .shared
            .id_in(stream_url)
            .ok_or_else(|| anyhow!(STREAM_GONE))?;
        let entry = self.shared.entry(&id).ok_or_else(|| anyhow!(STREAM_GONE))?;
        // A buffered channel goes as the provider sends it, not as its buffer.
        let (playlist, mode) = match &entry.timeshift {
            Some(session) if session.is_hls_source() => (true, PlaybackMode::Hls),
            Some(_) => (false, PlaybackMode::Mpegts),
            None => (entry.mode == PlaybackMode::Hls, entry.mode),
        };
        let extension = entry.extension.clone().unwrap_or_else(|| fallback_extension(&entry.target, mode).into());
        Ok(self.shared.external(entry.target.clone(), playlist, extension, title))
    }

    /// The same for a stream Xorvio isn't playing (from a title's or a
    /// channel's menu). It isn't opened first, so its address and the hint
    /// decide whether it's an HLS playlist.
    pub fn external_target(
        &self,
        target: StreamTarget,
        hint: &PlayHint,
        title: &str,
    ) -> anyhow::Result<String> {
        Input::of(&target).ok_or_else(|| anyhow!("Xorvio can't open this kind of address."))?;
        let extension = hint
            .extension
            .as_deref()
            .map(clean_extension)
            .filter(|e| !e.is_empty())
            .or_else(|| extension_of(&target.url));
        let playlist = is_playlist(&target, extension.as_deref(), hint.live_format);
        let extension = extension.unwrap_or_else(|| fallback_extension(&target, PlaybackMode::Native).into());
        Ok(self.shared.external(target, playlist, extension, title))
    }
}

/// Whether a stream that hasn't been opened plays as an HLS playlist. A live
/// stream that says nothing about itself goes as one: the playlist route
/// hands over anything that turns out not to be a playlist as it comes.
fn is_playlist(target: &StreamTarget, extension: Option<&str>, live_format: Option<LiveStreamFormat>) -> bool {
    match extension {
        Some("m3u8" | "m3u") => true,
        Some("ts" | "mts" | "mpegts" | "flv") => false,
        _ if !target.is_live => false,
        _ => live_format != Some(LiveStreamFormat::Mpegts),
    }
}

/// An extension for a stream whose address doesn't show one.
fn fallback_extension(target: &StreamTarget, mode: PlaybackMode) -> &'static str {
    match mode {
        _ if target.is_live => "ts",
        PlaybackMode::Native => "mp4",
        PlaybackMode::Mpegts => "ts",
        PlaybackMode::Hls => "m3u8",
        PlaybackMode::Remux => "mkv",
    }
}

/// "Severance - S01E03.mkv", made safe for an address: no slashes or other
/// characters file systems refuse, and percent-encoded.
fn file_name(title: &str, extension: &str) -> String {
    let plain: String = title
        .chars()
        .map(|c| if c.is_control() || "/\\:*?\"<>|#%".contains(c) { ' ' } else { c })
        .collect();
    let plain: String = plain.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(100).collect();
    let stem = match plain.trim_end_matches('.').trim() {
        "" => "Video",
        stem => stem,
    };
    let mut encoded = String::new();
    for byte in format!("{stem}.{extension}").bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~()".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let streams = std::mem::take(&mut *lock(&self.shared.streams));
        for entry in streams.by_id.values() {
            entry.halt();
        }
        if let Some(shutdown) = lock(&self.shutdown).take() {
            let _ = shutdown.send(());
        }
    }
}

struct Choice {
    mode: PlaybackMode,
    duration: Option<f64>,
    remux: Option<RemuxPlan>,
    probe: Option<Probe>,
}

impl Choice {
    fn plain(mode: PlaybackMode) -> Choice {
        Choice {
            mode,
            duration: None,
            remux: None,
            probe: None,
        }
    }
}

enum Sniffed {
    Playlist,
    Transport,
}

impl Shared {
    async fn choose(
        &self,
        target: &StreamTarget,
        input: &Input,
        ext: Option<&str>,
        hint: &PlayHint,
    ) -> anyhow::Result<Choice> {
        if matches!(ext, Some("m3u8" | "m3u")) {
            return Ok(Choice::plain(PlaybackMode::Hls));
        }
        // A recording still being made: ffmpeg follows the file as it grows.
        // Its length isn't known yet.
        if let Input::File(path) = input
            && self.tools.is_some()
            && (target.is_live || is_growing_transport(path, ext))
        {
            let plan = RemuxPlan::new(self.probe(target).await.as_ref(), self.support()).following();
            return Ok(Choice {
                mode: PlaybackMode::Remux,
                duration: None,
                remux: Some(plan),
                probe: None,
            });
        }
        if target.is_live {
            if matches!(ext, Some("ts" | "mts" | "mpegts" | "flv")) {
                return Ok(Choice::plain(PlaybackMode::Mpegts));
            }
            match hint.live_format {
                Some(LiveStreamFormat::Hls) => return Ok(Choice::plain(PlaybackMode::Hls)),
                Some(LiveStreamFormat::Mpegts) => return Ok(Choice::plain(PlaybackMode::Mpegts)),
                None => {}
            }
            if let Input::Web(url) = input {
                match self.sniff(target, url.clone()).await {
                    Some(Sniffed::Playlist) => return Ok(Choice::plain(PlaybackMode::Hls)),
                    Some(Sniffed::Transport) => return Ok(Choice::plain(PlaybackMode::Mpegts)),
                    None => {}
                }
            }
            // Something else that's live: ffmpeg turns it into MPEG-TS when it can.
            return Ok(match self.tools {
                Some(_) => Choice {
                    mode: PlaybackMode::Remux,
                    duration: None,
                    remux: Some(RemuxPlan::new(None, self.support())),
                    probe: None,
                },
                None => Choice::plain(PlaybackMode::Mpegts),
            });
        }

        let mut probed = self.probe(target).await;
        // The preferred language's audio, when the file has it.
        if let Some(probe) = &mut probed
            && let Some(language) = hint.audio_language.as_deref()
            && probe.audio.as_ref().is_none_or(|a| a.language.as_deref() != Some(language))
            && let Some(index) = probe.audio_tracks.iter().find(|a| a.language.as_deref() == Some(language)).map(|a| a.index)
        {
            *probe = probe.with_audio(index);
        }
        let duration = probed
            .as_ref()
            .and_then(|p| p.duration)
            .or(hint.duration_seconds);
        let native = Choice {
            mode: PlaybackMode::Native,
            duration,
            remux: None,
            probe: probed.clone(),
        };
        if let Some(probe) = &probed {
            if probe.is_hls() {
                return Ok(Choice::plain(PlaybackMode::Hls));
            }
            if probe.plays_natively(self.support()) {
                return Ok(native);
            }
            return Ok(Choice {
                mode: PlaybackMode::Remux,
                duration,
                remux: Some(RemuxPlan::new(Some(probe), self.support())),
                probe: probed.clone(),
            });
        }
        // Not probed: go by the extension.
        let web_container = matches!(ext, Some("mp4" | "m4v" | "mov" | "webm"));
        match &self.tools {
            _ if web_container => Ok(native),
            Some(_) => Ok(Choice {
                mode: PlaybackMode::Remux,
                duration,
                remux: Some(RemuxPlan::new(None, self.support())),
                probe: None,
            }),
            None if ext.is_none() => Ok(native),
            None => bail!(FFMPEG_MISSING),
        }
    }

    fn support(&self) -> PlaybackSupport {
        *self.support.lock().unwrap()
    }

    async fn probe(&self, target: &StreamTarget) -> Option<Probe> {
        let tools = self.tools.as_ref()?;
        let started = std::time::Instant::now();
        match probe(&tools.ffprobe, target).await {
            Ok(probe) => {
                let seconds = started.elapsed().as_secs_f32();
                tracing::info!("probed in {seconds:.1}s: {}", probe.describe());
                Some(probe)
            }
            Err(error) => {
                tracing::info!("ffprobe couldn't read a stream: {error:#}");
                None
            }
        }
    }

    /// Reads the first bytes of a live stream whose address says nothing
    /// about its format.
    async fn sniff(&self, target: &StreamTarget, url: Url) -> Option<Sniffed> {
        let read = async {
            let mut response = self
                .upstream(reqwest::Method::GET, url, target)
                .send()
                .await
                .ok()?;
            if !response.status().is_success() {
                return None;
            }
            let mut head = Vec::new();
            while head.len() < 16 {
                match response.chunk().await.ok()? {
                    Some(chunk) => head.extend_from_slice(&chunk),
                    None => break,
                }
            }
            Some(head)
        };
        let head = tokio::time::timeout(SNIFF_TIMEOUT, read).await.ok()??;
        if hls::looks_like_playlist(&head) {
            Some(Sniffed::Playlist)
        } else if head.first() == Some(&0x47) || head.starts_with(b"FLV") {
            Some(Sniffed::Transport)
        } else {
            None
        }
    }

    fn upstream(
        &self,
        method: reqwest::Method,
        url: Url,
        target: &StreamTarget,
    ) -> reqwest::RequestBuilder {
        upstream(&self.http, method, url, target)
    }

    fn register(&self, entry: Entry) -> String {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut streams = lock(&self.streams);
        streams.by_id.insert(id.clone(), Arc::new(entry));
        streams.order.push_back(id.clone());
        while streams.order.len() > MAX_STREAMS {
            if let Some(old) = streams.order.pop_front()
                && let Some(entry) = streams.by_id.remove(&old)
            {
                entry.halt();
            }
        }
        id
    }

    fn release(&self, id: &str) {
        let mut streams = lock(&self.streams);
        if let Some(entry) = streams.by_id.remove(id) {
            entry.halt();
        }
        streams.order.retain(|other| other != id);
        streams.external.retain(|other| other != id);
        let _ = std::fs::remove_dir_all(self.subtitle_root.join(id));
    }

    /// Releases the buffered streams `which` picks.
    fn release_timeshifts(&self, which: impl Fn(&Entry) -> bool) {
        let ids: Vec<String> = lock(&self.streams)
            .by_id
            .iter()
            .filter(|(_, entry)| entry.timeshift.is_some() && which(entry))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.release(&id);
        }
    }

    /// Releases the oldest buffered streams until at most `keep` are left.
    fn limit_timeshifts(&self, keep: usize) {
        let ids: Vec<String> = {
            let streams = lock(&self.streams);
            streams
                .order
                .iter()
                .filter(|id| streams.by_id.get(*id).is_some_and(|e| e.timeshift.is_some()))
                .cloned()
                .collect()
        };
        for id in ids.iter().take(ids.len().saturating_sub(keep)) {
            tracing::info!("stopping the oldest live buffer to make room for another");
            self.release(id);
        }
    }

    /// The folders of this gateway's live buffers.
    fn timeshift_dirs(&self) -> Vec<PathBuf> {
        lock(&self.streams)
            .by_id
            .values()
            .filter_map(|entry| entry.timeshift.as_ref().map(|s| s.dir().to_path_buf()))
            .collect()
    }

    /// Registers a stream for another player and gives its address: the
    /// playlist route for HLS, the pass-through for everything else.
    fn external(&self, target: StreamTarget, playlist: bool, extension: String, title: &str) -> String {
        let mode = if playlist { PlaybackMode::Hls } else { PlaybackMode::Native };
        let name = if playlist { "index.m3u8".to_string() } else { file_name(title, &extension) };
        let entry = Entry::new(target, mode, None, 0.0, None).with_extension(Some(extension));
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut streams = lock(&self.streams);
        streams.by_id.insert(id.clone(), Arc::new(entry));
        streams.external.push_back(id.clone());
        while streams.external.len() > MAX_EXTERNAL {
            if let Some(old) = streams.external.pop_front()
                && let Some(entry) = streams.by_id.remove(&old)
            {
                entry.halt();
            }
        }
        drop(streams);
        self.address(if playlist { "h" } else { "s" }, &id, &name)
    }

    /// What the player gets for a registered stream: its address, and for a
    /// remuxed one, its tracks and where its subtitles are served.
    fn resolved(&self, id: &str, kind: &str, name: &str) -> ResolvedStream {
        let entry = self.entry(id);
        let entry = entry.as_deref();
        let remuxed = entry.filter(|e| e.mode == PlaybackMode::Remux);
        let probe = remuxed.and_then(|e| e.probe.as_ref());
        let plan = remuxed.and_then(|e| e.remux.as_ref());
        let audio_tracks = probe
            .map(|p| {
                p.audio_tracks
                    .iter()
                    .map(|a| MediaTrack {
                        index: a.index,
                        codec: a.codec.clone(),
                        language: a.language.clone(),
                        title: a.title.clone(),
                        channels: a.channels,
                        default: a.default,
                        forced: false,
                        url: None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let subtitle_tracks = probe
            .map(|p| {
                p.subtitles
                    .iter()
                    .map(|t| MediaTrack {
                        index: t.index,
                        codec: t.codec.clone(),
                        language: t.language.clone(),
                        title: t.title.clone(),
                        channels: None,
                        default: t.default,
                        forced: t.forced,
                        url: plan
                            .filter(|plan| plan.has_subtitles(t.index))
                            .map(|_| self.address("t", id, &format!("{}.vtt", t.index))),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let session = entry.and_then(|e| e.timeshift.as_ref());
        let info = entry
            .and_then(|e| e.probe.as_ref())
            .map(|p| StreamInfo {
                notes: plan.map(RemuxPlan::notes).unwrap_or_default(),
                ..p.info()
            })
            .or_else(|| session.map(|s| s.info()));
        ResolvedStream {
            url: self.address(kind, id, name),
            mode: entry.map_or(PlaybackMode::Native, |e| e.mode),
            is_live: entry.is_some_and(|e| e.target.is_live),
            duration_seconds: entry.and_then(|e| e.duration),
            start_offset_seconds: entry.map_or(0.0, |e| e.offset),
            audio_tracks,
            subtitle_tracks,
            audio_index: probe.and_then(|p| p.audio.as_ref()).map(|a| a.index),
            audio_delay_seconds: entry.map_or(0.0, |e| e.audio_delay),
            burned_subtitle: plan.and_then(RemuxPlan::burned_subtitle),
            info,
            timeshift: session.map(|s| TimeshiftInfo {
                window_seconds: s.window(),
            }),
        }
    }

    fn entry(&self, id: &str) -> Option<Arc<Entry>> {
        lock(&self.streams).by_id.get(id).cloned()
    }

    fn address(&self, kind: &str, id: &str, name: &str) -> String {
        format!("{}/{}/{kind}/{id}/{name}", self.origin, self.token)
    }

    /// The stream id in one of this gateway's addresses.
    fn id_in(&self, stream_url: &str) -> Option<String> {
        let url = Url::parse(stream_url).ok()?;
        let mut parts = url.path_segments()?;
        (parts.next()? == self.token).then_some(())?;
        parts.next()?;
        parts.next().map(String::from)
    }
}

/// A request to the provider with the target's user agent and headers.
pub(crate) fn upstream(
    http: &reqwest::Client,
    method: reqwest::Method,
    url: Url,
    target: &StreamTarget,
) -> reqwest::RequestBuilder {
    let mut request = http.request(method, url);
    if let Some(agent) = target
        .user_agent
        .as_deref()
        .and_then(|a| HeaderValue::from_str(a).ok())
    {
        request = request.header(header::USER_AGENT, agent);
    }
    for (name, value) in &target.headers {
        if !is_safe_header(name, value) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            request = request.header(name, value);
        }
    }
    request
}

// MARK: Serving

async fn handle(
    State(shared): State<Arc<Shared>>,
    UrlPath((token, kind, id, name)): UrlPath<(String, String, String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Response<Body> {
    // Paths hold only random ids, never the stream's address, so they're safe to log.
    tracing::debug!(%method, %kind, %name, range = ?headers.get(header::RANGE), "gateway request");
    if token != shared.token {
        return plain(StatusCode::NOT_FOUND, "Not found");
    }
    if method == Method::OPTIONS {
        return cors(
            Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .unwrap_or_default(),
        );
    }
    if method != Method::GET && method != Method::HEAD {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "Only GET and HEAD");
    }
    if kind == "y" {
        return trailer_page();
    }
    // Thumbnails outlast the stream they were made for: they're found by
    // the title's key.
    if kind == "p" {
        return cors(thumbnails::serve(&shared, &id, &name, &method, &headers).await);
    }
    let Some(entry) = shared.entry(&id) else {
        return plain(StatusCode::NOT_FOUND, STREAM_GONE);
    };
    let response = match kind.as_str() {
        "s" if entry.feed.is_some() => thumbnails::feed(&shared, &entry, &method, &headers).await,
        "s" => pass_through(&shared.http, &entry.target, None, &method, &headers, Some(entry.halted())).await,
        "h" => serve_hls(&shared, &id, &entry, &name, &method, &headers).await,
        "r" => serve_remux(&shared, &id, &entry, &method),
        "t" => serve_subtitles(&shared, &id, &entry, &name, &headers).await,
        "l" => serve_timeshift(&entry, &name, &method, &headers).await,
        _ => plain(StatusCode::NOT_FOUND, "Not found"),
    };
    // Kept for the interface, which can't see the status the web view got.
    // The latest answer counts: a stream that came back is fine again. A
    // range past the end is only a player looking.
    let status = response.status();
    if matches!(kind.as_str(), "s" | "h" | "l") && status != StatusCode::RANGE_NOT_SATISFIABLE {
        let failed = status.is_client_error() || status.is_server_error();
        entry.failure.store(if failed { status.as_u16() } else { 0 }, Ordering::Relaxed);
    }
    cors(response)
}

/// The page that plays a title's trailer behind its page (trailer.html).
fn trailer_page() -> Response<Body> {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        // YouTube hears only the gateway's origin, never the token in the path.
        .header(header::REFERRER_POLICY, "strict-origin-when-cross-origin")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(TRAILER_PAGE))
        .unwrap_or_default()
}

const TRAILER_PAGE: &str = include_str!("gateway/trailer.html");

/// Lets the web view's player read everything, whatever its origin. What a
/// provider sends is media to play, never a page: one opened anyway can't
/// run scripts as the gateway (the trailer page, which must, isn't sent
/// through here).
fn cors(mut response: Response<Body>) -> Response<Body> {
    let headers = response.headers_mut();
    let mut set = |name: &'static str, value: &'static str| {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    };
    set("access-control-allow-origin", "*");
    set("access-control-allow-methods", "GET, HEAD, OPTIONS");
    set("access-control-allow-headers", "*");
    set(
        "access-control-expose-headers",
        "Content-Length, Content-Range, Accept-Ranges, Content-Type",
    );
    set("access-control-allow-private-network", "true");
    set("cross-origin-resource-policy", "cross-origin");
    set("content-security-policy", "sandbox");
    response
}

fn plain(status: StatusCode, message: &str) -> Response<Body> {
    cors(
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Body::from(message.to_string()))
            .unwrap_or_default(),
    )
}

/// Streams the bytes of `url` (or the target's own address) through, with
/// `http`, passing Range requests along so the player can seek. The body
/// ends when `halted` does (the stream was released), closing the provider
/// connection.
async fn pass_through(
    http: &reqwest::Client,
    target: &StreamTarget,
    url: Option<Url>,
    method: &Method,
    incoming: &HeaderMap,
    halted: Option<impl Future<Output = ()> + Send + 'static>,
) -> Response<Body> {
    let url = match url {
        Some(url) => url,
        None => match Input::of(target) {
            Some(Input::Web(url)) => url,
            Some(Input::File(path)) => return serve_file(path, method, incoming).await,
            None => return plain(StatusCode::BAD_REQUEST, "Unsupported address"),
        },
    };
    let host = url.host_str().unwrap_or_default().to_string();
    let upstream_method = if method == Method::HEAD {
        reqwest::Method::HEAD
    } else {
        reqwest::Method::GET
    };
    let mut request = upstream(http, upstream_method, url, target);
    for name in [header::RANGE, header::IF_RANGE] {
        if let Some(value) = incoming.get(&name) {
            request = request.header(name, value.clone());
        }
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::info!(%host, "a stream couldn't be reached: {}", error.without_url());
            return plain(StatusCode::BAD_GATEWAY, "The stream couldn't be reached.");
        }
    };
    let mut builder = Response::builder().status(response.status());
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
        header::LAST_MODIFIED,
        header::ETAG,
    ] {
        if let Some(value) = response.headers().get(&name) {
            builder = builder.header(name, value.clone());
        }
    }
    let body = match halted {
        _ if method == Method::HEAD => Body::empty(),
        Some(halted) => Body::from_stream(response.bytes_stream().take_until(halted)),
        None => Body::from_stream(response.bytes_stream()),
    };
    builder.body(body).unwrap_or_default()
}

/// A file on this computer (a media folder item, a recording), with ranges.
async fn serve_file(path: PathBuf, method: &Method, incoming: &HeaderMap) -> Response<Body> {
    let mut request = Request::builder().method(method.clone()).uri("/");
    for name in [
        header::RANGE,
        header::IF_RANGE,
        header::IF_MODIFIED_SINCE,
        header::IF_NONE_MATCH,
    ] {
        if let Some(value) = incoming.get(&name) {
            request = request.header(name, value.clone());
        }
    }
    let Ok(request) = request.body(Body::empty()) else {
        return plain(StatusCode::BAD_REQUEST, "Bad request");
    };
    match tower_http::services::ServeFile::new(path)
        .oneshot(request)
        .await
    {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    }
}

async fn serve_hls(
    shared: &Arc<Shared>,
    id: &str,
    entry: &Arc<Entry>,
    name: &str,
    method: &Method,
    incoming: &HeaderMap,
) -> Response<Body> {
    if name == "index.m3u8" {
        return match Input::of(&entry.target) {
            Some(Input::Web(url)) => serve_playlist(shared, id, entry, url, method).await,
            Some(Input::File(path)) => serve_file(path, method, incoming).await,
            None => plain(StatusCode::BAD_REQUEST, "Unsupported address"),
        };
    }
    let number = name.split('.').next().and_then(|n| n.parse::<u64>().ok());
    let child = number.and_then(|n| lock(&entry.children).by_id.get(&n).cloned());
    match child {
        Some(Child {
            url,
            kind: ChildKind::Playlist,
        }) => serve_playlist(shared, id, entry, url, method).await,
        Some(Child {
            url,
            kind: ChildKind::Media,
        }) => pass_through(&shared.http, &entry.target, Some(url), method, incoming, None::<std::future::Ready<()>>).await,
        None => plain(StatusCode::NOT_FOUND, "That segment has expired."),
    }
}

/// Fetches a playlist and rewrites it so everything in it comes back here.
async fn serve_playlist(
    shared: &Arc<Shared>,
    id: &str,
    entry: &Arc<Entry>,
    url: Url,
    method: &Method,
) -> Response<Body> {
    let host = url.host_str().unwrap_or_default().to_string();
    let request = shared.upstream(reqwest::Method::GET, url, &entry.target);
    let mut response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::info!(%host, "a playlist couldn't be reached: {}", error.without_url());
            return plain(StatusCode::BAD_GATEWAY, "The stream couldn't be reached.");
        }
    };
    if !response.status().is_success() {
        return plain(response.status(), "The provider refused the stream.");
    }
    let base = response.url().clone();
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
    let first = match tokio::time::timeout(PLAYLIST_TIMEOUT, response.chunk()).await {
        Ok(Ok(chunk)) => chunk.unwrap_or_default(),
        _ => {
            return plain(
                StatusCode::GATEWAY_TIMEOUT,
                "The stream took too long to answer.",
            );
        }
    };
    if !hls::looks_like_playlist(&first) {
        // Some "playlists" are the media itself; hand it over as it comes.
        let rest = futures_util::stream::once(async move { Ok::<_, reqwest::Error>(first) })
            .chain(response.bytes_stream());
        let mut builder = Response::builder().status(StatusCode::OK);
        if let Some(content_type) = content_type {
            builder = builder.header(header::CONTENT_TYPE, content_type);
        }
        let body = if method == Method::HEAD {
            Body::empty()
        } else {
            Body::from_stream(rest)
        };
        return builder.body(body).unwrap_or_default();
    }
    let mut bytes = first.to_vec();
    let rest = async {
        while let Some(chunk) = response.chunk().await? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > MAX_PLAYLIST_BYTES {
                break;
            }
        }
        Ok::<_, reqwest::Error>(())
    };
    match tokio::time::timeout(PLAYLIST_TIMEOUT, rest).await {
        Ok(Ok(())) if bytes.len() <= MAX_PLAYLIST_BYTES => {}
        _ => return plain(StatusCode::BAD_GATEWAY, "The playlist couldn't be read."),
    }
    let text = String::from_utf8_lossy(&bytes);
    let rewritten = hls::rewrite(&text, &base, |url, kind| {
        let extension = extension_of(url.path()).unwrap_or_else(|| match kind {
            ChildKind::Playlist => "m3u8".into(),
            ChildKind::Media => "ts".into(),
        });
        let n = lock(&entry.children).register(url, kind);
        shared.address("h", id, &format!("{n}.{extension}"))
    });
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(rewritten)
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(body)
        .unwrap_or_default()
}

/// Starts ffmpeg for this request and streams MPEG-TS back. Only one
/// ffmpeg runs per stream: a new request stops the one before.
fn serve_remux(shared: &Shared, id: &str, entry: &Entry, method: &Method) -> Response<Body> {
    let Some(tools) = &shared.tools else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, FFMPEG_MISSING);
    };
    let (Some(plan), Some(input)) = (&entry.remux, Input::of(&entry.target)) else {
        return plain(StatusCode::BAD_REQUEST, "This stream isn't remuxed.");
    };
    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/mp2t")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ACCEPT_RANGES, "none");
    if method == Method::HEAD {
        return builder.body(Body::empty()).unwrap_or_default();
    }
    entry.halt();
    let generation = *entry.stop.borrow();
    let subtitle_dir = shared.subtitle_root.join(id);
    let subtitle_dir = std::fs::create_dir_all(&subtitle_dir).is_ok().then_some(subtitle_dir.as_path());
    let args = plan.args(&entry.target, &input, entry.offset, subtitle_dir);
    match remux::spawn(&tools.ffmpeg, args, entry.stop.subscribe(), generation) {
        Ok(stream) => builder.body(Body::from_stream(stream)).unwrap_or_default(),
        Err(error) => {
            tracing::error!("ffmpeg couldn't be started: {error:#}");
            plain(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ffmpeg couldn't be started.",
            )
        }
    }
}

/// A remuxed stream's subtitle track as WebVTT, from the file ffmpeg is
/// writing beside the video. It grows as the video streams, so the player
/// asks again for the bytes after those it has (a `Range` from there on).
async fn serve_subtitles(shared: &Shared, id: &str, entry: &Entry, name: &str, headers: &HeaderMap) -> Response<Body> {
    let Some(index) = name.strip_suffix(".vtt").and_then(|i| i.parse::<i64>().ok()) else {
        return plain(StatusCode::NOT_FOUND, "Not found");
    };
    if !entry.remux.as_ref().is_some_and(|plan| plan.has_subtitles(index)) {
        return plain(StatusCode::NOT_FOUND, "This stream has no such subtitles.");
    }
    // Not written yet (ffmpeg hasn't reached the first one) reads as empty.
    let bytes = tokio::fs::read(shared.subtitle_root.join(id).join(format!("{index}.vtt"))).await.unwrap_or_default();
    let from = headers
        .get(header::RANGE)
        .and_then(|r| r.to_str().ok())
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.split('-').next())
        .and_then(|start| start.parse::<usize>().ok())
        .unwrap_or(0)
        .min(bytes.len());
    Response::builder()
        .status(if from > 0 { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK })
        .header(header::CONTENT_TYPE, "text/vtt; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes[from..].to_vec()))
        .unwrap_or_default()
}

/// A buffered live stream: its playlist (once it lists a segment), init
/// section and segments, from the folder ffmpeg writes. The playlist's
/// addresses are relative, so they resolve to this route.
async fn serve_timeshift(entry: &Entry, name: &str, method: &Method, incoming: &HeaderMap) -> Response<Body> {
    let Some(session) = &entry.timeshift else {
        return plain(StatusCode::NOT_FOUND, "Not found");
    };
    if name == timeshift::PLAYLIST {
        if let Err(unready) = session.wait_ready(FIRST_SEGMENT_WAIT).await {
            let status = match unready {
                Unready::Failed(_) => StatusCode::BAD_GATEWAY,
                Unready::Slow => StatusCode::GATEWAY_TIMEOUT,
                Unready::Gone => StatusCode::NOT_FOUND,
            };
            return plain(status, unready.message());
        }
        let Ok(playlist) = tokio::fs::read_to_string(session.playlist()).await else {
            return plain(StatusCode::NOT_FOUND, STREAM_GONE);
        };
        let body = if method == Method::HEAD {
            Body::empty()
        } else {
            Body::from(playlist)
        };
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")
            .header(header::CACHE_CONTROL, "no-store")
            .body(body)
            .unwrap_or_default();
    }
    if !timeshift::is_part(name) {
        return plain(StatusCode::NOT_FOUND, "Not found");
    }
    let mut response = serve_file(session.dir().join(name), method, incoming).await;
    if response.status() == StatusCode::NOT_FOUND {
        return plain(StatusCode::NOT_FOUND, "That part of the buffer has gone.");
    }
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
    response
}

/// Where remuxed streams' subtitles are written while they play.
fn subtitle_root() -> PathBuf {
    crate::scratch::temp_root().join("xorvio-subtitles")
}

/// An MPEG-TS file modified in the last few seconds: a recording in progress.
fn is_growing_transport(path: &std::path::Path, ext: Option<&str>) -> bool {
    matches!(ext, Some("ts" | "mts" | "m2ts"))
        && path
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age < GROWING_WINDOW))
}

/// A short, plain extension: "mkv" from "…/film.MKV?token=…".
fn extension_of(address: &str) -> Option<String> {
    let path = address.split(['?', '#']).next().unwrap_or(address);
    let last = path.rsplit('/').next().unwrap_or(path);
    let (_, ext) = last.rsplit_once('.')?;
    let ext = clean_extension(ext);
    (!ext.is_empty()).then_some(ext)
}

fn clean_extension(ext: &str) -> String {
    let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    if ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        ext
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert_eq!(
            extension_of("http://a/b/film.MKV?x=1.y").as_deref(),
            Some("mkv")
        );
        assert_eq!(extension_of("http://a/live/1234"), None);
        assert_eq!(extension_of("http://a/x.reallylong"), None);
        assert_eq!(clean_extension(".M3U8"), "m3u8");
    }

    #[test]
    fn file_names_for_other_players() {
        assert_eq!(file_name("Severance - S01E03 - Good News", "mkv"), "Severance%20-%20S01E03%20-%20Good%20News.mkv");
        assert_eq!(file_name("AC/DC: Live at River Plate (2011)", "mp4"), "AC%20DC%20Live%20at%20River%20Plate%20(2011).mp4");
        assert_eq!(file_name("  \n ", "ts"), "Video.ts");
        assert_eq!(file_name("Amélie", "mkv"), "Am%C3%A9lie.mkv");
        assert_eq!(file_name("?#%", "ts"), "Video.ts", "nothing that ends the path or starts a query");
        assert!(file_name(&"x".repeat(500), "mkv").len() < 120);
    }

    #[test]
    fn unopened_streams_go_as_playlists_by_address_and_format() {
        let live = |url: &str| StreamTarget { url: url.into(), user_agent: None, headers: vec![], is_live: true };
        let film = StreamTarget { is_live: false, ..live("http://a/movie/1.mkv") };
        assert!(is_playlist(&film, Some("m3u8"), None));
        assert!(!is_playlist(&film, Some("mkv"), None));
        assert!(!is_playlist(&film, None, None));
        let channel = live("http://a/live/u/p/1");
        assert!(!is_playlist(&channel, Some("ts"), Some(LiveStreamFormat::Hls)), "the address wins");
        assert!(is_playlist(&channel, None, Some(LiveStreamFormat::Hls)));
        assert!(!is_playlist(&channel, None, Some(LiveStreamFormat::Mpegts)));
        assert!(is_playlist(&channel, None, None), "unknown live streams take the route that handles both");
        assert_eq!(fallback_extension(&channel, PlaybackMode::Remux), "ts");
        assert_eq!(fallback_extension(&film, PlaybackMode::Remux), "mkv");
    }

    #[test]
    fn segments_are_forgotten_but_playlists_kept() {
        let mut children = Children::default();
        let playlist =
            children.register(Url::parse("https://a/v.m3u8").unwrap(), ChildKind::Playlist);
        for n in 0..(MAX_SEGMENTS + 10) {
            children.register(
                Url::parse(&format!("https://a/s{n}.ts")).unwrap(),
                ChildKind::Media,
            );
        }
        assert!(children.by_id.contains_key(&playlist));
        assert_eq!(children.segments.len(), MAX_SEGMENTS);
        assert_eq!(children.by_id.len(), MAX_SEGMENTS + 1);
        assert_eq!(
            children.register(Url::parse("https://a/v.m3u8").unwrap(), ChildKind::Playlist),
            playlist
        );
        // Endless new variants are dropped the same way.
        for n in 0..(MAX_SEGMENTS + 10) {
            children.register(Url::parse(&format!("https://a/v{n}.m3u8")).unwrap(), ChildKind::Playlist);
        }
        assert_eq!(children.others.len(), MAX_SEGMENTS);
        assert_eq!(children.by_id.len(), 2 * MAX_SEGMENTS);
    }
}
