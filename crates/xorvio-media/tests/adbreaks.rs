//! Finding ad breaks in recordings: synthetic programmes made with ffmpeg
//! (programme, black and silent joins, spots of the usual lengths,
//! programme again) whose breaks are known, one with none but plenty to
//! mislead, and comskip's list used when it's installed.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use xorvio_media::{AdBreak, AdBreakState, AdBreaks, ChannelResolver, Recorder, Tools};
use xorvio_model::StreamTarget;

/// A piece of a synthetic broadcast.
#[derive(Clone, Copy)]
enum Piece {
    /// Moving test pattern and a steady tone.
    Programme(f64),
    /// Black and silent: a join between spots, or into or out of a break.
    Join(f64),
    /// An advert: another picture and tone (by number).
    Spot(f64, usize),
    /// A dark shot with sound.
    DarkShot(f64),
    /// A quiet moment with picture.
    Hush(f64),
    /// A fade to black and silence inside the programme.
    Fade(f64),
}

/// Picture size and frame rate: small and slow is quick to make, 25 fps
/// is a broadcast's.
#[derive(Clone, Copy)]
struct Format(&'static str);
const SMALL: Format = Format("s=320x180:r=10");
const BROADCAST: Format = Format("s=480x270:r=25");

impl Piece {
    fn seconds(self) -> f64 {
        match self {
            Piece::Programme(s) | Piece::Join(s) | Piece::Spot(s, _) | Piece::DarkShot(s) | Piece::Hush(s) | Piece::Fade(s) => s,
        }
    }

    /// Its video and audio sources for a filter graph.
    fn sources(self, format: Format) -> (String, String) {
        let d = self.seconds();
        let size = format.0;
        let tone = |hz: u32| format!("sine=f={hz}:r=48000:d={d}");
        let quiet = format!("anullsrc=r=48000:cl=mono,atrim=duration={d}");
        let black = format!("color=c=black:{size}:d={d}");
        let pattern = format!("testsrc2={size}:d={d}");
        match self {
            Piece::Programme(_) => (pattern, tone(440)),
            Piece::Join(_) | Piece::Fade(_) => (black, quiet),
            Piece::Spot(_, n) => {
                let looks = ["smptebars", "rgbtestsrc", "testsrc", "smptehdbars"];
                (format!("{}={size}:d={d}", looks[n % looks.len()]), tone(550 + 110 * n as u32))
            }
            Piece::DarkShot(_) => (black, tone(300)),
            Piece::Hush(_) => (pattern, quiet),
        }
    }
}

/// Writes the pieces one after another into an MPEG-TS file, as a recording
/// would be, and returns the breaks in it: runs of spots with their joins.
fn broadcast(tools: &Tools, path: &Path, pieces: &[Piece]) -> Vec<AdBreak> {
    broadcast_as(tools, path, pieces, SMALL)
}

fn broadcast_as(tools: &Tools, path: &Path, pieces: &[Piece], format: Format) -> Vec<AdBreak> {
    let mut graph = String::new();
    let mut inputs = String::new();
    for (i, piece) in pieces.iter().enumerate() {
        let (video, audio) = piece.sources(format);
        graph.push_str(&format!("{video},setsar=1,format=yuv420p[v{i}];{audio},aformat=sample_rates=48000:channel_layouts=mono[a{i}];"));
        inputs.push_str(&format!("[v{i}][a{i}]"));
    }
    graph.push_str(&format!("{inputs}concat=n={}:v=1:a=1[v][a]", pieces.len()));
    let path = path.to_string_lossy();
    ffmpeg(tools, &["-filter_complex", &graph, "-map", "[v]", "-map", "[a]", "-c:v", "mpeg2video", "-q:v", "4", "-g", "10", "-c:a", "aac", "-b:a", "64k", "-f", "mpegts", &path]);

    let mut breaks = Vec::new();
    let mut at = 0.0;
    let mut open: Option<f64> = None;
    for (i, piece) in pieces.iter().enumerate() {
        let next = pieces.get(i + 1);
        match piece {
            Piece::Join(_) if open.is_none() && matches!(next, Some(Piece::Spot(..))) => open = Some(at),
            Piece::Join(s) if open.is_some() && !matches!(next, Some(Piece::Spot(..))) => {
                breaks.push(AdBreak { start_seconds: open.take().unwrap(), end_seconds: at + s });
            }
            _ => {}
        }
        at += piece.seconds();
    }
    breaks
}

/// Spots with a join before each and after the last.
fn ad_break(spots: &[f64]) -> Vec<Piece> {
    ad_break_with(spots, &[0.5, 0.4, 0.6, 0.5, 0.3])
}

fn ad_break_with(spots: &[f64], joins: &[f64]) -> Vec<Piece> {
    let mut pieces = Vec::new();
    for (n, spot) in spots.iter().enumerate() {
        pieces.push(Piece::Join(joins[n % joins.len()]));
        pieces.push(Piece::Spot(*spot, n));
    }
    pieces.push(Piece::Join(0.5));
    pieces
}

struct NoChannels;

impl ChannelResolver for NoChannels {
    fn resolve(&self, _: &str, _: &str) -> anyhow::Result<StreamTarget> {
        anyhow::bail!("no channels here")
    }
}

/// A recorder whose recordings.sqlite already holds finished recordings of
/// `files`, named by their ids.
fn recorder_with(tools: &Tools, data: &Path, files: &[(&str, &Path)]) -> Arc<Recorder> {
    drop(Recorder::open(data, data.join("rec"), Some(tools.clone()), Arc::new(NoChannels)).unwrap());
    let db = rusqlite::Connection::open(data.join("recordings.sqlite")).unwrap();
    for (id, file) in files {
        db.execute(
            "INSERT INTO recordings (id, source_id, channel_id, channel_name, title, start, end, status, file_path, file_size,
                recorded_seconds, created_at, parts, covered_seconds, wanted_from)
             VALUES (?1, 'src', 'src|ch|1', 'Harbor One', 'Evening Film', 1000, 2000, 'completed', ?2, 1, 600, 900, ?3, 600, 1000)",
            rusqlite::params![id, file.to_string_lossy(), serde_json::to_string(&[file.to_string_lossy()]).unwrap()],
        )
        .unwrap();
    }
    let recorder = Recorder::open(data, data.join("rec"), Some(tools.clone()), Arc::new(NoChannels)).unwrap();
    recorder.set_comskip(None);
    recorder
}

/// Waits until a recording's breaks have been looked for.
async fn looked(recorder: &Recorder, id: &str) -> AdBreaks {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let breaks = recorder.ad_breaks(id).unwrap();
        if breaks.state != AdBreakState::Finding {
            return breaks;
        }
        assert!(Instant::now() < deadline, "finding the breaks took too long");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn close_to(found: &[AdBreak], truth: &[AdBreak]) -> bool {
    found.len() == truth.len()
        && found.iter().zip(truth).all(|(f, t)| (f.start_seconds - t.start_seconds).abs() <= 1.0 && (f.end_seconds - t.end_seconds).abs() <= 1.0)
}

/// What's in a folder, with each file's size and modification time.
fn listing(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| {
            let meta = e.metadata().unwrap();
            (e.path(), meta.len(), meta.modified().unwrap())
        })
        .collect();
    files.sort();
    files
}

#[tokio::test(flavor = "multi_thread")]
async fn finds_the_breaks_in_a_programme_and_none_in_one_without() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("media");
    std::fs::create_dir_all(&media).unwrap();

    // Programme, a break of four spots, programme, a break of five, programme.
    let mut with: Vec<Piece> = vec![Piece::Programme(90.0)];
    with.extend(ad_break(&[30.0, 20.0, 30.0, 15.0]));
    with.push(Piece::Programme(120.0));
    with.extend(ad_break(&[10.0, 30.0, 30.0, 60.0, 20.0]));
    with.push(Piece::Programme(45.0));
    let with_file = media.join("Harbor One - Evening Film.ts");
    let truth = broadcast(&tools, &with_file, &with);
    assert_eq!(truth.len(), 2);

    // Fades, a dark shot, a hush, two spot-length stretches in a row and
    // three short ones: all programme.
    let mut without = vec![Piece::Programme(37.0), Piece::Fade(0.5), Piece::Programme(81.0), Piece::DarkShot(1.0), Piece::Programme(52.0)];
    without.extend([Piece::Hush(2.0), Piece::Programme(20.0), Piece::Fade(0.6), Piece::Programme(30.0), Piece::Fade(0.4)]);
    without.extend([Piece::Programme(60.0), Piece::Fade(0.5), Piece::Programme(44.0)]);
    without.extend(ad_break(&[10.0, 10.0, 10.0]));
    without.push(Piece::Programme(40.0));
    let without_file = media.join("Harbor One - Quiz Night.ts");
    broadcast(&tools, &without_file, &without);
    let untouched = listing(&media);

    let data = dir.path().join("data");
    let recorder = recorder_with(&tools, &data, &[("with", &with_file), ("without", &without_file)]);
    recorder.start(Arc::new(|_| {}));
    assert_eq!(recorder.find_ad_breaks("with").unwrap().state, AdBreakState::Finding);
    assert_eq!(recorder.find_ad_breaks("without").unwrap().state, AdBreakState::Finding);

    let found = looked(&recorder, "with").await;
    assert_eq!((found.state, found.found_by.as_deref()), (AdBreakState::Found, Some("Xorvio")), "{found:?}");
    assert!(close_to(&found.breaks, &truth), "found {:?}, the breaks are {truth:?}", found.breaks);
    let none = looked(&recorder, "without").await;
    assert_eq!(none.state, AdBreakState::Found);
    assert!(none.breaks.is_empty(), "programme taken for a break: {:?}", none.breaks);

    // The recordings themselves are only read, and nothing is written beside them.
    assert_eq!(listing(&media), untouched);

    // Kept with the recording.
    drop(recorder);
    let recorder = Recorder::open(&data, data.join("rec"), Some(tools.clone()), Arc::new(NoChannels)).unwrap();
    assert_eq!(recorder.ad_breaks("with").unwrap(), found);
    assert!(recorder.find_ad_breaks("nonexistent").is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn short_joins_at_a_broadcast_frame_rate() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    // Joins of three to five frames, and spots a frame or two off their lengths.
    let joins = [0.12, 0.16, 0.2, 0.12, 0.16, 0.2];
    let mut pieces = vec![Piece::Programme(75.0)];
    pieces.extend(ad_break_with(&[29.96, 20.04, 10.0, 30.08, 15.0, 9.92], &joins));
    pieces.push(Piece::Programme(150.0));
    pieces.extend(ad_break_with(&[60.0, 30.0, 29.92], &joins));
    pieces.push(Piece::Programme(30.0));
    let file = dir.path().join("show.ts");
    let truth = broadcast_as(&tools, &file, &pieces, BROADCAST);
    let data = dir.path().join("data");
    let recorder = recorder_with(&tools, &data, &[("show", &file)]);
    recorder.start(Arc::new(|_| {}));
    recorder.find_ad_breaks("show").unwrap();
    let found = looked(&recorder, "show").await;
    assert!(close_to(&found.breaks, &truth), "found {:?}, the breaks are {truth:?}", found.breaks);
}

#[tokio::test(flavor = "multi_thread")]
async fn what_was_waiting_when_xorvio_quit_is_looked_through_after() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("show.ts");
    let mut pieces = vec![Piece::Programme(40.0)];
    pieces.extend(ad_break(&[30.0, 30.0, 30.0]));
    pieces.push(Piece::Programme(30.0));
    let truth = broadcast(&tools, &file, &pieces);
    let data = dir.path().join("data");
    drop(recorder_with(&tools, &data, &[("waiting", &file)]));
    let db = rusqlite::Connection::open(data.join("recordings.sqlite")).unwrap();
    db.execute("UPDATE recordings SET ad_scan = 'queued'", []).unwrap();
    drop(db);

    let recorder = Recorder::open(&data, data.join("rec"), Some(tools.clone()), Arc::new(NoChannels)).unwrap();
    recorder.set_comskip(None);
    assert_eq!(recorder.ad_breaks("waiting").unwrap().state, AdBreakState::Finding);
    recorder.start(Arc::new(|_| {}));
    let found = looked(&recorder, "waiting").await;
    assert!(close_to(&found.breaks, &truth), "found {:?}, the breaks are {truth:?}", found.breaks);
}

/// A stand-in comskip: notes its arguments, writes an EDL where it's told
/// to (or fails, with `fail`), and touches nothing else. A shell script, so
/// only for Unix.
#[cfg(unix)]
fn fake_comskip(dir: &Path, fail: bool) -> PathBuf {
    let script = dir.join(if fail { "comskip-broken" } else { "comskip" });
    let body = if fail {
        "#!/bin/sh\nexit 3\n".to_string()
    } else {
        format!(
            "#!/bin/sh\nfor arg in \"$@\"; do echo \"$arg\" >> '{}'; case \"$arg\" in --output=*) out=\"${{arg#--output=}}\";; esac; done\nprintf '0.00\\t12.00\\t0\\n610.00\\t790.50\\t0\\n' > \"$out/show.edl\"\nexit 0\n",
            dir.join("comskip-args.txt").display()
        )
    };
    std::fs::write(&script, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn comskip_is_used_when_it_is_installed() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("media");
    std::fs::create_dir_all(&media).unwrap();
    let file = media.join("show.ts");
    let mut pieces = vec![Piece::Programme(40.0)];
    pieces.extend(ad_break(&[30.0, 30.0, 30.0]));
    pieces.push(Piece::Programme(30.0));
    let truth = broadcast(&tools, &file, &pieces);
    let untouched = listing(&media);
    let data = dir.path().join("data");
    let recorder = recorder_with(&tools, &data, &[("show", &file)]);
    recorder.start(Arc::new(|_| {}));

    recorder.set_comskip(Some(fake_comskip(dir.path(), false)));
    recorder.find_ad_breaks("show").unwrap();
    let found = looked(&recorder, "show").await;
    assert_eq!(found.found_by.as_deref(), Some("comskip"));
    assert_eq!(
        found.breaks,
        [AdBreak { start_seconds: 0.0, end_seconds: 12.0 }, AdBreak { start_seconds: 610.0, end_seconds: 790.5 }]
    );
    let args = std::fs::read_to_string(dir.path().join("comskip-args.txt")).unwrap();
    assert!(args.lines().any(|a| a.starts_with("--ini=")), "{args}");
    let output = args.lines().find_map(|a| a.strip_prefix("--output=")).unwrap();
    assert!(!Path::new(output).starts_with(&media) && !Path::new(output).exists(), "its own folder, cleared after: {output}");
    assert_eq!(listing(&media), untouched);

    // A comskip that fails: Xorvio looks itself.
    recorder.set_comskip(Some(fake_comskip(dir.path(), true)));
    recorder.find_ad_breaks("show").unwrap();
    let found = looked(&recorder, "show").await;
    assert_eq!(found.found_by.as_deref(), Some("Xorvio"));
    assert!(close_to(&found.breaks, &truth), "found {:?}, the breaks are {truth:?}", found.breaks);
}
