//! Fixtures for the gateway and recorder tests: media generated with ffmpeg
//! into temporary folders, served by a local HTTP server that insists on the
//! headers a provider would. Nothing reaches the internet.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;
use tower_http::services::ServeDir;
use xorvio_media::Tools;
use xorvio_model::StreamTarget;

pub const AGENT: &str = "TestAgent/1";
pub const KEY_HEADER: &str = "x-test-key";
pub const KEY: &str = "s3cret-key";

/// ffmpeg and ffprobe, or `None` (and a note) so a test can skip itself.
pub fn tools() -> Option<Tools> {
    let tools = Tools::locate();
    if tools.is_none() {
        eprintln!("ffmpeg isn't installed; skipping");
    }
    tools
}

/// The ffmpeg that makes test media. With `XORVIO_FFMPEG_DIR` the code under
/// test runs Xorvio's own build, which holds only what the app uses (no test
/// sources, no MPEG-2 or AC-3 encoder), so the media is made by the ffmpeg
/// installed on this computer instead.
pub fn maker(tools: &Tools) -> PathBuf {
    let Some(dir) = std::env::var_os(Tools::DIR_VARIABLE).filter(|d| !d.is_empty()) else {
        return tools.ffmpeg.clone();
    };
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let homebrew = ["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from);
    let paths = std::env::var_os("PATH").unwrap_or_default();
    let under_test = std::fs::canonicalize(&dir).ok();
    std::env::split_paths(&paths)
        .chain(homebrew)
        .filter(|candidate| std::fs::canonicalize(candidate).ok() != under_test)
        .map(|candidate| candidate.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("making test media needs an ffmpeg besides the one in {}", Tools::DIR_VARIABLE))
}

/// A full ffmpeg and ffprobe for looking at what the app made (a JPEG sheet,
/// say), which Xorvio's own slim build needn't read: the pair beside
/// [`maker`]'s ffmpeg, or the ones under test when they're the system's.
pub fn inspector(tools: &Tools) -> Tools {
    let ffmpeg = maker(tools);
    let name = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };
    let ffprobe = ffmpeg.with_file_name(name);
    if ffprobe.is_file() { Tools { ffmpeg, ffprobe } } else { Tools { ffmpeg, ffprobe: tools.ffprobe.clone() } }
}

/// Whether the ffmpeg that makes test media has an encoder. The ones the app
/// itself encodes with (libx264, AAC) are in every build it bundles.
pub fn has_encoder(tools: &Tools, name: &str) -> bool {
    let output = Command::new(maker(tools))
        .args(["-hide_banner", "-encoders"])
        .output();
    output.is_ok_and(|o| {
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .any(|l| l.split_whitespace().nth(1) == Some(name))
    })
}

/// Runs ffmpeg with test sources; panics with its message on failure.
pub fn ffmpeg(tools: &Tools, args: &[&str]) {
    let output = Command::new(maker(tools))
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .output()
        .expect("ffmpeg runs");
    assert!(
        output.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub const VIDEO_SOURCE: [&str; 4] = ["-f", "lavfi", "-i", "testsrc=size=160x120:rate=25"];
pub const AUDIO_SOURCE: [&str; 4] = ["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"];

/// `seconds` of test video and tone as MPEG-TS (MPEG-2 video, AAC), which
/// every ffmpeg build can make.
pub fn make_ts(tools: &Tools, path: &Path, seconds: u32) {
    let duration = seconds.to_string();
    let mut args: Vec<&str> = VIDEO_SOURCE.to_vec();
    args.extend(AUDIO_SOURCE);
    args.extend([
        "-t",
        &duration,
        "-c:v",
        "mpeg2video",
        "-b:v",
        "300k",
        "-g",
        "12",
        "-c:a",
        "aac",
        "-b:a",
        "64k",
        "-f",
        "mpegts",
    ]);
    let path = path.to_string_lossy();
    args.push(&path);
    ffmpeg(tools, &args);
}

#[derive(Debug, Default)]
pub struct ProbeSummary {
    pub format: String,
    pub duration: f64,
    pub video: Option<String>,
    pub audio: Option<String>,
}

/// What ffprobe says about a file or address.
pub fn probe(tools: &Tools, input: &str) -> ProbeSummary {
    let output = Command::new(&tools.ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            input,
        ])
        .output()
        .expect("ffprobe runs");
    assert!(
        output.status.success(),
        "ffprobe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let codec = |kind: &str| {
        value["streams"]
            .as_array()?
            .iter()
            .find(|s| s["codec_type"] == kind)
            .and_then(|s| s["codec_name"].as_str())
            .map(String::from)
    };
    ProbeSummary {
        format: value["format"]["format_name"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        duration: value["format"]["duration"]
            .as_str()
            .and_then(|d| d.parse().ok())
            .unwrap_or(0.0),
        video: codec("video"),
        audio: codec("audio"),
    }
}

/// A target that needs the test headers to be served.
pub fn target(url: &str, is_live: bool) -> StreamTarget {
    StreamTarget {
        url: url.to_string(),
        user_agent: Some(AGENT.to_string()),
        headers: vec![("X-Test-Key".to_string(), KEY.to_string())],
        is_live,
    }
}

/// Refuses requests without the provider headers, as a picky provider would.
async fn require_headers(request: Request, next: Next) -> Response {
    let headers = request.headers();
    let agent_ok = headers.get("user-agent").is_some_and(|v| v == AGENT);
    let key_ok = headers.get(KEY_HEADER).is_some_and(|v| v == KEY);
    if agent_ok && key_ok {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

/// Serves `dir` under `/files/` (with ranges) and `extra` routes, all behind
/// the header check. Returns the server's origin.
pub async fn serve(dir: &Path, extra: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .nest_service("/files", ServeDir::new(dir))
        .merge(extra)
        .layer(middleware::from_fn(require_headers));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    origin
}

/// A recorded stream played out at its real pace, as a live channel sends it.
#[derive(Clone)]
pub struct Paced {
    pub bytes: Arc<Vec<u8>>,
    pub seconds: f64,
    /// Ends the response after this many seconds and refuses later requests: a drop.
    pub drop_after: Option<f64>,
    pub requests: Arc<AtomicUsize>,
}

impl Paced {
    pub fn new(file: &Path, seconds: f64, drop_after: Option<f64>) -> Paced {
        Paced {
            bytes: Arc::new(std::fs::read(file).unwrap()),
            seconds,
            drop_after,
            requests: Arc::default(),
        }
    }

    pub fn route(self, path: &str) -> Router {
        Router::new().route(path, get(paced)).with_state(self)
    }
}

async fn paced(State(paced): State<Paced>) -> Response {
    let attempt = paced.requests.fetch_add(1, Ordering::SeqCst);
    if paced.drop_after.is_some() && attempt > 0 {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let rate = paced.bytes.len() as f64 / paced.seconds;
    let end = paced.drop_after.map_or(paced.bytes.len(), |s| {
        ((rate * s) as usize).min(paced.bytes.len())
    });
    let per_tick = ((rate / 10.0) as usize).max(188);
    let bytes = paced.bytes.clone();
    let body = futures_util::stream::unfold(0usize, move |offset| {
        let bytes = bytes.clone();
        async move {
            if offset >= end {
                return None;
            }
            if offset > 0 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let next = (offset + per_tick).min(end);
            Some((
                Ok::<_, std::io::Error>(Bytes::copy_from_slice(&bytes[offset..next])),
                next,
            ))
        }
    });
    Response::builder()
        .header("content-type", "video/mp2t")
        .body(Body::from_stream(body))
        .unwrap()
}

pub fn temp_path(dir: &tempfile::TempDir, name: &str) -> PathBuf {
    dir.path().join(name)
}
