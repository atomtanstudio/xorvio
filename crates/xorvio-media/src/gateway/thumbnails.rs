//! Thumbnails for the seek bar: a title's frames, one every few seconds,
//! drawn 160 pixels wide into sheets of 10 × 10, made once per title in the
//! background and kept in a size-capped folder in the app's data folder. The
//! gateway serves the sheets (kind `p`, addressed by the title's key); the
//! interface shows the tile for the time under the pointer.
//!
//! Each tile is one short ffmpeg at a low priority: input seeking to the
//! moment, only the keyframe there decoded, on one thread
//! (`-noaccurate_seek -skip_frame nokey`; six times quicker than decoding up
//! to the moment for long-GOP video, and no slower for anything else),
//! scaled and written raw. A tile that has no keyframe to show (intra-refresh
//! video) is taken again by decoding up to the moment. ffmpeg's `tile` filter
//! then makes the JPEG sheets. Seeking reads only a little of the file
//! around each moment, which matters most for a film on a server.
//!
//! A remote title's thumbnails cost a connection of their own to where the
//! title comes from. They're made only while nothing else is playing beyond
//! the title's own stream (no other stream, no other player's, nothing cast)
//! and the caller's check passes (for a provider: it allows more than one
//! stream at a time, and no recording from it is running or about to start),
//! both asked again before every tile, so a recording starting or another
//! channel playing stops them. Another stream starting stops even the tile
//! under way, so the two never overlap. Tiles are a second apart and there
//! are at most 150. ffmpeg reads the title through the gateway (a feed of its
//! own), which lets one request reach the provider at a time: when ffmpeg
//! seeks it opens the next request before closing the last, and the feed ends
//! the last one first. Each request has a connection of its own, closed with
//! it, so none is ever kept open beside it. Otherwise the title has none:
//! building them from the stream as it plays would only cover what has been
//! watched, not where someone scrubs to, and would have the ffmpeg serving
//! the player decode video it otherwise only copies.

mod cache;

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use axum::body::Body;
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::http::{Method, Response, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use xorvio_model::{PlaybackMode, StreamTarget};

pub(crate) use cache::Cache;

use super::{Entry, Gateway, STREAM_GONE, Shared, lock, pass_through, plain, serve_file};
use crate::process::{Input, background, drain_stderr, web_input_args};
use crate::remux::RemuxPlan;

/// A title's thumbnails for the seek bar: sheets of small frames, each read
/// left to right and top to bottom, one every `interval_seconds`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailSheet {
    /// The sheets' gateway addresses (JPEG), in order.
    pub images: Vec<String>,
    /// Tile `k` shows the title about `(k + 0.5) × interval` seconds in.
    pub interval_seconds: f64,
    pub count: usize,
    /// Tiles across and down a full sheet; the last sheet can have fewer rows.
    pub columns: usize,
    pub rows: usize,
    /// One tile's size in pixels.
    pub width: u32,
    pub height: u32,
}

/// Where a title's thumbnails are: ready, being made, or not to be had.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Thumbnails {
    Ready { sheet: ThumbnailSheet },
    /// Being made (or waiting for another title's to finish): ask again in
    /// a few seconds.
    Making,
    /// None for this title, and why, for the log.
    Unavailable { reason: String },
}

/// Whether making a remote title's thumbnails may open a connection to where
/// it comes from right now: `Err` says why not. It's given the host the
/// title is on ("provider.example:8080"), for titles whose source doesn't
/// say on its own whose connections they use (an add-on's links).
pub type ConnectionCheck = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// How many streams a title's source allows at once, as far as is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Allowance {
    /// Nothing limits it: the person's own media server, or an add-on's link.
    Unlimited,
    /// What the provider reports.
    Streams(i64),
    /// The provider doesn't say.
    Unknown,
}

/// The rule for a remote title's thumbnails, which need a connection beside
/// the one playing: only where the source allows more than one stream at a
/// time, and only while no recording from it is running or about to start
/// (`recordings`). A provider that allows one stream never sees two.
pub fn second_connection(allowance: Allowance, recordings: usize) -> Result<(), String> {
    match allowance {
        Allowance::Streams(limit) if limit <= 1 => {
            return Err("The provider allows one stream at a time, and thumbnails would need a second.".into());
        }
        Allowance::Unknown => {
            return Err("The provider doesn't say how many streams it allows at once, so no second one is opened for thumbnails.".into());
        }
        _ => {}
    }
    if recordings > 0 {
        return Err("A recording from this source is running or about to start, and needs the provider's streams.".into());
    }
    Ok(())
}

/// The tiles' width; their height follows the picture's shape.
const TILE_WIDTH: u32 = 160;
const COLUMNS: usize = 10;
const ROWS: usize = 10;
/// Seconds between tiles, the shortest that keeps a title within its count.
const STEPS: [f64; 10] = [2.0, 5.0, 10.0, 15.0, 20.0, 30.0, 60.0, 120.0, 300.0, 600.0];
const MOST_TILES: usize = 400;
/// Each of a remote title's tiles is a request to its server: fewer, and
/// spaced out, so a provider never sees anything like a burst.
const MOST_REMOTE_TILES: usize = 150;
const REMOTE_PACE: Duration = Duration::from_secs(1);
/// How long a title plays before its first tile, so thumbnails never
/// compete with the player starting it (WebKit can open a couple of
/// requests at once while it does): longer for a title on a server.
const LOCAL_WAIT: Duration = Duration::from_secs(2);
const REMOTE_WAIT: Duration = Duration::from_secs(15);
/// Shorter titles are scrubbed easily enough without.
const SHORTEST: f64 = 20.0;
const TILE_TIMEOUT: Duration = Duration::from_secs(30);
/// Tiles in a row that can't be read before giving up on a title.
const MOST_MISSES: usize = 6;
/// How long a feed's request waits for the one before it to close.
const FEED_WAIT: Duration = Duration::from_secs(5);
/// How often a tile being taken checks the title is still playing.
const GONE_CHECK: Duration = Duration::from_millis(250);
/// Changed whenever how thumbnails are made changes, so old ones are made again.
const FORMAT: u32 = 1;

/// The thumbnails part of the gateway's state.
#[derive(Default)]
pub(crate) struct Previews {
    cache: Mutex<Option<Arc<Cache>>>,
    /// The title whose thumbnails are being made (one at a time).
    making: Mutex<Option<String>>,
    /// Titles whose thumbnails couldn't be made this session, and why.
    failed: Mutex<HashMap<String, String>>,
    /// The wait before a remote title's first tile, when not the usual.
    remote_wait: Mutex<Option<Duration>>,
}

/// What `sheet.json` keeps beside a title's sheets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Layout {
    pub interval_seconds: f64,
    pub count: usize,
    pub columns: usize,
    pub rows: usize,
    pub width: u32,
    pub height: u32,
    /// How many sheets there are.
    pub sheets: usize,
}

/// What to make for a title.
#[derive(Debug, Clone, PartialEq)]
struct Plan {
    interval: f64,
    count: usize,
    width: u32,
    height: u32,
}

impl Plan {
    fn new(duration: f64, aspect: Option<f64>, remote: bool) -> Plan {
        let most = if remote { MOST_REMOTE_TILES } else { MOST_TILES };
        let interval = STEPS
            .into_iter()
            .find(|step| duration / step <= most as f64)
            .unwrap_or_else(|| (duration / most as f64).ceil());
        let count = ((duration / interval).floor() as usize).max(1);
        let height = aspect
            .filter(|a| a.is_finite() && *a > 0.0)
            .map_or(90, |a| ((TILE_WIDTH as f64 / a / 2.0).round() as u32 * 2).clamp(40, 320));
        Plan { interval, count, width: TILE_WIDTH, height }
    }

    fn time_of(&self, tile: usize) -> f64 {
        (tile as f64 + 0.5) * self.interval
    }

    fn frame_bytes(&self) -> usize {
        // yuv420p: a full-size luma plane and two quarter-size chroma planes.
        (self.width * self.height * 3 / 2) as usize
    }

    fn sheets(&self) -> usize {
        self.count.div_ceil(COLUMNS * ROWS)
    }

    fn layout(&self) -> Layout {
        Layout {
            interval_seconds: self.interval,
            count: self.count,
            columns: COLUMNS,
            rows: ROWS.min(self.count.div_ceil(COLUMNS)),
            width: self.width,
            height: self.height,
            sheets: self.sheets(),
        }
    }
}

/// A title's name in the cache: its file, size and modification time for
/// one on this computer (so a changed file is a new title), its item id and
/// length for one elsewhere (whose address can change). The standard hasher
/// may change between Rust releases, which only means making them again.
fn title_key(input: &Input, item_id: &str, duration: f64) -> Option<String> {
    let mut hasher = DefaultHasher::new();
    FORMAT.hash(&mut hasher);
    match input {
        Input::File(path) => {
            let meta = std::fs::metadata(path).ok()?;
            let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
            ("file", path, meta.len(), modified).hash(&mut hasher);
        }
        Input::Web(_) => {
            let item = item_id.trim();
            if item.is_empty() {
                return None;
            }
            ("title", item, duration.round() as i64).hash(&mut hasher);
        }
    }
    Some(format!("{:016x}", hasher.finish()))
}

fn unavailable(reason: &str) -> Thumbnails {
    Thumbnails::Unavailable { reason: reason.to_string() }
}

/// A job's reason for stopping early.
enum Stop {
    /// The title isn't playing any more.
    Gone,
    /// A remote title's connection isn't allowed now; it may be later.
    Refused(String),
    /// They can't be made: remembered for the session.
    Failed(String),
}

/// One title's thumbnails being made.
struct Job {
    shared: Arc<Shared>,
    cache: Arc<Cache>,
    ffmpeg: PathBuf,
    key: String,
    /// The title, as the player has it.
    target: StreamTarget,
    /// What ffmpeg reads: the file, or the gateway's feed of a remote title.
    input: StreamTarget,
    /// The feed's stream id, released when the job ends.
    feed: Option<String>,
    video_map: String,
    plan: Plan,
    /// The caller's check, for a remote title.
    remote: Option<ConnectionCheck>,
}

impl Gateway {
    /// Where thumbnails are kept, and how much room they may take: the
    /// least recently used go first once it's full. What a previous run
    /// left half made goes now.
    pub fn set_thumbnail_cache(&self, dir: PathBuf, limit_bytes: u64) {
        match Cache::open(dir, limit_bytes) {
            Ok(cache) => *lock(&self.shared.previews.cache) = Some(Arc::new(cache)),
            Err(error) => tracing::warn!("the thumbnails folder couldn't be used: {error}"),
        }
    }

    /// How long a remote title plays before its thumbnails start, for tests.
    #[doc(hidden)]
    pub fn set_thumbnail_wait(&self, wait: Duration) {
        *lock(&self.shared.previews.remote_wait) = Some(wait);
    }

    /// The seek bar's thumbnails for the film, episode or recording playing
    /// at `stream_url`, under the item it is (`item_id`, for a title from
    /// elsewhere). The first ask starts making them, in the background, and
    /// the next asks say when they're ready; once made they're kept for the
    /// title. `may_connect` is asked before a remote title's first tile and
    /// every one after (see the module's notes).
    pub fn thumbnails(&self, stream_url: &str, item_id: &str, may_connect: ConnectionCheck) -> Thumbnails {
        let shared = &self.shared;
        let Some(entry) = shared.id_in(stream_url).and_then(|id| shared.entry(&id)) else {
            return unavailable(STREAM_GONE);
        };
        let Some(tools) = &shared.tools else {
            return unavailable("Thumbnails need ffmpeg, and Xorvio couldn't find it.");
        };
        let Some(cache) = lock(&shared.previews.cache).clone() else {
            return unavailable("There's nowhere to keep thumbnails.");
        };
        if entry.target.is_live || entry.timeshift.is_some() {
            return unavailable("Live channels have no thumbnails.");
        }
        if entry.remux.as_ref().is_some_and(RemuxPlan::is_following) {
            return unavailable("Thumbnails come once the recording has finished.");
        }
        if entry.mode == PlaybackMode::Hls {
            return unavailable("Streamed playlists have no thumbnails.");
        }
        let Some(duration) = entry.duration.filter(|d| d.is_finite() && *d >= SHORTEST) else {
            return unavailable("The title's length isn't known, or it's too short to need thumbnails.");
        };
        let Some(input) = Input::of(&entry.target) else {
            return unavailable("Xorvio can't open this kind of address.");
        };
        let Some(key) = title_key(&input, item_id, duration) else {
            return unavailable("The title couldn't be told apart from others.");
        };
        if let Some(layout) = cache.layout(&key) {
            cache.touch(&key);
            return Thumbnails::Ready { sheet: shared.sheet(&key, &layout) };
        }
        if let Some(reason) = lock(&shared.previews.failed).get(&key) {
            return unavailable(reason);
        }
        let remote = input.is_web();
        {
            let mut making = lock(&shared.previews.making);
            // One title at a time; another's stop by themselves once their
            // title stops playing, and this one's start when next asked.
            if making.is_some() {
                return Thumbnails::Making;
            }
            if remote && let Err(reason) = shared.may_connect(&entry.target, &may_connect) {
                tracing::info!("no thumbnails for a remote title: {reason}");
                return unavailable(&reason);
            }
            *making = Some(key.clone());
        }
        let video = entry.probe.as_ref().and_then(|p| p.video.as_ref());
        let aspect = video.and_then(|v| {
            v.aspect.or_else(|| Some(v.width? as f64 / v.height.filter(|h| *h > 0)? as f64))
        });
        let (input, feed) = match remote {
            true => {
                let (url, id) = shared.register_feed(&entry.target);
                (StreamTarget { url, user_agent: None, headers: Vec::new(), is_live: false }, Some(id))
            }
            false => (entry.target.clone(), None),
        };
        let job = Job {
            shared: shared.clone(),
            cache,
            ffmpeg: tools.ffmpeg.clone(),
            key,
            target: entry.target.clone(),
            input,
            feed,
            video_map: video.map_or("0:V:0".to_string(), |v| format!("0:{}", v.index)),
            plan: Plan::new(duration, aspect, remote),
            remote: remote.then_some(may_connect),
        };
        tokio::spawn(job.run());
        Thumbnails::Making
    }
}

impl Shared {
    /// Registers a thumbnails job's feed of a remote title, returning its
    /// address and id.
    fn register_feed(&self, target: &StreamTarget) -> (String, String) {
        let entry = Entry {
            feed: Some(Arc::new(tokio::sync::Semaphore::new(1))),
            ..Entry::new(target.clone(), PlaybackMode::Native, None, 0.0, None)
        };
        let id = self.register(entry);
        (self.address("s", &id, "title"), id)
    }

    /// A title's sheets with this launch's addresses.
    fn sheet(&self, key: &str, layout: &Layout) -> ThumbnailSheet {
        ThumbnailSheet {
            images: (0..layout.sheets).map(|n| self.address("p", key, &format!("{n}.jpg"))).collect(),
            interval_seconds: layout.interval_seconds,
            count: layout.count,
            columns: layout.columns,
            rows: layout.rows,
            width: layout.width,
            height: layout.height,
        }
    }

    /// Whether the player still has the title: a stream of it registered
    /// that isn't another player's. A seek registers the title again under
    /// a new address before releasing the old one.
    fn still_playing(&self, target: &StreamTarget) -> bool {
        let streams = lock(&self.streams);
        streams
            .by_id
            .iter()
            .any(|(id, entry)| entry.target.url == target.url && entry.feed.is_none() && !streams.external.contains(id))
    }

    /// Whether a remote title's thumbnails may open a connection now: the
    /// title's own stream is the only one from anywhere else (nothing else
    /// playing, no address handed to another player, nothing on a TV), and
    /// the caller's check passes.
    fn may_connect(&self, target: &StreamTarget, check: &ConnectionCheck) -> Result<(), String> {
        let (own, others) = self.streams_of(target);
        if others > 0 || own > 1 {
            return Err(ELSE_PLAYING.into());
        }
        if self.is_casting() {
            return Err(ON_A_TV.into());
        }
        let host = url::Url::parse(&target.url).ok().and_then(|url| {
            let host = url.host_str()?.to_string();
            Some(url.port().map_or(host.clone(), |port| format!("{host}:{port}")))
        });
        check(host.as_deref().unwrap_or_default())
    }

    /// Something besides the title started (another stream, an address handed
    /// to another player, a TV) and needs the connection a tile holds. The
    /// title opened twice doesn't count here: a seek does that for a moment.
    fn something_else(&self, target: &StreamTarget) -> Option<&'static str> {
        if self.streams_of(target).1 > 0 {
            Some(ELSE_PLAYING)
        } else {
            self.is_casting().then_some(ON_A_TV)
        }
    }

    /// The players with the title, and the streams from anywhere else
    /// beside it (another title, an address handed to another player).
    fn streams_of(&self, target: &StreamTarget) -> (usize, usize) {
        let streams = lock(&self.streams);
        // Feeds are thumbnails' own, one request at a time.
        let players = || streams.by_id.iter().filter(|(_, entry)| entry.feed.is_none());
        let web = |entry: &Entry| Input::of(&entry.target).is_some_and(|input| input.is_web());
        let own = players()
            .filter(|(id, entry)| entry.target.url == target.url && !streams.external.contains(id))
            .count();
        let others = players()
            .filter(|(id, entry)| web(entry) && (entry.target.url != target.url || streams.external.contains(id)))
            .count();
        (own, others)
    }
}

const ELSE_PLAYING: &str = "Something else is playing, and thumbnails would need another connection.";
const ON_A_TV: &str = "Something is playing on a TV, and thumbnails would need another connection.";

impl Job {
    async fn run(self) {
        let started = Instant::now();
        let outcome = self.make().await;
        let previews = &self.shared.previews;
        match outcome {
            Ok(()) => {
                let seconds = started.elapsed().as_secs_f32();
                tracing::info!("made {} thumbnails in {seconds:.0}s", self.plan.count);
                self.cache.tidy(Some(&self.key));
            }
            Err(Stop::Gone) => tracing::debug!("thumbnails stopped: the title isn't playing any more"),
            Err(Stop::Refused(reason)) => tracing::info!("thumbnails stopped: {reason}"),
            Err(Stop::Failed(reason)) => {
                tracing::info!("thumbnails couldn't be made: {reason}");
                lock(&previews.failed).insert(self.key.clone(), reason);
            }
        }
        let _ = std::fs::remove_dir_all(self.cache.work_dir(&self.key));
        if let Some(feed) = &self.feed {
            self.shared.release(feed);
        }
        let mut making = lock(&previews.making);
        if making.as_deref() == Some(self.key.as_str()) {
            *making = None;
        }
    }

    async fn make(&self) -> Result<(), Stop> {
        let failed = |e: std::io::Error| Stop::Failed(format!("The thumbnails folder couldn't be written: {e}"));
        let work = self.cache.work_dir(&self.key);
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).map_err(failed)?;
        let plan = &self.plan;
        let frame_bytes = plan.frame_bytes();
        let mut tiles: Vec<u8> = Vec::with_capacity(plan.count * frame_bytes);
        // Black, until there's a frame to repeat over one that can't be read.
        let mut last = black(plan);
        let (mut made, mut misses) = (0usize, 0usize);
        let mut keyframes = true;
        let mut last_connection: Option<Instant> = None;
        let wait = match self.remote {
            Some(_) => lock(&self.shared.previews.remote_wait).unwrap_or(REMOTE_WAIT),
            None => LOCAL_WAIT,
        };
        tokio::time::sleep(wait).await;
        for tile in 0..plan.count {
            if !self.shared.still_playing(&self.target) {
                return Err(Stop::Gone);
            }
            if let Some(check) = &self.remote {
                if let Some(at) = last_connection {
                    tokio::time::sleep(REMOTE_PACE.saturating_sub(at.elapsed())).await;
                }
                let (shared, target, check) = (self.shared.clone(), self.target.clone(), check.clone());
                let allowed = tokio::task::spawn_blocking(move || shared.may_connect(&target, &check))
                    .await
                    .unwrap_or_else(|_| Err("The check couldn't run.".into()));
                allowed.map_err(Stop::Refused)?;
                last_connection = Some(Instant::now());
            }
            let time = plan.time_of(tile);
            let mut frame = self.take(time, keyframes).await?;
            // Video with no keyframes to land on (intra refresh) is decoded
            // up to the moment instead, from then on.
            if frame.is_none() && keyframes && made == 0 {
                frame = self.take(time, false).await?;
                keyframes = frame.is_none();
            }
            match frame {
                Some(bytes) => {
                    last = bytes;
                    made += 1;
                    misses = 0;
                }
                None => {
                    misses += 1;
                    if misses >= MOST_MISSES {
                        return Err(Stop::Failed("The video couldn't be read for thumbnails.".into()));
                    }
                }
            }
            tiles.extend_from_slice(&last);
        }
        if made == 0 {
            return Err(Stop::Failed("The video couldn't be read for thumbnails.".into()));
        }
        let layout = plan.layout();
        let per_sheet = COLUMNS * ROWS;
        for (n, chunk) in tiles.chunks(per_sheet * frame_bytes).enumerate() {
            let rows = (chunk.len() / frame_bytes).div_ceil(COLUMNS);
            let out = work.join(format!("{n}.jpg"));
            if let Err(error) = assemble(&self.ffmpeg, chunk, plan, rows, &out).await {
                return Err(Stop::Failed(format!("The thumbnail sheets couldn't be made: {error:#}")));
            }
        }
        let json = serde_json::to_vec(&layout).map_err(|e| Stop::Failed(e.to_string()))?;
        std::fs::write(work.join(cache::LAYOUT), json).map_err(failed)?;
        self.cache.finish(&self.key, &work).map_err(failed)
    }

    /// One tile: the raw frame at `time`, or `None` when there's none to be
    /// had there. Stops as soon as the title stops playing, or for a remote
    /// title, something else starts.
    async fn take(&self, time: f64, keyframes: bool) -> Result<Option<Vec<u8>>, Stop> {
        let Some(input) = Input::of(&self.input) else {
            return Err(Stop::Failed("Xorvio can't open this kind of address.".into()));
        };
        let args = tile_args(&self.input, &input, &self.video_map, time, &self.plan, keyframes);
        let spawned = background(&self.ffmpeg)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => return Err(Stop::Failed(format!("ffmpeg couldn't be started: {error}"))),
        };
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(drain_stderr(stderr, "thumbnails"));
        }
        let Some(mut stdout) = child.stdout.take() else {
            return Ok(None);
        };
        let want = self.plan.frame_bytes();
        let read = async {
            let mut bytes = Vec::with_capacity(want);
            let _ = (&mut stdout).take(want as u64).read_to_end(&mut bytes).await;
            bytes
        };
        let gone = async {
            loop {
                tokio::time::sleep(GONE_CHECK).await;
                if !self.shared.still_playing(&self.target) {
                    break Stop::Gone;
                }
                if self.remote.is_some()
                    && let Some(reason) = self.shared.something_else(&self.target)
                {
                    break Stop::Refused(reason.into());
                }
            }
        };
        // Dropping the child kills ffmpeg, closing any connection it holds.
        let bytes = tokio::select! {
            bytes = tokio::time::timeout(TILE_TIMEOUT, read) => bytes.unwrap_or_default(),
            stop = gone => return Err(stop),
        };
        let _ = child.start_kill();
        Ok((bytes.len() == want).then_some(bytes))
    }
}

/// ffmpeg's arguments for one tile, written raw (yuv420p) to stdout.
fn tile_args(target: &StreamTarget, input: &Input, video_map: &str, time: f64, plan: &Plan, keyframes: bool) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-nostats", "-loglevel", "error", "-threads", "1"]
        .map(String::from)
        .to_vec();
    if input.is_web() {
        args.extend(web_input_args(target));
        args.extend(["-rw_timeout", "15000000"].map(String::from));
    }
    if keyframes {
        args.extend(["-noaccurate_seek", "-skip_frame", "nokey"].map(String::from));
    }
    args.extend(["-ss".to_string(), format!("{time:.3}"), "-i".to_string(), input.ffmpeg_input()]);
    args.extend(["-map".to_string(), video_map.to_string()]);
    args.extend(["-an", "-sn", "-dn", "-frames:v", "1"].map(String::from));
    args.extend([
        "-vf".to_string(),
        format!("scale={}:{}:flags=bicubic,setsar=1", plan.width, plan.height),
    ]);
    args.extend(["-pix_fmt", "yuv420p", "-f", "rawvideo", "pipe:1"].map(String::from));
    args
}

/// A black tile in yuv420p.
fn black(plan: &Plan) -> Vec<u8> {
    let luma = (plan.width * plan.height) as usize;
    let mut frame = vec![16u8; luma];
    frame.resize(plan.frame_bytes(), 128);
    frame
}

/// Tiles one sheet's frames into a JPEG with ffmpeg's `tile` filter.
async fn assemble(ffmpeg: &Path, frames: &[u8], plan: &Plan, rows: usize, out: &Path) -> anyhow::Result<()> {
    let mut child = background(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "error", "-y"])
        .args(["-f", "rawvideo", "-pix_fmt", "yuv420p"])
        .args(["-video_size", &format!("{}x{}", plan.width, plan.height)])
        .args(["-framerate", "1", "-i", "pipe:0"])
        .args(["-vf", &format!("tile={COLUMNS}x{rows}")])
        .args(["-frames:v", "1", "-c:v", "mjpeg", "-q:v", "5", "-f", "image2", "-update", "1"])
        .arg(out)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let errors = child.stderr.take().map(|stderr| tokio::spawn(drain_stderr(stderr, "thumbnails")));
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(frames).await?;
    }
    let status = tokio::time::timeout(Duration::from_secs(60), child.wait()).await??;
    if !status.success() {
        let tail = match errors {
            Some(task) => task.await.unwrap_or_default(),
            None => Vec::new(),
        };
        anyhow::bail!("ffmpeg stopped: {}", tail.last().map_or("no reason given", String::as_str));
    }
    if !out.exists() {
        anyhow::bail!("ffmpeg wrote nothing");
    }
    Ok(())
}

/// A thumbnails job's request for a remote title: the one before it ends
/// first, and this one goes to the provider, on a new connection, once
/// that has closed.
pub(super) async fn feed(shared: &Shared, entry: &Entry, method: &Method, incoming: &HeaderMap) -> Response<Body> {
    let Some(gate) = entry.feed.clone() else {
        return plain(StatusCode::NOT_FOUND, "Not found");
    };
    entry.halt();
    let Ok(Ok(permit)) = tokio::time::timeout(FEED_WAIT, gate.acquire_owned()).await else {
        return plain(StatusCode::SERVICE_UNAVAILABLE, "The title's last request hasn't closed yet.");
    };
    let response = pass_through(&shared.feed_http, &entry.target, None, method, incoming, Some(entry.halted())).await;
    // The permit goes when the body does, so the next request waits for it.
    let (parts, body) = response.into_parts();
    let body = futures_util::StreamExt::map(body.into_data_stream(), move |chunk| {
        let _ = &permit;
        chunk
    });
    Response::from_parts(parts, Body::from_stream(body))
}

/// A sheet, for the interface. Keys and names are checked, so nothing
/// outside the thumbnails folder can be asked for.
pub(super) async fn serve(shared: &Shared, key: &str, name: &str, method: &Method, incoming: &HeaderMap) -> Response<Body> {
    let Some(cache) = lock(&shared.previews.cache).clone() else {
        return plain(StatusCode::NOT_FOUND, "Not found");
    };
    let valid_key = key.len() == 16 && key.bytes().all(|b| b.is_ascii_hexdigit());
    let valid_name = name
        .strip_suffix(".jpg")
        .is_some_and(|n| !n.is_empty() && n.len() <= 4 && n.bytes().all(|b| b.is_ascii_digit()));
    if !valid_key || !valid_name {
        return plain(StatusCode::NOT_FOUND, "Not found");
    }
    let mut response = serve_file(cache.path(key, name), method, incoming).await;
    if response.status() == StatusCode::NOT_FOUND {
        return plain(StatusCode::NOT_FOUND, "Those thumbnails have gone.");
    }
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    // A title's key changes when the title does, so a sheet never changes.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=604800, immutable"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_are_spaced_by_length() {
        let episode = Plan::new(22.0 * 60.0, Some(16.0 / 9.0), false);
        assert_eq!((episode.interval, episode.count), (5.0, 264));
        assert_eq!((episode.width, episode.height), (160, 90));
        let film = Plan::new(2.0 * 3600.0, Some(2.39), false);
        assert_eq!((film.interval, film.count, film.height), (20.0, 360, 66));
        assert_eq!(film.sheets(), 4);
        assert_eq!(film.layout().rows, 10);
        // A remote film has fewer, so its server sees fewer requests.
        let remote = Plan::new(2.0 * 3600.0, None, true);
        assert_eq!((remote.interval, remote.count, remote.height), (60.0, 120, 90));
        let short = Plan::new(45.0, Some(4.0 / 3.0), false);
        assert_eq!((short.interval, short.count, short.height), (2.0, 22, 120));
        assert_eq!(short.layout().rows, 3, "a small title's one sheet has only the rows it needs");
        let very_long = Plan::new(400.0 * 3600.0, None, false);
        assert!(very_long.count <= MOST_TILES && very_long.interval >= 3600.0);
        assert_eq!(short.time_of(0), 1.0);
        assert_eq!(black(&short).len(), short.frame_bytes());
    }

    #[test]
    fn tile_arguments_seek_to_keyframes_and_carry_headers() {
        let plan = Plan::new(600.0, None, true);
        let film = StreamTarget {
            url: "https://a.example/movie/u/p/1.mkv".into(),
            user_agent: Some("UA".into()),
            headers: vec![],
            is_live: false,
        };
        let input = Input::of(&film).unwrap();
        let args = tile_args(&film, &input, "0:0", 75.0, &plan, true).join(" ");
        assert!(args.contains("-threads 1 -protocol_whitelist http,https,tls,tcp,crypto,data,httpproxy -user_agent UA -rw_timeout 15000000 -noaccurate_seek -skip_frame nokey -ss 75.000 -i https://a.example/movie/u/p/1.mkv"), "{args}");
        assert!(args.ends_with("-map 0:0 -an -sn -dn -frames:v 1 -vf scale=160:90:flags=bicubic,setsar=1 -pix_fmt yuv420p -f rawvideo pipe:1"), "{args}");
        let show = if cfg!(windows) { r"C:\rec\show.ts" } else { "/rec/show.ts" };
        let recording = StreamTarget { url: show.into(), user_agent: None, headers: vec![], is_live: false };
        let input = Input::of(&recording).unwrap();
        let args = tile_args(&recording, &input, "0:V:0", 1.0, &plan, false).join(" ");
        assert!(args.contains(&format!("-threads 1 -ss 1.000 -i {show}")), "decoded up to the moment: {args}");
    }

    #[test]
    fn keys_follow_files_and_titles() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("show.ts");
        std::fs::write(&file, b"one").unwrap();
        let local = Input::File(file.clone());
        let first = title_key(&local, "", 60.0).unwrap();
        assert_eq!(first.len(), 16);
        assert_eq!(title_key(&local, "ignored", 61.0).unwrap(), first, "a file is known by itself");
        std::fs::write(&file, b"changed").unwrap();
        assert_ne!(title_key(&local, "", 60.0).unwrap(), first, "a changed file is a new title");

        let web = Input::Web(url::Url::parse("https://a.example/x.mkv?token=1").unwrap());
        let film = title_key(&web, "src|movie|7", 5400.0).unwrap();
        let moved = Input::Web(url::Url::parse("https://b.example/x.mkv?token=2").unwrap());
        assert_eq!(title_key(&moved, "src|movie|7", 5400.2).unwrap(), film, "a new address is the same film");
        assert_ne!(title_key(&web, "src|movie|8", 5400.0).unwrap(), film);
        assert_eq!(title_key(&web, " ", 5400.0), None);
    }

    #[test]
    fn the_interface_reads_them_as_types_ts_says() {
        let json = |t: &Thumbnails| serde_json::to_value(t).unwrap();
        assert_eq!(json(&Thumbnails::Making), serde_json::json!({ "state": "making" }));
        assert_eq!(json(&unavailable("Live channels have no thumbnails.")), serde_json::json!({ "state": "unavailable", "reason": "Live channels have no thumbnails." }));
        let sheet = ThumbnailSheet { images: vec!["http://127.0.0.1:1/t/p/k/0.jpg".into()], interval_seconds: 5.0, count: 3, columns: 10, rows: 1, width: 160, height: 90 };
        assert_eq!(
            json(&Thumbnails::Ready { sheet }),
            serde_json::json!({ "state": "ready", "sheet": { "images": ["http://127.0.0.1:1/t/p/k/0.jpg"], "intervalSeconds": 5.0, "count": 3, "columns": 10, "rows": 1, "width": 160, "height": 90 } })
        );
    }

    #[test]
    fn a_second_connection_only_where_the_source_allows_it() {
        assert!(second_connection(Allowance::Streams(1), 0).unwrap_err().contains("one stream at a time"));
        assert!(second_connection(Allowance::Streams(0), 0).is_err());
        assert!(second_connection(Allowance::Unknown, 0).is_err());
        assert!(second_connection(Allowance::Streams(2), 0).is_ok());
        assert!(second_connection(Allowance::Streams(4), 1).unwrap_err().contains("recording"));
        assert!(second_connection(Allowance::Unlimited, 0).is_ok());
        assert!(second_connection(Allowance::Unlimited, 1).is_err(), "a recording still comes first");
    }
}
