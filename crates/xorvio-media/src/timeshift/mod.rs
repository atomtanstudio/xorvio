//! Pausing and rewinding live TV. While a buffered channel plays, ffmpeg
//! writes it as a sliding window of HLS (fragmented MP4 segments) into a
//! temporary folder, which the gateway serves; the web view plays that as
//! live HLS, so pausing, seeking within the window and the seekable range
//! all come from the player itself.
//!
//! The provider sees one connection, as most IPTV logins allow. For MPEG-TS
//! and FLV the gateway reads the stream itself and pipes it into ffmpeg,
//! after reading its first packets to learn the codecs (and to start ffmpeg
//! on a keyframe, which makes it start much sooner). For HLS, ffmpeg reads
//! the playlist itself; the codecs come from the playlist's CODECS
//! attribute or one segment fetched beforehand.
//!
//! A session's folder goes when it's released (and ffmpeg with it); folders
//! a crash left behind are cleared as the gateway starts.

mod plan;
mod sniff;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin};
use tokio::sync::watch;
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::Instant;
use url::Url;
use xorvio_model::{StreamInfo, StreamTarget};

use self::plan::Plan;
use self::sniff::{Codecs, TsScan};
use crate::gateway::{PlaybackSupport, upstream};
use crate::hls;
use crate::process::{Input, drain_stderr, tool_command};

pub(crate) use plan::{INIT, PLAYLIST};

/// How long to read a stream for its codecs and first keyframe before
/// starting ffmpeg with what's known.
const SNIFF_TIME: Duration = Duration::from_secs(10);
const SNIFF_BYTES: usize = 32 * 1024 * 1024;
/// Not MPEG-TS after this much: no point waiting for a PMT.
const NOT_TRANSPORT_BYTES: usize = 4096;
const MAX_PLAYLIST_BYTES: usize = 8 * 1024 * 1024;
/// A segment or init section fetched for its codecs.
const PART_TIME: Duration = Duration::from_secs(8);
const ANSWER_TIME: Duration = Duration::from_secs(15);
/// A stream that sends nothing for this long has stalled.
const READ_TIMEOUT: Duration = Duration::from_secs(15);
/// The first connection is tried this often, a second apart: zapping can
/// leave the previous channel's connection open for a moment (until its own
/// start finishes and the interface releases it), which a provider allowing
/// one refuses.
const CONNECT_ATTEMPTS: usize = 4;
const RETRY_DELAY: Duration = Duration::from_secs(1);
/// Reconnects in a row before the buffer stops growing; a stream that ran
/// this long counts as a fresh start.
const MAX_RECONNECTS: u32 = 6;
const STEADY: Duration = Duration::from_secs(30);
/// A folder nothing has written to for this long belongs to no running buffer.
const STALE: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(100);

const UNREACHABLE: &str = "The stream couldn't be reached.";
const REFUSED: &str = "The provider refused the stream.";
const SLOW: &str = "The stream took too long to answer.";
const NOT_VIDEO: &str = "The provider sent something that isn't a video stream.";
const UNREADABLE_PLAYLIST: &str = "The channel's playlist couldn't be read.";

/// Where buffers live: a folder per session.
pub(crate) fn root() -> PathBuf {
    std::env::temp_dir().join("xorvio-timeshift")
}

/// Removes buffers a run that ended without cleaning up (a crash, a forced
/// quit) left behind: folders nothing has written to for a minute. A buffer
/// that's running gets a segment every couple of seconds, so another copy
/// of Xorvio running at the same time (a development build) keeps its own.
pub(crate) fn sweep(root: &Path, keep: &[PathBuf]) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if keep.contains(&path) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > STALE));
        if stale {
            tracing::info!("removing a live buffer left from before");
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Stops buffers' ffmpegs that outlived the Xorvio that started them (a
/// crash, a forced quit): one reading an HLS channel holds the provider's
/// connection and writes for good, with nothing to tell it to stop. Those
/// were handed to launchd when Xorvio ended; another copy of Xorvio still
/// running keeps its own.
#[cfg(unix)]
pub(crate) fn stop_orphans(root: &Path) {
    let Ok(listing) = std::process::Command::new("ps").args(["-A", "-o", "pid=,ppid=,command="]).output() else { return };
    let root = root.to_string_lossy();
    for line in String::from_utf8_lossy(&listing.stdout).lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (fields.next(), fields.next()) else { continue };
        let command = fields.collect::<Vec<_>>().join(" ");
        if ppid == "1" && command.contains("ffmpeg") && command.contains(root.as_ref()) {
            tracing::info!("stopping a live buffer left running from before");
            let _ = std::process::Command::new("kill").arg(pid).status();
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn stop_orphans(_root: &Path) {}

/// The files a session serves besides its playlist: `init.mp4` and `seg<n>.m4s`.
pub(crate) fn is_part(name: &str) -> bool {
    name == INIT
        || name
            .strip_prefix("seg")
            .and_then(|n| n.strip_suffix(".m4s"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

#[derive(Debug, Clone, PartialEq)]
enum State {
    Running,
    /// ffmpeg stopped with an error; the message says why, for people.
    Failed(String),
    /// The stream ended and ffmpeg finished the playlist.
    Ended,
}

/// Why a session's playlist can't be served (yet).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Unready {
    Failed(String),
    Slow,
    Gone,
}

impl Unready {
    pub fn message(&self) -> &str {
        match self {
            Unready::Failed(message) => message,
            Unready::Slow => "The channel is taking too long to start.",
            Unready::Gone => "That stream isn't playing any more.",
        }
    }
}

/// A live channel being buffered: ffmpeg writing into its folder, and the
/// task feeding it the provider's stream (for MPEG-TS and FLV).
pub(crate) struct Session {
    window: f64,
    dir: PathBuf,
    /// The provider's stream is an HLS playlist (rather than MPEG-TS or FLV).
    hls_source: bool,
    info: StreamInfo,
    started: Instant,
    state: watch::Receiver<State>,
    stop: watch::Sender<bool>,
    feeder: Mutex<Option<AbortHandle>>,
    /// ffmpeg's process, for stopping it when Xorvio quits.
    pid: Option<u32>,
    /// The time to the first segment has been logged.
    reported: AtomicBool,
}

impl Session {
    /// Opens the live `target`, learns what it holds, and starts ffmpeg
    /// writing a buffer of `window` seconds into a new folder under `root`.
    /// Returns once ffmpeg is running; [`Session::wait_ready`] waits for the
    /// first segment. Errors are plain English (the provider refused, the
    /// stream isn't video…).
    pub async fn start(
        http: &reqwest::Client,
        ffmpeg: &Path,
        root: &Path,
        target: &StreamTarget,
        support: PlaybackSupport,
        window: f64,
    ) -> anyhow::Result<Arc<Session>> {
        Session::start_decoding(http, ffmpeg, root, target, hls_support(support), window).await
    }

    /// The same for a player that decodes exactly `support` in HLS (a TV,
    /// which gets H.264 and AAC), rather than the web view.
    pub async fn start_decoding(
        http: &reqwest::Client,
        ffmpeg: &Path,
        root: &Path,
        target: &StreamTarget,
        support: PlaybackSupport,
        window: f64,
    ) -> anyhow::Result<Arc<Session>> {
        let Some(Input::Web(url)) = Input::of(target) else {
            bail!("Only channels streamed over the web can be paused.");
        };
        let started = Instant::now();
        let source = open(http, target, url).await?;
        let (input, codecs, container) = match &source {
            Source::Pipe { format, from_keyframe, codecs, .. } => (
                plan::Input::Pipe { format: *format, from_keyframe: *from_keyframe },
                *codecs,
                match format {
                    Some("flv") => "FLV",
                    Some(_) => "MPEG-TS",
                    None => "Live stream",
                },
            ),
            Source::Playlist { url, codecs } => (plan::Input::Playlist { url: url.clone() }, *codecs, "HLS"),
        };
        let hls_source = matches!(source, Source::Playlist { .. });
        let plan = Plan::new(input, codecs, support, window);
        let dir = root.join(uuid::Uuid::new_v4().simple().to_string());
        std::fs::create_dir_all(&dir).map_err(|error| {
            tracing::error!("the live buffer's folder couldn't be made: {error}");
            anyhow!("Xorvio couldn't make a folder for the live buffer.")
        })?;
        let spawned = tool_command(ffmpeg)
            .args(plan.args(target, &dir))
            .stdin(if plan.reads_stdin() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&dir);
                tracing::error!("ffmpeg couldn't be started: {error}");
                bail!("ffmpeg couldn't be started.");
            }
        };
        let read_for = started.elapsed().as_secs_f32();
        tracing::info!("live buffer started after {read_for:.1}s reading the stream: {}", plan.describe());
        let stderr = match child.stderr.take() {
            Some(stderr) => tokio::spawn(drain_stderr(stderr, "timeshift")),
            None => tokio::spawn(async { Vec::new() }),
        };
        let feeder = match (source, child.stdin.take()) {
            (Source::Pipe { head, response, url, .. }, Some(stdin)) => {
                let task = feed(http.clone(), target.clone(), url, head, *response, stdin);
                Some(tokio::spawn(task).abort_handle())
            }
            _ => None,
        };
        let (stop, stopped) = watch::channel(false);
        let (state_sender, state) = watch::channel(State::Running);
        let pid = child.id();
        tokio::spawn(supervise(child, stderr, stopped, state_sender, dir.clone()));
        Ok(Arc::new(Session {
            window,
            dir,
            hls_source,
            info: StreamInfo {
                container: container.to_string(),
                video: codecs.video.and_then(|v| v.codec.name()).map(String::from),
                audio: codecs.audio.and_then(|a| a.codec.name()).map(String::from),
                notes: plan.notes(),
            },
            started,
            state,
            stop,
            feeder: Mutex::new(feeder),
            pid,
            reported: AtomicBool::new(false),
        }))
    }

    pub fn window(&self) -> f64 {
        self.window
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn playlist(&self) -> PathBuf {
        self.dir.join(PLAYLIST)
    }

    pub fn is_hls_source(&self) -> bool {
        self.hls_source
    }

    pub fn info(&self) -> StreamInfo {
        self.info.clone()
    }

    /// Stops reading the provider's stream (closing the connection now) and
    /// ffmpeg; the folder goes once ffmpeg has.
    pub fn stop(&self) {
        if let Some(feeder) = self.feeder.lock().unwrap_or_else(|e| e.into_inner()).take() {
            feeder.abort();
        }
        self.stop.send_replace(true);
    }

    /// Stops ffmpeg and removes the folder before returning, for when Xorvio
    /// quits and the tasks that would otherwise do it won't run again.
    pub fn stop_now(&self) {
        self.stop();
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
        }
        #[cfg(windows)]
        if let Some(pid) = self.pid {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .creation_flags(crate::process::CREATE_NO_WINDOW)
                .status();
            // Windows can't delete a file that's still open, and a killed
            // ffmpeg lets go of its segments a moment after taskkill returns.
            for _ in 0..20 {
                if std::fs::remove_dir_all(&self.dir).is_ok() || !self.dir.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }

    /// Waits up to `limit` until the playlist lists a segment, so the player
    /// never sees an empty or missing one; or until ffmpeg fails.
    pub async fn wait_ready(&self, limit: Duration) -> Result<(), Unready> {
        let deadline = Instant::now() + limit;
        loop {
            if self.has_segment().await {
                if !self.reported.swap(true, Ordering::Relaxed) {
                    let seconds = self.started.elapsed().as_secs_f32();
                    tracing::info!("live buffer: first segment {seconds:.1}s after the stream was opened");
                }
                return Ok(());
            }
            if *self.stop.borrow() {
                return Err(Unready::Gone);
            }
            let state = self.state.borrow().clone();
            match state {
                State::Failed(message) => return Err(Unready::Failed(message)),
                State::Ended => return Err(Unready::Failed("The channel stopped before any of it was buffered.".into())),
                State::Running => {}
            }
            if Instant::now() >= deadline {
                return Err(Unready::Slow);
            }
            tokio::time::sleep(POLL).await;
        }
    }

    async fn has_segment(&self) -> bool {
        tokio::fs::read_to_string(self.playlist())
            .await
            .is_ok_and(|text| text.lines().any(|line| !line.trim().is_empty() && !line.starts_with('#')))
    }
}

/// On macOS the web view plays the buffer with WebKit's own HLS player,
/// which decodes Dolby audio whatever its Media Source reports; elsewhere
/// hls.js feeds Media Source, so that report stands.
fn hls_support(support: PlaybackSupport) -> PlaybackSupport {
    let native = cfg!(target_os = "macos");
    PlaybackSupport {
        ac3: support.ac3 || native,
        eac3: support.eac3 || native,
        ..support
    }
}

/// What the provider's address turned out to be.
enum Source {
    /// MPEG-TS or FLV (or something ffmpeg will recognise), read by the
    /// gateway. `head` is what ffmpeg gets first; `response` carries on.
    Pipe {
        format: Option<&'static str>,
        from_keyframe: bool,
        codecs: Codecs,
        head: Vec<u8>,
        response: Box<reqwest::Response>,
        url: Url,
    },
    /// An HLS playlist, for ffmpeg to read at `url`.
    Playlist { url: String, codecs: Codecs },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Read {
    More,
    End,
    Late,
}

async fn read_more(response: &mut reqwest::Response, buf: &mut Vec<u8>, deadline: Instant) -> Read {
    match tokio::time::timeout_at(deadline, response.chunk()).await {
        Ok(Ok(Some(chunk))) => {
            buf.extend_from_slice(&chunk);
            Read::More
        }
        Ok(Ok(None) | Err(_)) => Read::End,
        Err(_) => Read::Late,
    }
}

/// Connects to the channel and reads enough to know what it is and what it
/// holds. One connection: what's read here is what ffmpeg gets first.
async fn open(http: &reqwest::Client, target: &StreamTarget, url: Url) -> anyhow::Result<Source> {
    let mut response = connect(http, target, &url, CONNECT_ATTEMPTS).await?;
    let deadline = Instant::now() + SNIFF_TIME;
    let mut head = Vec::new();
    let mut read = Read::More;
    while head.len() < 16 && read == Read::More {
        read = read_more(&mut response, &mut head, deadline).await;
    }
    if head.is_empty() {
        bail!(if read == Read::Late { SLOW } else { NOT_VIDEO });
    }

    if hls::looks_like_playlist(&head) {
        let base = response.url().clone();
        while read == Read::More && head.len() <= MAX_PLAYLIST_BYTES {
            read = read_more(&mut response, &mut head, deadline).await;
        }
        if read != Read::End {
            bail!(UNREADABLE_PLAYLIST);
        }
        drop(response);
        let text = String::from_utf8_lossy(&head).into_owned();
        return playlist_source(http, target, url, base, &text).await;
    }

    if head.starts_with(b"FLV") {
        while read == Read::More && head.len() < SNIFF_BYTES && !sniff::scan_flv(&head).is_some_and(|s| s.is_complete()) {
            read = read_more(&mut response, &mut head, deadline).await;
        }
        let codecs = sniff::scan_flv(&head).map_or_else(Codecs::unknown, |s| s.codecs());
        return Ok(Source::Pipe { format: Some("flv"), from_keyframe: false, codecs, head, response: Box::new(response), url });
    }

    let mut scan = TsScan::default();
    loop {
        scan.scan(&head);
        let not_transport = !scan.is_transport() && head.len() >= NOT_TRANSPORT_BYTES;
        if scan.is_ready() || not_transport || read != Read::More || head.len() >= SNIFF_BYTES {
            break;
        }
        read = read_more(&mut response, &mut head, deadline).await;
    }
    if !scan.is_transport() && read == Read::End {
        // A short answer that isn't a stream: an error page, "too many connections"…
        bail!(NOT_VIDEO);
    }
    let codecs = match scan.codecs() {
        Some(codecs) if codecs.is_empty() => bail!("This channel has no picture or sound Xorvio can buffer."),
        Some(codecs) => codecs,
        None => Codecs::unknown(),
    };
    let from_keyframe = scan.starts_at_random_access() || (scan.knows_program() && codecs.video.is_none());
    if scan.knows_program() && !from_keyframe {
        tracing::info!("no keyframe found in the first seconds of the stream; ffmpeg probes longer");
    }
    Ok(Source::Pipe {
        format: scan.is_transport().then_some("mpegts"),
        from_keyframe,
        codecs,
        head: scan.start_bytes(&head),
        response: Box::new(response),
        url,
    })
}

/// An HLS channel: the address ffmpeg reads and what it holds. A master
/// playlist's best variant goes to ffmpeg by itself (so it reads only that
/// one), unless its audio comes as a separate rendition.
async fn playlist_source(
    http: &reqwest::Client,
    target: &StreamTarget,
    original: Url,
    base: Url,
    text: &str,
) -> anyhow::Result<Source> {
    let variants = sniff::variants(text);
    let (input, media) = match variants.iter().max_by_key(|v| v.bandwidth) {
        None => (original.to_string(), Some((base, text.to_string()))),
        Some(best) => {
            let variant = base.join(best.uri.trim()).map_err(|_| anyhow!(UNREADABLE_PLAYLIST))?;
            let separate_audio = best.audio.as_deref().is_some_and(|group| sniff::has_separate_audio(text, group));
            let input = if variants.len() == 1 || separate_audio { original.to_string() } else { variant.to_string() };
            if let Some(codecs) = best.codecs.as_deref().map(sniff::codecs_attribute).filter(|c| !c.is_empty()) {
                return Ok(Source::Playlist { url: input, codecs });
            }
            let media = fetch(http, target, variant, MAX_PLAYLIST_BYTES, |_| false)
                .await
                .map(|(base, bytes)| (base, String::from_utf8_lossy(&bytes).into_owned()));
            (input, media)
        }
    };
    let codecs = match media {
        Some((base, text)) => segment_codecs(http, target, &base, &text).await,
        None => None,
    };
    if codecs.is_none() {
        tracing::info!("the HLS channel's codecs couldn't be read; converting both");
    }
    Ok(Source::Playlist { url: input, codecs: codecs.unwrap_or_else(Codecs::unknown).without_pids() })
}

/// What a media playlist's streams are, from its init section (fMP4) or the
/// start of its newest segment (MPEG-TS).
async fn segment_codecs(http: &reqwest::Client, target: &StreamTarget, base: &Url, text: &str) -> Option<Codecs> {
    let (init, last) = sniff::media_parts(text);
    if let Some(init) = init {
        let (_, bytes) = fetch(http, target, base.join(&init).ok()?, 1 << 20, |_| false).await?;
        return sniff::mp4_codecs(&bytes);
    }
    let segment = base.join(&last?).ok()?;
    let mut scan = TsScan::default();
    let (_, bytes) = fetch(http, target, segment, 4 << 20, |bytes| {
        scan.scan(bytes);
        scan.knows_program()
    })
    .await?;
    scan.scan(&bytes);
    scan.codecs()
}

/// GETs `url` until `enough` says so, `limit` bytes or the end, then drops
/// the connection. The final address (after redirects) comes back with the bytes.
async fn fetch(
    http: &reqwest::Client,
    target: &StreamTarget,
    url: Url,
    limit: usize,
    mut enough: impl FnMut(&[u8]) -> bool,
) -> Option<(Url, Vec<u8>)> {
    let deadline = Instant::now() + PART_TIME;
    let request = upstream(http, reqwest::Method::GET, url, target).send();
    let mut response = tokio::time::timeout_at(deadline, request).await.ok()?.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let base = response.url().clone();
    let mut bytes = Vec::new();
    while bytes.len() < limit && !enough(&bytes) {
        if read_more(&mut response, &mut bytes, deadline).await != Read::More {
            break;
        }
    }
    Some((base, bytes))
}

/// A GET of the channel that answered with success, trying `attempts` times.
async fn connect(
    http: &reqwest::Client,
    target: &StreamTarget,
    url: &Url,
    attempts: usize,
) -> anyhow::Result<reqwest::Response> {
    let host = url.host_str().unwrap_or_default().to_string();
    let mut last = anyhow!(UNREACHABLE);
    for attempt in 0..attempts {
        if attempt > 0 {
            tokio::time::sleep(RETRY_DELAY).await;
        }
        let request = upstream(http, reqwest::Method::GET, url.clone(), target).send();
        match tokio::time::timeout(ANSWER_TIME, request).await {
            Ok(Ok(response)) if response.status().is_success() => return Ok(response),
            Ok(Ok(response)) => {
                let status = response.status();
                tracing::info!(%host, %status, "the provider refused a live buffer's stream");
                last = anyhow!(REFUSED);
                // Not there at all: asking again won't help.
                if matches!(status.as_u16(), 404 | 410) {
                    break;
                }
            }
            Ok(Err(error)) => {
                tracing::info!(%host, "a live buffer's stream couldn't be reached: {}", error.without_url());
                last = anyhow!(UNREACHABLE);
            }
            Err(_) => last = anyhow!(SLOW),
        }
    }
    Err(last)
}

/// Writes the provider's stream into ffmpeg, reconnecting (one connection at
/// a time) when it drops. Giving up closes ffmpeg's input, so it finishes
/// the playlist and what's buffered stays watchable.
async fn feed(
    http: reqwest::Client,
    target: StreamTarget,
    url: Url,
    head: Vec<u8>,
    response: reqwest::Response,
    mut stdin: ChildStdin,
) {
    if stdin.write_all(&head).await.is_err() {
        return;
    }
    drop(head);
    let host = url.host_str().unwrap_or_default().to_string();
    let mut response = Some(response);
    let mut failures = 0u32;
    loop {
        if let Some(mut body) = response.take() {
            let since = Instant::now();
            loop {
                match tokio::time::timeout(READ_TIMEOUT, body.chunk()).await {
                    Ok(Ok(Some(chunk))) => {
                        if stdin.write_all(&chunk).await.is_err() {
                            // ffmpeg has stopped; the supervisor says why.
                            return;
                        }
                    }
                    Ok(Ok(None)) => {
                        tracing::info!(%host, "the provider ended a buffered stream");
                        break;
                    }
                    Ok(Err(error)) => {
                        tracing::info!(%host, "a buffered stream dropped: {}", error.without_url());
                        break;
                    }
                    Err(_) => {
                        tracing::info!(%host, "a buffered stream stalled");
                        break;
                    }
                }
            }
            if since.elapsed() >= STEADY {
                failures = 0;
            }
        }
        failures += 1;
        if failures > MAX_RECONNECTS {
            tracing::info!(%host, "a live buffer stopped reconnecting");
            return;
        }
        tokio::time::sleep(Duration::from_secs(1 << (failures - 1).min(3))).await;
        response = connect(&http, &target, &url, 1).await.ok();
    }
}

/// Watches ffmpeg until the session stops: records why it ended if it does,
/// then kills it (when it's still running) and removes the folder.
async fn supervise(
    mut child: Child,
    stderr: JoinHandle<Vec<String>>,
    mut stop: watch::Receiver<bool>,
    state: watch::Sender<State>,
    dir: PathBuf,
) {
    let stopped = tokio::select! {
        status = child.wait() => {
            let tail = stderr.await.unwrap_or_default();
            if !tail.is_empty() {
                tracing::info!("a live buffer's ffmpeg said: {}", tail.join(" | "));
            }
            state.send_replace(match status {
                Ok(status) if status.success() => State::Ended,
                _ => State::Failed(failure(&tail)),
            });
            false
        }
        () = async { let _ = stop.wait_for(|stop| *stop).await; } => true,
    };
    if stopped {
        let _ = child.kill().await;
    } else {
        // What was buffered stays watchable until the stream is released.
        let _ = stop.wait_for(|stop| *stop).await;
    }
    if let Err(error) = tokio::fs::remove_dir_all(&dir).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("a live buffer's folder couldn't be removed: {error}");
    }
}

/// A message for people from ffmpeg's errors: the first (the cause; later
/// ones follow from it), without ffmpeg's "[muxer @ 0x…]" prefixes.
fn failure(tail: &[String]) -> String {
    let reason = tail
        .iter()
        .map(|line| {
            let mut line = line.as_str();
            while let Some(rest) = line.strip_prefix('[').and_then(|l| l.split_once("] ")).map(|(_, rest)| rest) {
                line = rest;
            }
            line.trim()
        })
        .find(|line| !line.is_empty());
    match reason {
        Some(reason) => {
            let reason: String = reason.chars().take(160).collect();
            format!("Xorvio couldn't buffer this channel ({}).", reason.trim_end_matches('.'))
        }
        None => "Xorvio couldn't buffer this channel.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_are_the_init_section_and_numbered_segments() {
        assert!(is_part("init.mp4"));
        assert!(is_part("seg0.m4s") && is_part("seg1234.m4s"));
        assert!(!is_part("seg.m4s") && !is_part("seg1.m4s.tmp") && !is_part("index.m3u8"));
        assert!(!is_part("../seg1.m4s") && !is_part("seg-1.m4s"));
    }

    /// A session over `dir` whose ffmpeg state the test sets.
    fn session(dir: &Path) -> (Session, watch::Sender<State>) {
        let (state_sender, state) = watch::channel(State::Running);
        let session = Session {
            window: 1800.0,
            dir: dir.to_path_buf(),
            hls_source: false,
            info: StreamInfo::default(),
            started: Instant::now(),
            state,
            stop: watch::channel(false).0,
            feeder: Mutex::new(None),
            pid: None,
            reported: AtomicBool::new(false),
        };
        (session, state_sender)
    }

    #[tokio::test]
    async fn the_playlist_waits_for_a_first_segment() {
        let dir = tempfile::tempdir().unwrap();
        let (session, state) = session(dir.path());
        // ffmpeg writes a playlist with no segment first, then one with a segment.
        std::fs::write(dir.path().join(PLAYLIST), "#EXTM3U\n#EXT-X-TARGETDURATION:2\n").unwrap();
        let playlist = dir.path().join(PLAYLIST);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            std::fs::write(playlist, "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\nseg0.m4s\n").unwrap();
        });
        let started = Instant::now();
        assert_eq!(session.wait_ready(Duration::from_secs(5)).await, Ok(()));
        assert!(started.elapsed() >= Duration::from_millis(400), "it waited");

        let empty = tempfile::tempdir().unwrap();
        let (slow, _) = super::tests::session(empty.path());
        assert_eq!(slow.wait_ready(Duration::from_millis(300)).await, Err(Unready::Slow));
        let (failed, failing) = super::tests::session(empty.path());
        failing.send_replace(State::Failed("Xorvio couldn't buffer this channel.".into()));
        assert_eq!(failed.wait_ready(Duration::from_secs(5)).await, Err(Unready::Failed("Xorvio couldn't buffer this channel.".into())));
        let (ended, ending) = super::tests::session(empty.path());
        ending.send_replace(State::Ended);
        assert!(matches!(ended.wait_ready(Duration::from_secs(5)).await, Err(Unready::Failed(_))));
        ended.stop();
        assert_eq!(ended.wait_ready(Duration::from_secs(5)).await, Err(Unready::Gone));
        drop(state);
    }

    #[test]
    fn failures_read_plainly() {
        let tail = [
            "[in#0/mpegts @ 0x9c5018500] Could not find codec parameters for stream 0".to_string(),
            "[out#0/hls @ 0xb52c28300] Could not write header (incorrect codec parameters ?): Invalid argument".to_string(),
        ];
        assert_eq!(failure(&tail), "Xorvio couldn't buffer this channel (Could not find codec parameters for stream 0).");
        assert_eq!(failure(&[]), "Xorvio couldn't buffer this channel.");
    }

    #[test]
    fn sweeps_only_stale_folders() {
        let root = tempfile::tempdir().unwrap();
        let fresh = root.path().join("fresh");
        let ours = root.path().join("ours");
        let old = root.path().join("old");
        for dir in [&fresh, &ours, &old] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("seg1.m4s"), b"x").unwrap();
        }
        let long_ago = std::time::SystemTime::now() - Duration::from_secs(600);
        for dir in [&ours, &old] {
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            // Windows opens a folder only with backup semantics, and to set
            // its times, with the right to write its attributes.
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                // FILE_WRITE_ATTRIBUTES; FILE_FLAG_BACKUP_SEMANTICS.
                options.access_mode(0x100).custom_flags(0x0200_0000);
            }
            options.open(dir).unwrap().set_modified(long_ago).unwrap();
        }
        sweep(root.path(), std::slice::from_ref(&ours));
        assert!(fresh.exists(), "another copy of Xorvio may be writing it");
        assert!(ours.exists(), "a buffer of this gateway whose stream ended");
        assert!(!old.exists());
    }

    #[test]
    fn dolby_audio_plays_in_webkits_own_hls_player() {
        let reported = PlaybackSupport { hevc: true, ac3: false, eac3: false, vp9: false, av1: false };
        let support = hls_support(reported);
        assert_eq!(support.ac3, cfg!(target_os = "macos"));
        assert_eq!(support.eac3, cfg!(target_os = "macos"));
        assert!(support.hevc);
    }
}
