//! The recorder against a local server that plays out a generated stream at
//! its real pace, as a live channel would, and hands out catch-up replays of
//! it as fast as it can, as a provider's archive would.

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as RoutePath, Query, State};
use axum::response::Response;
use axum::routing::get;
use common::*;
use xorvio_media::{ChannelResolver, Recorder, RecordingFolders, Tools};
use xorvio_model::{Recording, RecordingStatus, ScheduleRequest, StreamTarget};

struct Channels {
    live: HashMap<String, String>,
    origin: String,
}

impl ChannelResolver for Channels {
    fn resolve(&self, _source_id: &str, channel_id: &str) -> anyhow::Result<StreamTarget> {
        let url = self
            .live
            .get(channel_id)
            .ok_or_else(|| anyhow::anyhow!("no such channel"))?;
        Ok(target(url, true))
    }

    // A replay address that carries a login, as Xtream's do.
    fn replay(
        &self,
        _source_id: &str,
        channel_id: &str,
        start: i64,
        end: i64,
    ) -> anyhow::Result<StreamTarget> {
        let url = format!(
            "{}/replay/{channel_id}?user=viewer&password=hunter2&start={start}&end={end}",
            self.origin
        );
        Ok(target(&url, false))
    }
}

/// The stand-in archive: every replay asked for, and the generated stream
/// sent at once. "flaky" breaks off its first replay part-way.
#[derive(Clone)]
struct Archive {
    bytes: Arc<Vec<u8>>,
    asked: Arc<Mutex<Vec<(String, i64)>>>,
    flaky_requests: Arc<AtomicUsize>,
}

#[derive(serde::Deserialize)]
struct Asked {
    start: i64,
}

async fn replay(
    State(archive): State<Archive>,
    RoutePath(channel): RoutePath<String>,
    Query(asked): Query<Asked>,
) -> Response {
    archive
        .asked
        .lock()
        .unwrap()
        .push((channel.clone(), asked.start));
    let mut bytes = archive.bytes.as_slice();
    if channel == "flaky" && archive.flaky_requests.fetch_add(1, Ordering::SeqCst) == 0 {
        // About 6 of the 20 seconds, on a packet boundary.
        bytes = &bytes[..(bytes.len() * 3 / 10) / 188 * 188];
    }
    Response::builder()
        .header("content-type", "video/mp2t")
        .body(Body::from(bytes.to_vec()))
        .unwrap()
}

struct Setup {
    recorder: Arc<Recorder>,
    changes: Arc<Mutex<Vec<Recording>>>,
    tools: Tools,
    archive: Archive,
    dir: tempfile::TempDir,
}

async fn setup() -> Option<Setup> {
    let tools = tools()?;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.ts");
    make_ts(&tools, &source, 20);
    let archive = Archive {
        bytes: Arc::new(std::fs::read(&source).unwrap()),
        asked: Arc::default(),
        flaky_requests: Arc::default(),
    };
    let routes = Paced::new(&source, 20.0, None)
        .route("/live/one")
        .merge(Paced::new(&source, 20.0, Some(2.0)).route("/live/drop"))
        .merge(
            Router::new()
                .route("/replay/{channel}", get(replay))
                .with_state(archive.clone()),
        );
    let origin = serve(dir.path(), routes).await;
    let channels = Channels {
        live: HashMap::from([
            ("one".to_string(), format!("{origin}/live/one")),
            ("drop".to_string(), format!("{origin}/live/drop")),
            // Port 1: nothing listens there, and unlike a port bound and let
            // go, no other test's server can be handed it meanwhile.
            (
                "dead".to_string(),
                "http://127.0.0.1:1/live/dead.ts".to_string(),
            ),
        ]),
        origin,
    };
    let recorder = Recorder::open(
        &dir.path().join("data"),
        dir.path().join("Recordings"),
        Some(tools.clone()),
        Arc::new(channels),
    )
    .unwrap();
    let changes = Arc::new(Mutex::new(Vec::new()));
    let heard = changes.clone();
    recorder.start(Arc::new(move |recording| {
        heard.lock().unwrap().push(recording)
    }));
    Some(Setup {
        recorder,
        changes,
        tools,
        archive,
        dir,
    })
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn request(channel: &str, start: i64, end: i64) -> ScheduleRequest {
    ScheduleRequest {
        source_id: "test".into(),
        channel_id: channel.into(),
        channel_name: "One".into(),
        channel_logo_url: None,
        title: "Show: Live".into(),
        subtitle: None,
        summary: None,
        artwork_url: None,
        program_id: None,
        start,
        end,
        padding_before_seconds: 0,
        padding_after_seconds: 0,
    }
}

fn find(recorder: &Recorder, id: &str) -> Option<Recording> {
    recorder.list().unwrap().into_iter().find(|r| r.id == id)
}

async fn finished(recorder: &Recorder, id: &str) -> Recording {
    let started = Instant::now();
    loop {
        let recording = find(recorder, id).expect("still listed");
        if !matches!(
            recording.status,
            RecordingStatus::Scheduled | RecordingStatus::Recording
        ) {
            return recording;
        }
        assert!(
            started.elapsed() < Duration::from_secs(40),
            "still {:?}",
            recording.status
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test]
async fn records_until_the_end_time() {
    let Some(setup) = setup().await else { return };
    let started = Instant::now();
    let now = now();
    // ffmpeg analyses a live input for its first ~5 seconds before writing.
    let recording = setup
        .recorder
        .schedule(request("one", now, now + 8))
        .unwrap();
    assert_eq!(
        recording.status,
        RecordingStatus::Recording,
        "already on: starts at once"
    );
    assert_eq!(setup.recorder.active_count(), 1);

    let done = finished(&setup.recorder, &recording.id).await;
    assert_eq!(done.status, RecordingStatus::Completed, "{done:?}");
    assert!(
        started.elapsed() < Duration::from_secs(11),
        "stopped on time, not killed late"
    );
    assert_eq!(setup.recorder.active_count(), 0);
    let path = done.file_path.clone().unwrap();
    let name = std::path::Path::new(&path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with("One - Show Live - ") && name.ends_with(".ts"),
        "{name}"
    );
    assert_eq!(
        done.file_size_bytes,
        Some(std::fs::metadata(&path).unwrap().len() as i64)
    );
    let seconds = done.recorded_seconds.unwrap();
    assert!((6.0..=10.0).contains(&seconds), "recorded {seconds}s");
    let summary = probe(&setup.tools, &path);
    assert!(summary.format.contains("mpegts"));
    assert!((6.0..=10.0).contains(&summary.duration), "{summary:?}");

    let statuses: Vec<RecordingStatus> = setup
        .changes
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.status)
        .collect();
    assert!(
        statuses.contains(&RecordingStatus::Recording)
            && statuses.last() == Some(&RecordingStatus::Completed),
        "{statuses:?}"
    );
}

#[tokio::test]
async fn a_dropped_stream_is_kept_as_partial() {
    let Some(setup) = setup().await else { return };
    let now = now();
    let recording = setup
        .recorder
        .schedule(request("drop", now, now + 8))
        .unwrap();
    let done = finished(&setup.recorder, &recording.id).await;
    assert_eq!(done.status, RecordingStatus::Partial, "{:?}", done.error);
    assert!(
        done.error.as_deref().unwrap().contains("wasn't recorded"),
        "{:?}",
        done.error
    );
    let path = done.file_path.unwrap();
    let summary = probe(&setup.tools, &path);
    assert!((1.0..=4.0).contains(&summary.duration), "{summary:?}");
    // Refused reconnects left no empty parts behind.
    let files = std::fs::read_dir(std::path::Path::new(&path).parent().unwrap())
        .unwrap()
        .count();
    assert_eq!(files, 1);
}

#[tokio::test]
async fn stop_keeps_the_recording_and_delete_removes_it() {
    let Some(setup) = setup().await else { return };
    let recorder = &setup.recorder;
    let now = now();
    let recording = recorder
        .record_now(request("one", now - 600, now + 600))
        .unwrap();
    assert_eq!(recording.status, RecordingStatus::Recording);
    tokio::time::sleep(Duration::from_secs(3)).await;
    recorder.stop(&recording.id).unwrap();
    let done = finished(recorder, &recording.id).await;
    assert_eq!(
        done.status,
        RecordingStatus::Completed,
        "stopped by hand from when it was asked for: {:?}",
        done.error
    );
    let path = done.file_path.clone().unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    recorder.delete(&recording.id, true).unwrap();
    assert!(find(recorder, &recording.id).is_none());
    assert!(!std::path::Path::new(&path).exists());

    // Deleting one while it records stops ffmpeg and removes its file.
    let running = recorder.record_now(request("one", now, now + 600)).unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let path = find(recorder, &running.id).unwrap().file_path.unwrap();
    recorder.delete(&running.id, true).unwrap();
    let started = Instant::now();
    while recorder.active_count() > 0 || std::path::Path::new(&path).exists() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deleted recording kept going"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(recorder.list().unwrap().is_empty());
}

#[tokio::test]
async fn a_channel_that_never_answers_fails_in_plain_words() {
    let Some(setup) = setup().await else { return };
    let now = now();
    // Windows takes a couple of seconds to refuse a connection.
    let window = if cfg!(windows) { 10 } else { 4 };
    let recording = setup
        .recorder
        .schedule(request("dead", now, now + window))
        .unwrap();
    let done = finished(&setup.recorder, &recording.id).await;
    assert_eq!(done.status, RecordingStatus::Failed);
    assert_eq!(
        done.error.as_deref(),
        Some("The channel's stream couldn't be reached.")
    );
    assert!(done.file_path.is_none());
}

#[tokio::test]
async fn future_programmes_wait_and_can_be_cancelled() {
    let Some(setup) = setup().await else { return };
    let now = now();
    let mut later = request("one", now + 3600, now + 7200);
    later.padding_before_seconds = 60;
    let scheduled = setup.recorder.schedule(later).unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(
        find(&setup.recorder, &scheduled.id).unwrap().status,
        RecordingStatus::Scheduled
    );
    setup.recorder.cancel(&scheduled.id).unwrap();
    assert_eq!(
        find(&setup.recorder, &scheduled.id).unwrap().status,
        RecordingStatus::Cancelled
    );
    assert_eq!(setup.recorder.active_count(), 0);
}

/// A programme that aired an hour ago, `seconds` long with padding.
fn aired(channel: &str, seconds: i64, padding: i64) -> ScheduleRequest {
    let start = now() - 3600;
    ScheduleRequest {
        title: "Evening Report".into(),
        padding_before_seconds: padding,
        padding_after_seconds: padding,
        ..request(channel, start, start + seconds)
    }
}

#[tokio::test]
async fn saves_a_programme_from_catch_up_faster_than_it_aired() {
    let Some(setup) = setup().await else { return };
    let started = Instant::now();
    let asked = aired("one", 10, 1);
    let recording = setup.recorder.record_catchup(asked.clone()).unwrap();
    assert!(recording.from_catchup);
    assert_eq!(recording.status, RecordingStatus::Recording);
    assert_eq!(setup.recorder.active_count(), 1, "it holds a stream");

    let done = finished(&setup.recorder, &recording.id).await;
    assert_eq!(done.status, RecordingStatus::Completed, "{done:?}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "not tied to the clock: {:?}",
        started.elapsed()
    );
    assert!(done.from_catchup);
    let seconds = done.recorded_seconds.unwrap();
    assert!((11.0..=12.5).contains(&seconds), "saved {seconds}s");
    let path = done.file_path.clone().unwrap();
    let summary = probe(&setup.tools, &path);
    assert!(summary.format.contains("mpegts"));
    assert!((10.5..=13.0).contains(&summary.duration), "{summary:?}");
    assert!(
        path.contains("Evening Report"),
        "named like any recording: {path}"
    );

    // The replay was asked for with the padding before the programme.
    let asked_for = setup.archive.asked.lock().unwrap().clone();
    assert_eq!(asked_for, [("one".to_string(), asked.start - 1)]);

    // Its login never reaches what's kept or shown.
    let shown = serde_json::to_string(&setup.recorder.list().unwrap()).unwrap();
    assert!(
        !shown.contains("hunter2") && !shown.contains("/replay/"),
        "{shown}"
    );
    let database = std::fs::read(setup.dir.path().join("data").join("recordings.sqlite")).unwrap();
    assert!(!String::from_utf8_lossy(&database).contains("hunter2"));
}

#[tokio::test]
async fn a_replay_that_breaks_off_is_asked_for_again_from_where_it_stopped() {
    let Some(setup) = setup().await else { return };
    let asked = aired("flaky", 14, 0);
    let recording = setup.recorder.record_catchup(asked.clone()).unwrap();
    let done = finished(&setup.recorder, &recording.id).await;
    assert_eq!(done.status, RecordingStatus::Completed, "{done:?}");
    let asked_for = setup.archive.asked.lock().unwrap().clone();
    assert_eq!(asked_for.len(), 2, "{asked_for:?}");
    assert_eq!(asked_for[0].1, asked.start);
    let resumed_at = asked_for[1].1 - asked.start;
    assert!(
        (4..=8).contains(&resumed_at),
        "asked again from about 6 seconds in: {resumed_at}"
    );
    // The two parts became one file.
    let path = done.file_path.unwrap();
    let summary = probe(&setup.tools, &path);
    assert!((12.5..=15.5).contains(&summary.duration), "{summary:?}");
    let files = std::fs::read_dir(std::path::Path::new(&path).parent().unwrap())
        .unwrap()
        .count();
    assert_eq!(files, 1);
}

#[tokio::test]
async fn a_missing_source_folder_falls_back_and_says_so() {
    let Some(setup) = setup().await else { return };
    let main = setup.dir.path().join("Recordings");
    std::fs::create_dir_all(&main).unwrap();
    let unplugged = setup.dir.path().join("Volumes").join("TV Drive");
    setup.recorder.set_folders(RecordingFolders {
        main: main.clone(),
        fallback: setup.dir.path().join("Movies").join("Xorvio"),
        by_source: HashMap::from([("test".to_string(), unplugged.clone())]),
    });
    let recording = setup.recorder.record_catchup(aired("one", 4, 0)).unwrap();
    let done = finished(&setup.recorder, &recording.id).await;
    assert_eq!(done.status, RecordingStatus::Completed, "{done:?}");
    let path = std::path::PathBuf::from(done.file_path.unwrap());
    assert_eq!(path.parent().unwrap(), main);
    assert!(!unplugged.exists(), "the missing folder wasn't made");
    let note = done.note.unwrap();
    assert!(
        note.contains("TV Drive") && note.contains("wasn't available"),
        "{note}"
    );

    // With the drive back, recordings go to it again.
    std::fs::create_dir_all(&unplugged).unwrap();
    let mut again = aired("one", 4, 0);
    again.start -= 600;
    again.end -= 600;
    let recording = setup.recorder.record_catchup(again).unwrap();
    let done = finished(&setup.recorder, &recording.id).await;
    let path = std::path::PathBuf::from(done.file_path.unwrap());
    assert_eq!(path.parent().unwrap(), unplugged);
    assert_eq!(done.note, None);
}
