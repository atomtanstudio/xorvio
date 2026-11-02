//! Seek-bar thumbnails: made from generated media on this computer and on a
//! local server standing in for a provider, served by the gateway, kept in
//! a size-capped folder, and never costing a provider a connection it
//! doesn't allow.

mod common;

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::serve::ListenerExt;
use common::*;
use futures_util::StreamExt;
use tower_http::services::ServeDir;
use xorvio_media::{ConnectionCheck, Gateway, PlayHint, ThumbnailSheet, Thumbnails};
use xorvio_model::{PlaybackMode, StreamTarget};

fn allowed() -> ConnectionCheck {
    Arc::new(|_| Ok(()))
}

fn file_target(path: &Path) -> StreamTarget {
    StreamTarget {
        url: path.to_string_lossy().into_owned(),
        user_agent: None,
        headers: vec![],
        is_live: false,
    }
}

/// Asks until the thumbnails are ready, as the player does.
async fn ready(gateway: &Gateway, url: &str, item: &str, check: &ConnectionCheck) -> ThumbnailSheet {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match gateway.thumbnails(url, item, check.clone()) {
            Thumbnails::Ready { sheet } => return sheet,
            Thumbnails::Making => {}
            Thumbnails::Unavailable { reason } => panic!("no thumbnails: {reason}"),
        }
        assert!(Instant::now() < deadline, "the thumbnails took too long");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Asks until a sheet stopped a moment ago has let go (its ffmpeg ends more
/// slowly on Windows), and gives the answer then.
async fn once_let_go(gateway: &Gateway, url: &str, item: &str) -> Thumbnails {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match gateway.thumbnails(url, item, allowed()) {
            Thumbnails::Making => assert!(Instant::now() < deadline, "the stopped sheet never let go"),
            other => return other,
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// One tile of a sheet, as grey pixels, cut out by ffmpeg.
fn tile_pixels(tools: &xorvio_media::Tools, sheet: &Path, layout: &ThumbnailSheet, index: usize) -> Vec<u8> {
    let (x, y) = ((index % layout.columns) as u32 * layout.width, (index / layout.columns) as u32 * layout.height);
    let output = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(sheet)
        .args(["-vf", &format!("crop={}:{}:{x}:{y}", layout.width, layout.height), "-pix_fmt", "gray", "-f", "rawvideo", "-"])
        .output()
        .unwrap();
    assert!(output.status.success());
    output.stdout
}

#[tokio::test]
async fn recordings_and_files_get_sheets_the_gateway_serves() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Harbor News - Late Edition.ts");
    make_ts(&tools, &file, 60);
    let cache = dir.path().join("Thumbnails");
    // Thumbnails of something watched long ago, which the folder's limit has no room for now.
    let old = cache.join("0123456789abcdef");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("0.jpg"), vec![0xff; 2_000_000]).unwrap();
    std::fs::write(old.join("sheet.json"), r#"{"intervalSeconds":2,"count":1,"columns":10,"rows":1,"width":160,"height":90,"sheets":1}"#).unwrap();
    let file_handle = std::fs::File::options().write(true).open(old.join("sheet.json")).unwrap();
    file_handle.set_modified(std::time::SystemTime::now() - Duration::from_secs(86_400)).unwrap();
    // Finished an hour ago (a file written moments ago is a recording still being made).
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(3600)).unwrap();
    let before = std::fs::metadata(&file).unwrap();

    let gateway = Gateway::start(Some(tools.clone())).await.unwrap();
    gateway.set_thumbnail_cache(cache.clone(), 1_000_000);
    let stream = gateway.prepare(file_target(&file), PlayHint::default()).await.unwrap();
    assert_eq!(stream.mode, PlaybackMode::Remux);
    assert_eq!(gateway.thumbnails(&stream.url, "", allowed()), Thumbnails::Making);

    let sheet = ready(&gateway, &stream.url, "", &allowed()).await;
    assert_eq!(sheet.interval_seconds, 2.0, "a minute gets a tile every 2 seconds");
    assert!((29..=30).contains(&sheet.count), "{}", sheet.count);
    assert_eq!((sheet.columns, sheet.rows, sheet.width, sheet.height), (10, 3, 160, 120), "4:3 video, 4:3 tiles");
    assert_eq!(sheet.images.len(), 1);
    assert!(!old.exists(), "the least recently used went to make room");

    let response = reqwest::get(&sheet.images[0]).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "image/jpeg");
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    let jpeg = response.bytes().await.unwrap();
    assert_eq!(&jpeg[..2], [0xff, 0xd8]);
    let saved = dir.path().join("sheet.jpg");
    std::fs::write(&saved, &jpeg).unwrap();
    // Looked at with a full ffmpeg: Xorvio's own needn't read JPEGs.
    let look = inspector(&tools);
    let size = probe(&look, &saved.to_string_lossy());
    assert_eq!(size.video.as_deref(), Some("mjpeg"));
    let dimensions = Command::new(&look.ffprobe)
        .args(["-v", "error", "-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(&saved)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&dimensions.stdout).trim(), "1600,360");
    // The test pattern moves, so tiles a few seconds apart differ.
    assert_ne!(tile_pixels(&look, &saved, &sheet, 0), tile_pixels(&look, &saved, &sheet, 12));

    // Nothing outside the thumbnails folder can be asked for.
    let base = sheet.images[0].rsplit_once("/p/").unwrap().0.to_string();
    let key = sheet.images[0].rsplit('/').nth(1).unwrap().to_string();
    for path in [format!("/p/{key}/7.jpg"), format!("/p/{key}/sheet.json"), "/p/..%2F..%2Fx/0.jpg".to_string(), "/p/zzzzzzzzzzzzzzzz/0.jpg".to_string()] {
        let status = reqwest::get(format!("{base}{path}")).await.unwrap().status();
        assert_eq!(status, 404, "{path}");
    }

    // A remuxed stream seeks by starting again at an offset: same title, same sheet.
    let seeked = gateway.seek(&stream.url, 30.0).await.unwrap();
    assert_eq!(seeked.start_offset_seconds, 30.0);
    match gateway.thumbnails(&seeked.url, "", allowed()) {
        Thumbnails::Ready { sheet: again } => assert_eq!(again, sheet),
        other => panic!("expected the same sheet, got {other:?}"),
    }

    // Once per title: a later launch finds them made.
    drop(gateway);
    let gateway = Gateway::start(Some(tools.clone())).await.unwrap();
    gateway.set_thumbnail_cache(cache.clone(), 1_000_000);
    let stream = gateway.prepare(file_target(&file), PlayHint::default()).await.unwrap();
    assert!(matches!(gateway.thumbnails(&stream.url, "", allowed()), Thumbnails::Ready { .. }));

    // The recording itself is only read.
    let after = std::fs::metadata(&file).unwrap();
    assert_eq!((after.len(), after.modified().unwrap()), (before.len(), before.modified().unwrap()));
    let names: Vec<String> = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names.len(), 3, "the recording, the thumbnails folder and the sheet saved here: {names:?}");
}

#[tokio::test]
async fn live_channels_and_recordings_still_being_made_have_none() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("show.ts");
    make_ts(&tools, &file, 30);
    let gateway = Gateway::start(Some(tools)).await.unwrap();
    gateway.set_thumbnail_cache(dir.path().join("Thumbnails"), 1 << 30);
    let unavailable = |thumbnails: Thumbnails| match thumbnails {
        Thumbnails::Unavailable { reason } => reason,
        other => panic!("expected none, got {other:?}"),
    };
    let live = gateway.prepare(StreamTarget { is_live: true, ..file_target(&file) }, PlayHint::default()).await.unwrap();
    assert!(unavailable(gateway.thumbnails(&live.url, "", allowed())).contains("Live channels"));
    // Written moments ago: a recording still being made, which the player follows.
    let growing = gateway.prepare(file_target(&file), PlayHint::default()).await.unwrap();
    assert!(unavailable(gateway.thumbnails(&growing.url, "", allowed())).contains("once the recording has finished"));
    gateway.release(&growing.url);
    assert!(unavailable(gateway.thumbnails(&growing.url, "", allowed())).contains("isn't playing"));
}

/// A stand-in provider that counts requests, and the most connections it
/// ever had open at once when one came. A connection counts until the
/// client closes it, as its socket says, not until the server gets round to
/// noticing: a close and a request arriving together can be taken in either
/// order.
#[derive(Clone, Default)]
struct Counter {
    total: Arc<AtomicUsize>,
    /// Requests the server is still answering, for waiting until they're done.
    open: Arc<AtomicUsize>,
    most: Arc<AtomicUsize>,
    /// Another handle on each connection's socket, to ask whether the client has closed it.
    sockets: Arc<Mutex<Vec<std::net::TcpStream>>>,
}

impl Counter {
    fn accepted(&self, stream: &tokio::net::TcpStream) {
        #[cfg(unix)]
        let handle = std::os::fd::AsFd::as_fd(stream).try_clone_to_owned();
        #[cfg(windows)]
        let handle = std::os::windows::io::AsSocket::as_socket(stream).try_clone_to_owned();
        let socket = std::net::TcpStream::from(handle.unwrap());
        socket.set_nonblocking(true).unwrap();
        self.sockets.lock().unwrap().push(socket);
    }

    /// The connections the client still has open.
    fn connected(&self) -> usize {
        let mut sockets = self.sockets.lock().unwrap();
        // Nothing to read, or a request not read yet: open. Its end, or a reset: closed.
        sockets.retain(|socket| match socket.peek(&mut [0]) {
            Ok(read) => read > 0,
            Err(error) => error.kind() == std::io::ErrorKind::WouldBlock,
        });
        sockets.len()
    }
}

struct Closing(Counter);

impl Drop for Closing {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn count(State(counter): State<Counter>, request: Request, next: Next) -> Response {
    counter.total.fetch_add(1, Ordering::SeqCst);
    counter.open.fetch_add(1, Ordering::SeqCst);
    counter.most.fetch_max(counter.connected(), Ordering::SeqCst);
    let closing = Closing(counter.clone());
    let response = next.run(request).await;
    let (parts, body) = response.into_parts();
    let body = body.into_data_stream().map(move |chunk| {
        let _ = &closing;
        chunk
    });
    Response::from_parts(parts, Body::from_stream(body))
}

async fn provider(dir: &Path, counter: Counter) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let watching = counter.clone();
    let listener = listener.tap_io(move |stream| watching.accepted(stream));
    let app = Router::new()
        .nest_service("/movie", ServeDir::new(dir))
        .layer(middleware::from_fn_with_state(counter, count));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    origin
}

/// Asks for a title's thumbnails and waits until they've made a couple of
/// requests, returning how many there had been before.
async fn start_making(gateway: &Gateway, url: &str, item: &str, counter: &Counter) -> usize {
    let started = counter.total.load(Ordering::SeqCst);
    assert_eq!(gateway.thumbnails(url, item, allowed()), Thumbnails::Making);
    let deadline = Instant::now() + Duration::from_secs(30);
    while counter.total.load(Ordering::SeqCst) < started + 2 {
        assert!(Instant::now() < deadline, "no tiles were taken");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    started
}

/// Waits for a stream's requests to settle, then says how many there were.
async fn settled(counter: &Counter) -> usize {
    let mut last = counter.total.load(Ordering::SeqCst);
    loop {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let now = counter.total.load(Ordering::SeqCst);
        if now == last && counter.open.load(Ordering::SeqCst) == 0 {
            return now;
        }
        last = now;
    }
}

#[tokio::test]
async fn remote_titles_never_cost_a_connection_the_provider_doesnt_allow() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    make_ts(&tools, &dir.path().join("film.ts"), 24);
    make_ts(&tools, &dir.path().join("other.ts"), 24);
    let counter = Counter::default();
    let origin = provider(dir.path(), counter.clone()).await;
    let film = StreamTarget { url: format!("{origin}/movie/film.ts?token=s3cret"), user_agent: None, headers: vec![], is_live: false };
    let other = StreamTarget { url: format!("{origin}/movie/other.ts"), ..film.clone() };

    let gateway = Gateway::start(Some(tools)).await.unwrap();
    gateway.set_thumbnail_cache(dir.path().join("Thumbnails"), 1 << 30);
    // The app waits a while after a remote title starts; no need to here.
    gateway.set_thumbnail_wait(Duration::ZERO);
    let stream = gateway.prepare(film.clone(), PlayHint::default()).await.unwrap();
    let probed = settled(&counter).await;

    // A provider that allows one stream: not one request more.
    let asked_about = Arc::new(std::sync::Mutex::new(String::new()));
    let noted = asked_about.clone();
    let one_stream: ConnectionCheck = Arc::new(move |host| {
        *noted.lock().unwrap() = host.to_string();
        xorvio_media::second_connection(xorvio_media::Allowance::Streams(1), 0)
    });
    match gateway.thumbnails(&stream.url, "src|movie|1", one_stream) {
        Thumbnails::Unavailable { reason } => assert!(reason.contains("one stream"), "{reason}"),
        other => panic!("expected none, got {other:?}"),
    }
    assert_eq!(settled(&counter).await, probed);
    assert_eq!(*asked_about.lock().unwrap(), origin.trim_start_matches("http://"), "asked with the title's host, never its address");

    // Something else playing (here, a second title): none either.
    let second = gateway.prepare(other.clone(), PlayHint::default()).await.unwrap();
    let before = settled(&counter).await;
    match gateway.thumbnails(&stream.url, "src|movie|1", allowed()) {
        Thumbnails::Unavailable { reason } => assert!(reason.contains("Something else is playing"), "{reason}"),
        other => panic!("expected none, got {other:?}"),
    }
    assert_eq!(settled(&counter).await, before);
    gateway.release(&second.url);

    // Allowed, with nothing else playing: one tile's connection at a time.
    counter.most.store(0, Ordering::SeqCst);
    let before = counter.total.load(Ordering::SeqCst);
    let sheet = ready(&gateway, &stream.url, "src|movie|1", &allowed()).await;
    assert!((11..=12).contains(&sheet.count), "{}", sheet.count);
    let full = counter.total.load(Ordering::SeqCst) - before;
    assert!(full >= sheet.count, "a request (at least) per tile");
    assert_eq!(counter.most.load(Ordering::SeqCst), 1, "never two at once");

    // The title stopping stops them before the next tile.
    let started = start_making(&gateway, &stream.url, "src|movie|2", &counter).await;
    gateway.release(&stream.url);
    let stopped = settled(&counter).await;
    assert!((stopped - started) * 2 < full, "they stopped rather than finishing: {} of {full} requests", stopped - started);

    // So does something else starting: here, the film handed to another player.
    let stream = gateway.prepare(film, PlayHint::default()).await.unwrap();
    let started = start_making(&gateway, &stream.url, "src|movie|3", &counter).await;
    gateway.external(&stream.url, "Film").unwrap();
    let stopped = settled(&counter).await;
    assert!((stopped - started) * 2 < full, "they stopped rather than finishing: {} of {full} requests", stopped - started);
    match once_let_go(&gateway, &stream.url, "src|movie|3").await {
        Thumbnails::Unavailable { reason } => assert!(reason.contains("Something else is playing"), "{reason}"),
        other => panic!("expected none while another player has it, got {other:?}"),
    }
    assert_eq!(settled(&counter).await, stopped);
}
