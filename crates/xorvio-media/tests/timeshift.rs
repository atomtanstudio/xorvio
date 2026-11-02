//! Live buffers (pause and rewind) against local live sources: ffmpeg
//! encoding test pictures and tone in real time as MPEG-TS, served by a
//! test server that insists on the provider's headers, and ffmpeg writing a
//! live HLS playlist into a folder the same server serves. Nothing reaches
//! the internet.

mod common;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use bytes::Bytes;
use common::*;
use futures_util::StreamExt;
use tokio::io::AsyncReadExt;
use xorvio_media::{Gateway, PlayHint, Tools};
use xorvio_model::{LiveStreamFormat, PlaybackMode, ResolvedStream};

/// The encoder arguments of each kind of live source.
fn encoding(kind: &str) -> &'static [&'static str] {
    match kind {
        "h264" => &["-c:v", "libx264", "-preset", "ultrafast", "-g", "50", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "96k"],
        // A four-second GOP, for joining between keyframes.
        "h264-long-gop" => &["-c:v", "libx264", "-preset", "ultrafast", "-g", "100", "-pix_fmt", "yuv420p", "-c:a", "aac"],
        "hevc" => &[
            "-c:v", "libx265", "-preset", "ultrafast", "-x265-params", "keyint=50:min-keyint=50:log-level=error",
            "-pix_fmt", "yuv420p", "-c:a", "ac3", "-ac", "6", "-b:a", "192k",
        ],
        "mpeg2" => &["-c:v", "mpeg2video", "-g", "12", "-b:v", "1M", "-flags", "+ilme+ildct", "-c:a", "mp2", "-b:a", "128k"],
        // E-AC-3 the DVB way: private data with an enhanced AC-3 descriptor.
        "eac3-dvb" => &[
            "-c:v", "libx264", "-preset", "ultrafast", "-g", "50", "-pix_fmt", "yuv420p", "-c:a", "eac3",
            "-mpegts_flags", "system_b",
        ],
        other => panic!("no such source: {other}"),
    }
}

/// A live channel: every request starts ffmpeg encoding in real time, and
/// the connections are counted (the provider allows one).
#[derive(Clone)]
struct Live {
    ffmpeg: PathBuf,
    requests: Arc<AtomicUsize>,
    open: Arc<AtomicUsize>,
    most_open: Arc<AtomicUsize>,
}

impl Live {
    fn new(tools: &Tools) -> Live {
        Live {
            ffmpeg: maker(tools),
            requests: Arc::default(),
            open: Arc::default(),
            most_open: Arc::default(),
        }
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/live/{kind}", get(live))
            .route("/refused", get(|| async { StatusCode::FORBIDDEN }))
            .route("/page", get(|| async { "<html><body>Too many connections</body></html>" }))
            .with_state(self.clone())
    }
}

/// Counts a connection as open until the response body is dropped.
struct Open(Arc<AtomicUsize>);

impl Drop for Open {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(serde::Deserialize)]
struct LiveQuery {
    /// Seconds of the stream to throw away first, as if joining it then
    /// (whole packets, so it stays valid MPEG-TS).
    skip: Option<f64>,
}

async fn live(State(live): State<Live>, UrlPath(kind): UrlPath<String>, Query(query): Query<LiveQuery>) -> Response {
    live.requests.fetch_add(1, Ordering::SeqCst);
    let now = live.open.fetch_add(1, Ordering::SeqCst) + 1;
    live.most_open.fetch_max(now, Ordering::SeqCst);
    let open = Open(live.open.clone());
    let mut args: Vec<&str> = vec!["-hide_banner", "-loglevel", "error", "-re"];
    args.extend(["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25"]);
    args.extend(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"]);
    args.extend(["-t", "300"]);
    args.extend(encoding(&kind));
    args.extend(["-f", "mpegts", "pipe:1"]);
    let mut child = tokio::process::Command::new(&live.ffmpeg)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let skip_until = Instant::now() + Duration::from_secs_f64(query.skip.unwrap_or(0.0));
    struct Reading {
        _child: tokio::process::Child,
        _open: Open,
        stdout: tokio::process::ChildStdout,
        offset: usize,
        skipping: bool,
    }
    let state = Reading { _child: child, _open: open, stdout, offset: 0, skipping: query.skip.is_some() };
    let body = futures_util::stream::unfold(state, move |mut state| async move {
        loop {
            let mut buffer = vec![0u8; 32 * 1024];
            let n = state.stdout.read(&mut buffer).await.ok().filter(|&n| n > 0)?;
            buffer.truncate(n);
            let start = state.offset;
            state.offset += n;
            if !state.skipping {
                return Some((Ok::<_, std::io::Error>(Bytes::from(buffer)), state));
            }
            if Instant::now() < skip_until {
                continue;
            }
            // From the next whole packet on.
            state.skipping = false;
            let aligned = start.div_ceil(188) * 188;
            if aligned < state.offset {
                return Some((Ok(Bytes::copy_from_slice(&buffer[aligned - start..])), state));
            }
        }
    });
    Response::builder()
        .header("content-type", "video/mp2t")
        .body(Body::from_stream(body))
        .unwrap()
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// GETs the playlist, timing it (the first request waits for a segment).
async fn playlist(stream: &ResolvedStream) -> (String, Duration) {
    let started = Instant::now();
    let response = client().get(&stream.url).send().await.unwrap();
    let elapsed = started.elapsed();
    let status = response.status();
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    let content_type = response.headers()["content-type"].to_str().unwrap().to_string();
    let text = response.text().await.unwrap();
    assert!(status.is_success(), "{status}: {text}");
    assert_eq!(content_type, "application/vnd.apple.mpegurl");
    (text, elapsed)
}

fn segments(playlist: &str) -> Vec<String> {
    playlist.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect()
}

/// A file the playlist names, through the gateway (its addresses are relative).
async fn part(stream: &ResolvedStream, name: &str) -> Vec<u8> {
    let url = format!("{}/{name}", stream.url.rsplit_once('/').unwrap().0);
    let response = client().get(&url).send().await.unwrap();
    assert!(response.status().is_success(), "{} for {name}", response.status());
    assert_eq!(response.headers()["content-type"], "video/mp4");
    response.bytes().await.unwrap().to_vec()
}

/// What ffprobe reads at a gateway address (off the runtime, which serves it).
async fn probe_url(tools: &Tools, url: &str) -> ProbeSummary {
    let (tools, url) = (tools.clone(), url.to_string());
    tokio::task::spawn_blocking(move || probe(&tools, &url)).await.unwrap()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// How many ffmpeg processes write into `folder`. `None` on Windows, which
/// has no pgrep; there the folder going stands in for ffmpeg stopping, since
/// a file ffmpeg still has open can't be deleted.
fn ffmpeg_writing(folder: &Path) -> Option<usize> {
    if cfg!(windows) {
        return None;
    }
    let output = std::process::Command::new("pgrep").args(["-f", &folder.to_string_lossy()]).output().unwrap();
    Some(String::from_utf8_lossy(&output.stdout).split_whitespace().count())
}

/// Releases the stream and checks everything behind it goes: ffmpeg, the
/// folder and the provider connection.
async fn release_and_check(gateway: &Gateway, stream: &ResolvedStream, live: Option<&Live>) {
    let folder = gateway.timeshift_folder(&stream.url).expect("a buffered stream has a folder");
    assert!(folder.exists());
    assert!(ffmpeg_writing(&folder).is_none_or(|n| n == 1), "one ffmpeg writes the buffer");
    gateway.release(&stream.url);
    let deadline = Instant::now() + Duration::from_secs(5);
    while (folder.exists()
        || ffmpeg_writing(&folder).is_some_and(|n| n > 0)
        || live.is_some_and(|l| l.open.load(Ordering::SeqCst) > 0))
        && Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!folder.exists(), "the folder goes");
    assert!(ffmpeg_writing(&folder).is_none_or(|n| n == 0), "ffmpeg is stopped");
    if let Some(live) = live {
        assert_eq!(live.open.load(Ordering::SeqCst), 0, "the provider connection is closed");
    }
    let gone = client().get(&stream.url).send().await.unwrap();
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

struct Started {
    gateway: Arc<Gateway>,
    live: Live,
    origin: String,
    stream: ResolvedStream,
    playlist: String,
    first_segment: Duration,
}

/// Starts a buffer of a live source of `kind` and waits for its first segment.
async fn buffer(tools: &Tools, kind: &str, query: &str, window: f64) -> Started {
    let live = Live::new(tools);
    let dir = tempfile::tempdir().unwrap();
    let origin = serve(dir.path(), live.routes()).await;
    let gateway = Gateway::start(Some(tools.clone())).await.unwrap();
    let source = format!("{origin}/live/{kind}?token={KEY}{query}");
    let started = Instant::now();
    let stream = gateway.timeshift(target(&source, true), PlayHint::default(), window).await.unwrap();
    // It returns once the first segment is there, so the playlist has one at once.
    let first_segment = started.elapsed();
    let (playlist, waited) = playlist(&stream).await;
    assert!(waited < Duration::from_millis(500), "{waited:?}");
    assert!(!segments(&playlist).is_empty(), "{playlist}");
    println!("{kind}: first segment {:.2}s after asking", first_segment.as_secs_f64());
    Started { gateway, live, origin, stream, playlist, first_segment }
}

fn check_addresses(started: &Started) {
    let port = started.origin.rsplit(':').next().unwrap();
    let stream = &started.stream;
    assert!(stream.url.starts_with("http://127.0.0.1:") && stream.url.ends_with("/index.m3u8"));
    for text in [stream.url.as_str(), started.playlist.as_str()] {
        assert!(!text.contains(KEY) && !text.contains("/live/") && !text.contains(&format!(":{port}")), "{text}");
    }
}

fn check_resolved(stream: &ResolvedStream, window: f64) {
    assert_eq!(stream.mode, PlaybackMode::Hls);
    assert!(stream.is_live);
    assert_eq!(stream.duration_seconds, None);
    assert_eq!(stream.start_offset_seconds, 0.0);
    assert!(stream.audio_tracks.is_empty() && stream.subtitle_tracks.is_empty());
    assert_eq!(stream.timeshift.map(|t| t.window_seconds), Some(window));
    let json = serde_json::to_value(stream).unwrap();
    assert_eq!(json["timeshift"]["windowSeconds"], window);
}

#[tokio::test(flavor = "multi_thread")]
async fn h264_and_aac_are_buffered_from_one_connection() {
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx264") {
        return;
    }
    let started = buffer(&tools, "h264", "", 1800.0).await;
    check_resolved(&started.stream, 1800.0);
    check_addresses(&started);
    let info = started.stream.info.clone().unwrap();
    assert_eq!((info.container.as_str(), info.video.as_deref(), info.audio.as_deref()), ("MPEG-TS", Some("H.264"), Some("AAC")));
    assert!(info.notes.is_empty(), "copied as it is: {:?}", info.notes);
    assert!(started.first_segment < Duration::from_secs(6), "{:?}", started.first_segment);

    // A sliding live playlist of fMP4 with relative addresses.
    let text = &started.playlist;
    assert!(text.contains("#EXT-X-MAP:URI=\"init.mp4\"") && text.contains("#EXT-X-PROGRAM-DATE-TIME"), "{text}");
    assert!(!text.contains("#EXT-X-ENDLIST") && !text.contains("#EXT-X-PLAYLIST-TYPE"), "{text}");
    let names = segments(text);
    assert!(!names.is_empty() && names.iter().all(|n| n.starts_with("seg") && n.ends_with(".m4s")), "{text}");
    let init = part(&started.stream, "init.mp4").await;
    assert!(contains(&init, b"avc1") && contains(&init, b"mp4a"));
    let segment = part(&started.stream, &names[0]).await;
    assert!(contains(&segment, b"moof") && contains(&segment, b"mdat"));
    let missing = client().get(started.stream.url.replace("index.m3u8", "seg999999.m4s")).send().await.unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(missing.text().await.unwrap(), "That part of the buffer has gone.");

    // ffmpeg reads the buffer back as H.264 and AAC.
    let summary = probe_url(&tools, &started.stream.url).await;
    assert_eq!((summary.video.as_deref(), summary.audio.as_deref()), (Some("h264"), Some("aac")));

    assert_eq!(started.live.requests.load(Ordering::SeqCst), 1, "codecs were read from the one connection");
    release_and_check(&started.gateway, &started.stream, Some(&started.live)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn hevc_is_copied_as_hvc1_and_ac3_as_it_is() {
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx265") || !has_encoder(&tools, "ac3") {
        return;
    }
    let started = buffer(&tools, "hevc", "", 900.0).await;
    check_resolved(&started.stream, 900.0);
    let info = started.stream.info.clone().unwrap();
    assert_eq!((info.video.as_deref(), info.audio.as_deref()), (Some("HEVC"), Some("AC-3")));
    let init = part(&started.stream, "init.mp4").await;
    assert!(contains(&init, b"hvc1") && !contains(&init, b"hev1"), "WebKit's HLS player needs hvc1");
    if cfg!(target_os = "macos") {
        assert!(info.notes.is_empty(), "WebKit plays Dolby audio: {:?}", info.notes);
        assert!(contains(&init, b"ac-3") && contains(&init, b"dac3"));
    }
    release_and_check(&started.gateway, &started.stream, Some(&started.live)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mpeg2_and_mp2_are_converted() {
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx264") || !has_encoder(&tools, "mp2") {
        return;
    }
    let started = buffer(&tools, "mpeg2", "", 1800.0).await;
    let info = started.stream.info.clone().unwrap();
    assert_eq!((info.video.as_deref(), info.audio.as_deref()), (Some("MPEG-2"), Some("MPEG audio")));
    assert_eq!(info.notes, ["Video encoded to H.264", "Audio converted to AAC stereo"]);
    let summary = probe_url(&tools, &started.stream.url).await;
    assert_eq!((summary.video.as_deref(), summary.audio.as_deref()), (Some("h264"), Some("aac")));
    release_and_check(&started.gateway, &started.stream, Some(&started.live)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dvb_eac3_is_found_by_its_descriptor() {
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx264") || !has_encoder(&tools, "eac3") {
        return;
    }
    let started = buffer(&tools, "eac3-dvb", "", 1800.0).await;
    let info = started.stream.info.clone().unwrap();
    assert_eq!(info.audio.as_deref(), Some("E-AC-3"));
    let summary = probe_url(&tools, &started.stream.url).await;
    let expected = if cfg!(target_os = "macos") { "eac3" } else { "aac" };
    assert_eq!(summary.audio.as_deref(), Some(expected));
    release_and_check(&started.gateway, &started.stream, Some(&started.live)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn joining_between_keyframes_starts_at_the_next_one() {
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx264") {
        return;
    }
    // Joined a second into a four-second GOP: three seconds to the next keyframe.
    let started = buffer(&tools, "h264-long-gop", "&skip=1", 1800.0).await;
    assert!(started.first_segment < Duration::from_secs(10), "{:?}", started.first_segment);
    let summary = probe_url(&tools, &started.stream.url).await;
    assert_eq!(summary.video.as_deref(), Some("h264"));
    release_and_check(&started.gateway, &started.stream, Some(&started.live)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_window_slides_and_old_segments_leave_the_disk() {
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx264") {
        return;
    }
    // Ten seconds: five segments.
    let started = buffer(&tools, "h264", "", 10.0).await;
    check_resolved(&started.stream, 10.0);
    let folder = started.gateway.timeshift_folder(&started.stream.url).unwrap();
    let sequence = |text: &str| -> u64 {
        text.lines()
            .find_map(|l| l.strip_prefix("#EXT-X-MEDIA-SEQUENCE:"))
            .and_then(|n| n.parse().ok())
            .unwrap()
    };
    let deadline = Instant::now() + Duration::from_secs(40);
    // ffmpeg keeps one segment past the window (for a player still reading
    // it), so the first is gone once two have left.
    let text = loop {
        let (text, _) = playlist(&started.stream).await;
        if sequence(&text) >= 2 {
            break text;
        }
        assert!(Instant::now() < deadline, "the window never slid: {text}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    let names = segments(&text);
    assert_eq!(names.len(), 5, "{text}");
    assert_eq!(names[0], format!("seg{}.m4s", sequence(&text)), "{text}");
    assert!(!folder.join("seg0.m4s").exists(), "segments out of the window are deleted");
    let on_disk = std::fs::read_dir(&folder).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".m4s")).count();
    assert!(on_disk <= names.len() + 2, "{on_disk} segments on disk");
    let gone = client().get(started.stream.url.replace("index.m3u8", "seg0.m4s")).send().await.unwrap();
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);

    // Disk use: the segments' bytes over their seconds.
    let seconds: f64 = text.lines().filter_map(|l| l.strip_prefix("#EXTINF:")).filter_map(|d| d.trim_end_matches(',').parse::<f64>().ok()).sum();
    let bytes: u64 = names.iter().map(|n| std::fs::metadata(folder.join(n)).map(|m| m.len()).unwrap_or(0)).sum();
    println!("disk use for the 320x240 test source: {:.1} MB per minute", bytes as f64 / seconds * 60.0 / 1e6);
    release_and_check(&started.gateway, &started.stream, Some(&started.live)).await;
}

/// Quitting stops buffers then and there: the tasks that would otherwise
/// stop ffmpeg and remove the folder don't run once Xorvio has ended.
#[tokio::test(flavor = "multi_thread")]
async fn quitting_stops_every_buffer_at_once() {
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx264") {
        return;
    }
    let started = buffer(&tools, "h264", "", 600.0).await;
    let folder = started.gateway.timeshift_folder(&started.stream.url).unwrap();
    assert!(ffmpeg_writing(&folder).is_none_or(|n| n == 1));
    started.gateway.release_all();
    assert!(!folder.exists(), "the folder goes before release_all returns");
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while ffmpeg_writing(&folder).is_some_and(|n| n > 0) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(ffmpeg_writing(&folder).is_none_or(|n| n == 0), "ffmpeg is stopped");
    let gone = client().get(&started.stream.url).send().await.unwrap();
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn hls_sources_are_read_by_ffmpeg_itself() {
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx264") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // A live HLS channel, made as it plays.
    let mut source = tokio::process::Command::new(maker(&tools))
        .args(["-hide_banner", "-loglevel", "error", "-re"])
        .args(["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
        .args(["-t", "300", "-c:v", "libx264", "-preset", "ultrafast", "-g", "25", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .args(["-f", "hls", "-hls_time", "1", "-hls_list_size", "6", "-hls_flags", "delete_segments"])
        .arg("-hls_segment_filename")
        .arg(dir.path().join("chunk%d.ts"))
        .arg(dir.path().join("live.m3u8"))
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let source_playlist = dir.path().join("live.m3u8");
    let deadline = Instant::now() + Duration::from_secs(15);
    while std::fs::read_to_string(&source_playlist).map_or(0, |t| segments(&t).len()) < 3 {
        assert!(Instant::now() < deadline, "the source playlist never filled");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    std::fs::write(
        dir.path().join("master.m3u8"),
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=400000,CODECS=\"avc1.42c00d,mp4a.40.2\"\nlive.m3u8\n",
    )
    .unwrap();
    let live = Live::new(&tools);
    let origin = serve(dir.path(), live.routes()).await;
    let gateway = Gateway::start(Some(tools.clone())).await.unwrap();

    for name in ["live.m3u8", "master.m3u8"] {
        let started = Instant::now();
        let source = format!("{origin}/files/{name}?token={KEY}");
        let hint = PlayHint { live_format: Some(LiveStreamFormat::Hls), ..PlayHint::default() };
        let stream = gateway.timeshift(target(&source, true), hint, 600.0).await.unwrap();
        let (text, _) = playlist(&stream).await;
        println!("HLS source ({name}): first segment {:.2}s after asking", started.elapsed().as_secs_f64());
        check_resolved(&stream, 600.0);
        assert!(!stream.url.contains(KEY) && !text.contains(&origin) && !text.contains("chunk"), "{text}");
        let info = stream.info.clone().unwrap();
        assert_eq!((info.container.as_str(), info.video.as_deref(), info.audio.as_deref()), ("HLS", Some("H.264"), Some("AAC")));
        let summary = probe_url(&tools, &stream.url).await;
        assert_eq!((summary.video.as_deref(), summary.audio.as_deref()), (Some("h264"), Some("aac")));
        release_and_check(&gateway, &stream, None).await;
    }
    let _ = source.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pausing_a_channel_playing_directly_moves_it_to_a_buffer() {
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx264") {
        return;
    }
    let live = Live::new(&tools);
    let dir = tempfile::tempdir().unwrap();
    let origin = serve(dir.path(), live.routes()).await;
    let gateway = Gateway::start(Some(tools.clone())).await.unwrap();
    let source = format!("{origin}/live/h264?token={KEY}");
    let hint = PlayHint { live_format: Some(LiveStreamFormat::Mpegts), ..PlayHint::default() };
    let direct = gateway.prepare(target(&source, true), hint).await.unwrap();
    assert_eq!(direct.mode, PlaybackMode::Mpegts);
    assert_eq!(direct.timeshift, None);

    // The player reads the channel directly.
    let mut body = client().get(&direct.url).send().await.unwrap().bytes_stream();
    let first = body.next().await.unwrap().unwrap();
    assert_eq!(first[0], 0x47);
    let reading = tokio::spawn(async move {
        let mut read = 0;
        while let Some(Ok(chunk)) = body.next().await {
            read += chunk.len();
        }
        read
    });
    tokio::time::sleep(Duration::from_millis(500)).await;

    let started = Instant::now();
    let buffered = gateway.timeshift_playing(&direct.url, 1800.0).await.unwrap();
    // The direct stream ended when it was released, before the buffer connected.
    tokio::time::timeout(Duration::from_secs(2), reading).await.expect("the direct stream ends").unwrap();
    check_resolved(&buffered, 1800.0);
    assert_ne!(buffered.url, direct.url);
    let (text, _) = playlist(&buffered).await;
    println!("paused: buffer's first segment {:.2}s after the switch", started.elapsed().as_secs_f64());
    assert!(!segments(&text).is_empty());
    assert_eq!(live.requests.load(Ordering::SeqCst), 2);
    assert_eq!(live.most_open.load(Ordering::SeqCst), 1, "never two connections at once");
    assert_eq!(client().get(&direct.url).send().await.unwrap().status(), StatusCode::NOT_FOUND);

    // Asking again for a stream already buffered changes nothing.
    let again = gateway.timeshift_playing(&buffered.url, 1800.0).await.unwrap();
    assert_eq!(again.url, buffered.url);
    assert_eq!(live.requests.load(Ordering::SeqCst), 2);

    // Playing the same channel buffered again replaces the older buffer.
    let replaced = gateway.timeshift(target(&source, true), PlayHint::default(), 1800.0).await.unwrap();
    assert_eq!(client().get(&buffered.url).send().await.unwrap().status(), StatusCode::NOT_FOUND);
    assert_eq!(live.most_open.load(Ordering::SeqCst), 1, "never two connections at once");
    release_and_check(&gateway, &replaced, Some(&live)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_come_back_as_plain_errors() {
    let Some(tools) = tools() else { return };
    let live = Live::new(&tools);
    let dir = tempfile::tempdir().unwrap();
    let origin = serve(dir.path(), live.routes()).await;
    let gateway = Gateway::start(Some(tools.clone())).await.unwrap();
    let error = |result: anyhow::Result<ResolvedStream>| result.unwrap_err().to_string();

    let refused = gateway.timeshift(target(&format!("{origin}/refused"), true), PlayHint::default(), 1800.0).await;
    assert_eq!(error(refused), "The provider refused the stream.");
    let page = gateway.timeshift(target(&format!("{origin}/page"), true), PlayHint::default(), 1800.0).await;
    assert_eq!(error(page), "The provider sent something that isn't a video stream.");
    let film = gateway.timeshift(target(&format!("{origin}/live/h264"), false), PlayHint::default(), 1800.0).await;
    assert_eq!(error(film), "Only live channels can be paused and rewound.");
    let without = Gateway::start(None).await.unwrap();
    let missing = without.timeshift(target(&format!("{origin}/live/h264"), true), PlayHint::default(), 1800.0).await;
    assert!(error(missing).contains("ffmpeg"));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn ffmpeg_failing_early_is_an_error_rather_than_a_hang() {
    use std::os::unix::fs::PermissionsExt;
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx264") {
        return;
    }
    let bin = tempfile::tempdir().unwrap();
    let broken = bin.path().join("ffmpeg");
    std::fs::write(
        &broken,
        "#!/bin/sh\necho '[in#0/mpegts @ 0x1] Error opening input: Invalid data found when processing input' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();
    let live = Live::new(&tools);
    let dir = tempfile::tempdir().unwrap();
    let origin = serve(dir.path(), live.routes()).await;
    let gateway = Gateway::start(Some(Tools { ffmpeg: broken, ffprobe: tools.ffprobe.clone() })).await.unwrap();
    let started = Instant::now();
    let result = gateway
        .timeshift(target(&format!("{origin}/live/h264?token={KEY}"), true), PlayHint::default(), 1800.0)
        .await;
    assert!(started.elapsed() < Duration::from_secs(4), "it doesn't wait out the limit");
    assert_eq!(
        result.unwrap_err().to_string(),
        "Xorvio couldn't buffer this channel (Error opening input: Invalid data found when processing input)."
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while live.open.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(live.open.load(Ordering::SeqCst), 0, "the provider connection is closed");
}

// MARK: WebKit

/// Buffers of four kinds of source for a player to try: HEVC (copied as
/// hvc1) with 5.1 AC-3, H.264 with AAC, MPEG-2 and MP2 (converted), and
/// H.264 with DVB E-AC-3. `XORVIO_PLAYER_KINDS` picks fewer.
#[cfg(target_os = "macos")]
async fn buffers_to_play(tools: &Tools) -> (Arc<Gateway>, tempfile::TempDir, Vec<(String, String)>) {
    let live = Live::new(tools);
    let dir = tempfile::tempdir().unwrap();
    let origin = serve(dir.path(), live.routes()).await;
    let gateway = Gateway::start(Some(tools.clone())).await.unwrap();
    let kinds = std::env::var("XORVIO_PLAYER_KINDS").unwrap_or_else(|_| "hevc,h264,mpeg2,eac3-dvb".into());
    let mut streams = Vec::new();
    for kind in kinds.split(',') {
        let source = format!("{origin}/live/{kind}?token={KEY}");
        let stream = gateway.timeshift(target(&source, true), PlayHint::default(), 1800.0).await.unwrap();
        println!("{kind}: {:?}", stream.info);
        streams.push((kind.to_string(), stream.url));
    }
    (gateway, dir, streams)
}

/// What a player did with one buffer, from its reports: one a second
/// ("tick"), with "paused" at 10 s, "resumed" at 16 s and "back" (to the
/// start of the window) at 22 s. Each has `t`, `time`, `width`, `seekable`
/// and `error`. Returns what went wrong.
#[cfg(target_os = "macos")]
fn judge(name: &str, reports: &[serde_json::Value], has_sound: impl Fn(&serde_json::Value) -> bool) -> Vec<String> {
    use serde_json::Value;
    let mine: Vec<&Value> = reports.iter().filter(|r| r["name"] == name).collect();
    let at = |seconds: f64| mine.iter().rev().find(|r| r["event"] == "tick" && r["t"].as_f64().unwrap_or(0.0) <= seconds).copied();
    let time = |r: Option<&Value>| r.and_then(|r| r["time"].as_f64()).unwrap_or(-1.0);
    let window = |r: Option<&Value>| {
        let range = r?["seekable"].as_array()?.first()?.as_array()?.clone();
        Some((range[0].as_f64()?, range[1].as_f64()?))
    };
    let errors: Vec<&&Value> = mine.iter().filter(|r| !r["error"].is_null()).collect();
    let first = mine.iter().find(|r| r["event"] == "tick" && time(Some(r)) > 0.0).map(|r| r["t"].as_f64().unwrap_or(-1.0));
    let paused = mine.iter().find(|r| r["event"] == "paused").copied();
    let (before_resume, resumed, end) = (at(15.9), at(20.5), at(29.5));
    let width = end.and_then(|r| r["width"].as_f64()).unwrap_or(0.0);
    println!(
        "{name}: width {width} · playing by {:.0} s · paused at {:.1}, still {:.1} at 15.9 s · {:.1} at 20.5 s (4.5 s after resuming) · {:.1} at 29.5 s (sent back to the start at 22 s) · window {:?} · errors {}",
        first.unwrap_or(-1.0),
        time(paused),
        time(before_resume),
        time(resumed),
        time(end),
        window(end),
        errors.len()
    );
    let played_on = time(resumed) - time(paused);
    let checks = [
        (errors.is_empty(), "plays without errors"),
        (width == 320.0, "shows a picture"),
        (time(paused) > 0.5, "played before the pause"),
        ((time(before_resume) - time(paused)).abs() < 0.5, "stayed paused"),
        // From where it was paused: four seconds on, not ten (as if it had gone on live).
        ((2.0..6.0).contains(&played_on), "resumed where it was paused"),
        (window(end).is_some_and(|(start, end)| end - start > 15.0), "has a seekable window that grows"),
        (time(end) < 12.0, "went back to the start of the window"),
        (end.is_some_and(has_sound), "has its sound"),
    ];
    if checks.iter().any(|(ok, _)| !ok) {
        for r in &mine {
            println!("  {r}");
        }
    }
    checks.iter().filter(|(ok, _)| !ok).map(|(_, what)| format!("{name} {what}")).collect()
}

/// AVFoundation (WebKit's HLS player on macOS) plays, pauses and rewinds the
/// buffers, with their sound tracks recognised. No window: it runs a Swift
/// script with AVPlayer for half a minute.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "plays through AVFoundation for half a minute"]
async fn avfoundation_plays_pauses_and_rewinds_the_buffer() {
    let Some(tools) = tools() else { return };
    let (gateway, _dir, streams) = buffers_to_play(&tools).await;
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/avplayer.swift");
    let output = tokio::process::Command::new("xcrun")
        .arg("swift")
        .arg(&script)
        .args(streams.iter().map(|(name, url)| format!("{name}={url}")))
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{}{stdout}", String::from_utf8_lossy(&output.stderr));
    let reports: Vec<serde_json::Value> = stdout.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let mut failures = Vec::new();
    for (name, url) in &streams {
        let last = reports.iter().rev().find(|r| r["name"] == name.as_str());
        println!("{name}: tracks {}", last.map_or(serde_json::Value::Null, |r| r["tracks"].clone()));
        let sound = |r: &serde_json::Value| {
            r["tracks"].as_array().is_some_and(|t| t.iter().any(|t| t.as_str().is_some_and(|t| t.starts_with("soun:"))))
        };
        failures.extend(judge(name, &reports, sound));
        gateway.release(url);
    }
    assert!(failures.is_empty(), "{failures:?}");
}

/// The page for Safari: a muted `<video>` per buffer, played by WebKit's own
/// HLS player, reporting as [`judge`] reads.
#[cfg(target_os = "macos")]
const PAGE: &str = r#"<!doctype html><meta charset="utf-8"><title>Xorvio live buffer check</title>
<body style="background:#111;color:#eee;font:12px system-ui">
<script>
const streams = STREAMS;
const started = Date.now();
const ranges = (r) => Array.from({ length: r.length }, (_, i) => [r.start(i), r.end(i)]);
function report(name, v, event) {
  const body = {
    name, event, t: (Date.now() - started) / 1000, time: v.currentTime, width: v.videoWidth, seekable: ranges(v.seekable),
    error: v.error ? v.error.code + " " + v.error.message : null, visible: document.visibilityState,
    audioTracks: v.audioTracks ? v.audioTracks.length : 0,
  };
  fetch("/report", { method: "POST", body: JSON.stringify(body) }).catch(() => {});
}
const videos = streams.map(([name, url]) => {
  const v = document.createElement("video");
  Object.assign(v, { muted: true, autoplay: true, playsInline: true, src: url, width: 320 });
  v.addEventListener("error", () => report(name, v, "error"));
  document.body.append(name, v);
  v.play().catch(() => {});
  return [name, v];
});
setInterval(() => videos.forEach(([n, v]) => report(n, v, "tick")), 1000);
setTimeout(() => videos.forEach(([n, v]) => { v.pause(); report(n, v, "paused"); }), 10000);
setTimeout(() => videos.forEach(([n, v]) => { v.play(); report(n, v, "resumed"); }), 16000);
setTimeout(() => videos.forEach(([n, v]) => { if (v.seekable.length) v.currentTime = v.seekable.start(0) + 1; report(n, v, "back"); }), 22000);
</script>"#;

/// The same in Safari itself, through `<video>`. Safari holds back video in
/// a page that isn't showing, so it has to stay in front for the half minute
/// (the reports say `"visible": "hidden"` when it didn't); the tab closes after.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "opens Safari, which has to stay in front for half a minute"]
async fn safari_plays_pauses_and_rewinds_the_buffer() {
    use std::sync::Mutex;
    let Some(tools) = tools() else { return };
    let (gateway, _dir, streams) = buffers_to_play(&tools).await;
    let reports: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let page = PAGE.replace("STREAMS", &serde_json::to_string(&streams).unwrap());
    let collected = reports.clone();
    let app = Router::new().route("/page", get(move || async move { axum::response::Html(page) })).route(
        "/report",
        axum::routing::post(move |body: String| async move {
            if let Ok(value) = serde_json::from_str(&body) {
                collected.lock().unwrap().push(value);
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    std::process::Command::new("open").args(["-a", "Safari", &format!("http://127.0.0.1:{port}/page")]).status().unwrap();
    tokio::time::sleep(Duration::from_secs(30)).await;
    let close = format!("tell application \"Safari\" to close (every tab of every window whose URL contains \"127.0.0.1:{port}\")");
    let _ = std::process::Command::new("osascript").args(["-e", &close]).status();

    let reports = reports.lock().unwrap().clone();
    let mut failures = Vec::new();
    for (name, url) in &streams {
        failures.extend(judge(name, &reports, |r| r["audioTracks"].as_f64().unwrap_or(0.0) >= 1.0));
        gateway.release(url);
    }
    assert!(failures.is_empty(), "{failures:?}");
}

