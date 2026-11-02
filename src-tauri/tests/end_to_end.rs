//! The backend end to end, the way the app wires it: the demo source, a
//! channel played through the gateway, and a short recording of a live
//! channel. It uses the internet (the demo's public test streams) and ffmpeg,
//! so it only runs when asked:
//!
//!     cargo test -p xorvio --test end_to_end -- --ignored --nocapture

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use xorvio_core::{ChannelFilter, Core, MemorySecrets};
use xorvio_media::{ChannelResolver, Gateway, PlayHint, Recorder, Tools};
use xorvio_model::{PlaybackMode, RecordingStatus, ScheduleRequest, StreamTarget};

struct Resolver(Arc<Core>);

impl ChannelResolver for Resolver {
    fn resolve(&self, _source_id: &str, channel_id: &str) -> anyhow::Result<StreamTarget> {
        let channel = self.0.channel(channel_id)?.ok_or_else(|| anyhow::anyhow!("no such channel"))?;
        self.0.stream_target(&channel.source_id, &channel.stream, true)
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the network and ffmpeg"]
async fn demo_plays_and_records() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("xorvio-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let core = Core::open(&dir, Arc::new(MemorySecrets::default()))?;
    core.add_demo_source()?;

    let channels = core.channels(&ChannelFilter::default())?;
    assert!(!channels.is_empty(), "the demo has channels");
    let live = channels.iter().find(|c| c.stream.extension().as_deref() == Some("m3u8")).expect("a live HLS channel");
    let guide = core.now_next(std::slice::from_ref(&live.id), now())?;
    println!("{}: {} programmes now/next", live.name, guide.first().map(|r| r.programs.len()).unwrap_or(0));

    // Playing: the gateway hands back a local HLS address whose playlist loads.
    let tools = Tools::locate().expect("ffmpeg is installed");
    let gateway = Gateway::start(Some(tools.clone())).await?;
    let target = core.stream_target(&live.source_id, &live.stream, true)?;
    let stream = gateway.prepare(target, PlayHint { extension: live.stream.extension(), ..Default::default() }).await?;
    println!("gateway: {:?} {}", stream.mode, stream.url);
    assert_eq!(stream.mode, PlaybackMode::Hls);
    assert!(stream.url.starts_with("http://127.0.0.1:"));
    let playlist = reqwest::get(&stream.url).await?.error_for_status()?.text().await?;
    assert!(playlist.starts_with("#EXTM3U"), "a playlist came back");
    assert!(playlist.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).all(|l| l.contains("127.0.0.1") || !l.contains("://")), "every entry points back through the gateway");

    // Recording: a few seconds of the live channel end up in a playable file.
    let recordings_dir = dir.join("Recordings");
    let recorder = Recorder::open(&dir, recordings_dir.clone(), Some(tools.clone()), Arc::new(Resolver(core.clone())))?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    recorder.start(Arc::new(move |r| {
        let _ = tx.send(r);
    }));
    let start = now();
    let recording = recorder.record_now(ScheduleRequest {
        source_id: live.source_id.clone(),
        channel_id: live.id.clone(),
        channel_name: live.name.clone(),
        channel_logo_url: None,
        title: "End-to-end test".into(),
        subtitle: None,
        summary: None,
        artwork_url: None,
        program_id: None,
        start,
        end: start + 20,
        padding_before_seconds: 0,
        padding_after_seconds: 0,
    })?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let finished = loop {
        let update = tokio::time::timeout_at(deadline, rx.recv()).await?.expect("updates keep coming");
        if update.id == recording.id {
            println!("recording: {:?} {:?}s {:?} bytes", update.status, update.recorded_seconds, update.file_size_bytes);
            if matches!(update.status, RecordingStatus::Completed | RecordingStatus::Partial | RecordingStatus::Failed) {
                break update;
            }
        }
    };
    assert_ne!(finished.status, RecordingStatus::Failed, "{:?}", finished.error);
    let path = finished.file_path.expect("a file");
    let probe = std::process::Command::new(&tools.ffprobe)
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", &path])
        .output()?;
    let seconds: f64 = String::from_utf8_lossy(&probe.stdout).trim().parse().unwrap_or(0.0);
    println!("recorded file: {path} ({seconds:.1}s)");
    assert!(seconds > 5.0, "the file holds video");

    // And the recording plays back through the gateway too.
    let url = url::Url::from_file_path(&path).unwrap().to_string();
    let played = gateway
        .prepare(StreamTarget { url, user_agent: None, headers: vec![], is_live: false }, PlayHint { extension: Some("ts".into()), ..Default::default() })
        .await?;
    println!("recording plays as {:?}", played.mode);

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
#[ignore = "diagnostic"]
fn demo_guide_window() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("xorvio-guide-{}", std::process::id()));
    let core = Core::open(&dir, Arc::new(MemorySecrets::default()))?;
    core.add_demo_source()?;
    let ids: Vec<String> = core.channels(&ChannelFilter::default())?.into_iter().take(3).map(|c| c.id).collect();
    let t = now();
    for (from, to) in [(t - 6 * 3600, t + 6 * 3600), (t - 86400, t + 86400)] {
        let rows = core.guide(&ids, from, to)?;
        println!("window {}h: {:?}", (to - from) / 3600, rows.iter().map(|r| (r.channel.name.clone(), r.programs.len())).collect::<Vec<_>>());
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Times the queries Home makes against a copy of a real data folder
/// (`XORVIO_DATA_COPY`), to find the slow one.
#[test]
#[ignore = "diagnostic"]
fn home_query_timings() -> anyhow::Result<()> {
    use std::time::Instant;
    use xorvio_core::{BrowseQuery, BrowseSort};
    use xorvio_model::CategoryKind;
    let dir = std::path::PathBuf::from(std::env::var("XORVIO_DATA_COPY")?);
    let core = Core::open(&dir, Arc::new(MemorySecrets::default()))?;
    let t = now();
    let time = |label: &str, f: &dyn Fn() -> anyhow::Result<usize>| {
        let start = Instant::now();
        let n = f();
        println!("{label:<22} {:>8.1} ms  {:?}", start.elapsed().as_secs_f64() * 1000.0, n.map_err(|e| e.to_string()));
    };
    time("continue_watching", &|| Ok(core.continue_watching(20)?.len()));
    time("watchlist", &|| Ok(core.watchlist()?.len()));
    time("top_titles", &|| Ok(core.top_titles(CategoryKind::Movie, 10)?.0.len()));
    time("movies recent", &|| Ok(core.movies(&BrowseQuery { sort: BrowseSort::RecentlyAdded, limit: 24, ..Default::default() })?.len()));
    // Movies as the Movies screen lists them: one poster per film, a page at a time.
    time("movies versions", &|| Ok(core.movies(&BrowseQuery { sort: BrowseSort::RecentlyAdded, limit: 120, group_versions: true, ..Default::default() })?.len()));
    time("movies versions p50", &|| Ok(core.movies(&BrowseQuery { sort: BrowseSort::RecentlyAdded, limit: 120, offset: 6000, group_versions: true, ..Default::default() })?.len()));
    time("movies title versions", &|| Ok(core.movies(&BrowseQuery { sort: BrowseSort::Title, limit: 120, group_versions: true, ..Default::default() })?.len()));
    time("movies unwatched", &|| Ok(core.movies(&BrowseQuery { limit: 120, group_versions: true, watched: xorvio_core::WatchedFilter::Unwatched, ..Default::default() })?.len()));
    let page: Vec<String> = core.movies(&BrowseQuery { limit: 120, group_versions: true, ..Default::default() })?.into_iter().map(|m| m.id).collect();
    time("title_marks 120", &|| Ok(core.title_marks(CategoryKind::Movie, &page)?.len()));
    time("series recent", &|| Ok(core.series_list(&BrowseQuery { sort: BrowseSort::RecentlyAdded, limit: 24, ..Default::default() })?.len()));
    time("series rating", &|| Ok(core.series_list(&BrowseQuery { sort: BrowseSort::Rating, limit: 24, ..Default::default() })?.len()));
    time("upcoming_programs", &|| Ok(core.upcoming_programs(t + 300, t + 12 * 3600, 24)?.len()));
    time("categories live", &|| Ok(core.categories(CategoryKind::Live)?.len()));
    time("channels all", &|| Ok(core.channels(&ChannelFilter::default())?.len()));
    time("genres movie", &|| Ok(core.genres(CategoryKind::Movie)?.len()));
    time("search", &|| Ok(core.search("news", 40)?.programs.len()));
    let ids: Vec<String> = core.channels(&ChannelFilter::default())?.into_iter().take(20).map(|c| c.id).collect();
    time("guide 20 rows", &|| Ok(core.guide(&ids, t - 4 * 3600, t + 24 * 3600)?.len()));
    time("now_next 20", &|| Ok(core.now_next(&ids, t)?.len()));
    Ok(())
}
