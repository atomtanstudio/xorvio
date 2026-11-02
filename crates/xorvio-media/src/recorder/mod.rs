//! The recorder: schedules recordings, starts them `padding_before` early and
//! stops them `padding_after` late, and records with ffmpeg into MPEG-TS
//! (robust against a cut-off end). A stream that drops is reopened into a
//! new part; a recording that lost time is kept as *Partial*, never thrown
//! away, and only one that captured nothing at all is *Failed*.
//!
//! Series recordings (rules.rs) schedule every airing of a programme the
//! guide lists. Programmes that already aired can be saved from the
//! provider's catch-up (catchup.rs). Each recording goes to its source's
//! folder, or the recordings folder (folders.rs).
//!
//! Once a recording finishes, its ad breaks are looked for in the
//! background (ads.rs), so the player can skip them.
//!
//! Recordings need Xorvio running. At launch, ones whose time passed while
//! it was closed are marked as missed, and ones still on carry on.

mod ads;
mod catchup;
mod files;
mod folders;
mod job;
mod rules;
mod store;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use tokio::sync::{Notify, watch};
use xorvio_model::{Recording, RecordingStatus, ScheduleRequest, StreamTarget, Timestamp};

use crate::tools::Tools;
pub use folders::RecordingFolders;
pub use rules::{Airing, NewSeriesRule, SeriesGuide};
use store::{Progress, Row, Store};

/// Finds the stream to record for a channel, at the moment it's needed (and
/// again after a drop, since provider addresses can expire).
pub trait ChannelResolver: Send + Sync {
    fn resolve(&self, source_id: &str, channel_id: &str) -> anyhow::Result<StreamTarget>;

    /// The provider's catch-up of a channel from `start` to `end`, for
    /// saving a programme that already aired.
    fn replay(
        &self,
        _source_id: &str,
        _channel_id: &str,
        _start: Timestamp,
        _end: Timestamp,
    ) -> anyhow::Result<StreamTarget> {
        bail!("This channel can't be replayed.")
    }

    /// How many streams the source's provider allows at once, when it says.
    fn max_connections(&self, _source_id: &str) -> Option<i64> {
        None
    }
}

type OnChange = Arc<dyn Fn(Recording) + Send + Sync>;
type OnListChange = Arc<dyn Fn() + Send + Sync>;

/// How often the scheduler looks at what's due.
const TICK: Duration = Duration::from_secs(2);
/// Record Now with no programme end records this long, unless stopped.
const DEFAULT_LENGTH: i64 = 4 * 3600;

const NOT_RUNNING: &str = "Xorvio wasn't running (or the computer was asleep) when this was on.";
const CLOSED_PART_WAY: &str = "Xorvio was closed for part of it.";
const NO_FFMPEG: &str = "Recording needs ffmpeg, and Xorvio couldn't find it.";

/// What a running recording is told to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    Run,
    /// Finish and keep what was recorded.
    Stop,
    /// Kill ffmpeg at once; the recording was deleted.
    Abort {
        delete_files: bool,
    },
}

/// The recorder. Shared as `Arc<Recorder>`.
pub struct Recorder {
    me: Weak<Recorder>,
    store: Mutex<Store>,
    folders: Mutex<RecordingFolders>,
    tools: Option<Tools>,
    resolver: Arc<dyn ChannelResolver>,
    jobs: Mutex<HashMap<String, watch::Sender<Command>>>,
    wake: Arc<Notify>,
    on_change: RwLock<Option<OnChange>>,
    on_list_change: RwLock<Option<OnListChange>>,
    runtime: OnceLock<tokio::runtime::Handle>,
    /// Where series find their airings.
    guide: RwLock<Option<Arc<dyn SeriesGuide>>>,
    /// Settings › Recording's padding, in seconds, for series without their own.
    padding: Mutex<(i64, i64)>,
    /// Providers' stream allowances, as last asked.
    limits: Mutex<HashMap<String, Option<i64>>>,
    /// Held while series look in the guide, so two looks can't both schedule an airing.
    evaluating: Mutex<()>,
    /// Finished recordings waiting to have their ad breaks found.
    ads: ads::AdFinder,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Recorder {
    /// Opens `recordings.sqlite` in `data_dir` and settles recordings whose
    /// time passed while Xorvio was closed. Nothing records until [`Recorder::start`].
    pub fn open(
        data_dir: &Path,
        recordings_dir: PathBuf,
        tools: Option<Tools>,
        resolver: Arc<dyn ChannelResolver>,
    ) -> anyhow::Result<Arc<Recorder>> {
        let store = Store::open(data_dir)?;
        for row in store.pending()? {
            // Saving from catch-up isn't tied to the clock: it carries on.
            if !row.recording.from_catchup && window_end(&row.recording) <= unix_now() {
                settle_missed(&store, &row)?;
            }
        }
        Ok(Arc::new_cyclic(|me| Recorder {
            me: me.clone(),
            store: Mutex::new(store),
            folders: Mutex::new(RecordingFolders::single(recordings_dir)),
            tools,
            resolver,
            jobs: Mutex::default(),
            wake: Arc::new(Notify::new()),
            on_change: RwLock::new(None),
            on_list_change: RwLock::new(None),
            runtime: OnceLock::new(),
            guide: RwLock::new(None),
            padding: Mutex::new((60, 180)),
            limits: Mutex::default(),
            evaluating: Mutex::new(()),
            ads: ads::AdFinder::new(),
        }))
    }

    /// Starts the scheduler on the current Tokio runtime; `on_change` hears
    /// of every change (and of progress every few seconds while recording).
    /// Series look in the guide straight away and every 15 minutes.
    /// Calling it again only replaces `on_change`.
    pub fn start(self: &Arc<Self>, on_change: Arc<dyn Fn(Recording) + Send + Sync>) {
        *self.on_change.write().unwrap_or_else(|e| e.into_inner()) = Some(on_change);
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::error!("the recorder needs a Tokio runtime to start");
            return;
        };
        if self.runtime.set(runtime.clone()).is_err() {
            return;
        }
        self.start_finding_ad_breaks(&runtime);
        let recorder = Arc::downgrade(self);
        let wake = self.wake.clone();
        runtime.spawn(async move {
            let mut series_looked: Option<Instant> = None;
            loop {
                match recorder.upgrade() {
                    Some(recorder) => {
                        recorder.tick();
                        if series_looked.is_none_or(|at| at.elapsed() >= rules::EVERY) {
                            recorder.evaluate_rules_soon();
                            series_looked = Some(Instant::now());
                        }
                    }
                    None => break,
                }
                let _ = tokio::time::timeout(TICK, wake.notified()).await;
            }
        });
    }

    /// Every recording, by start time, with clashes marked.
    pub fn list(&self) -> anyhow::Result<Vec<Recording>> {
        let rows = self.store().list()?;
        Ok(self.with_conflicts(rows.into_iter().map(|row| row.recording).collect()))
    }

    /// Schedules a programme. One that's already on (within its padding)
    /// starts at once; the same programme scheduled twice is the same recording.
    pub fn schedule(&self, request: ScheduleRequest) -> anyhow::Result<Recording> {
        let now = unix_now();
        if request.end <= request.start {
            bail!("That programme's times don't make sense.");
        }
        if request.end + request.padding_after_seconds.max(0) <= now {
            bail!("That programme has already ended.");
        }
        if let Some(existing) = self.existing(&request)? {
            return Ok(existing);
        }
        let wanted_from = request.start.max(now);
        let row = new_row(request, now, wanted_from);
        self.store().insert(&row)?;
        self.emit(&row.recording.id);
        if now >= row.recording.start - row.recording.padding_before_seconds {
            self.start_job(row.clone());
        }
        // Others it overlaps may clash now.
        self.list_changed();
        self.get(&row.recording.id)
    }

    /// Records a channel from now until the programme's end (plus padding),
    /// or for a few hours when there's no end, unless stopped.
    pub fn record_now(&self, mut request: ScheduleRequest) -> anyhow::Result<Recording> {
        let now = unix_now();
        request.start = request.start.min(now);
        if request.end <= now {
            request.end = now + DEFAULT_LENGTH;
        }
        if let Some(existing) = self.existing(&request)? {
            return Ok(existing);
        }
        let row = new_row(request, now, now);
        self.store().insert(&row)?;
        self.emit(&row.recording.id);
        self.start_job(row.clone());
        self.list_changed();
        self.get(&row.recording.id)
    }

    /// Saves a programme that already aired from the channel's catch-up,
    /// with whatever padding the caller found the archive allows. It starts
    /// at once and goes as fast as the provider sends it.
    pub fn record_catchup(&self, request: ScheduleRequest) -> anyhow::Result<Recording> {
        let now = unix_now();
        if request.end <= request.start {
            bail!("That programme's times don't make sense.");
        }
        if request.end > now {
            bail!("That programme hasn't finished yet. Record it as it airs instead.");
        }
        let already = self.store().pending()?.into_iter().find(|row| {
            let r = &row.recording;
            r.from_catchup
                && r.channel_id == request.channel_id
                && r.start == request.start
                && r.end == request.end
        });
        if let Some(row) = already {
            return Ok(row.recording);
        }
        let wanted_from = request.start - request.padding_before_seconds.max(0);
        let mut row = new_row(request, now, wanted_from);
        row.recording.from_catchup = true;
        row.recording.status = RecordingStatus::Recording;
        self.store().insert(&row)?;
        self.emit(&row.recording.id);
        self.start_job(row.clone());
        self.list_changed();
        self.get(&row.recording.id)
    }

    /// Stops a recording early, keeping what was recorded. A scheduled one is cancelled.
    pub fn stop(&self, id: &str) -> anyhow::Result<()> {
        let row = self.row(id)?;
        match row.recording.status {
            RecordingStatus::Scheduled => self.cancel(id),
            RecordingStatus::Recording => {
                self.store().set_stopped_at(id, unix_now())?;
                match lock(&self.jobs).get(id) {
                    Some(job) => {
                        job.send_replace(Command::Stop);
                    }
                    // Not running yet (the scheduler hasn't started): settle it now.
                    None => {
                        let row = self.row(id)?;
                        settle_missed(&self.store(), &row)?;
                        self.emit(id);
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Cancels a scheduled recording. A series' airing that's cancelled
    /// isn't scheduled again.
    pub fn cancel(&self, id: &str) -> anyhow::Result<()> {
        let row = self.row(id)?;
        match row.recording.status {
            RecordingStatus::Scheduled => {
                self.store()
                    .set_status(id, RecordingStatus::Cancelled, None)?;
                self.forget_airing(&row);
                self.emit(id);
                self.list_changed();
                Ok(())
            }
            RecordingStatus::Recording => {
                bail!("That recording has already started. Stop it to keep what was recorded.")
            }
            _ => Ok(()),
        }
    }

    /// Deletes a recording, stopping it first if it's running, and its file
    /// when `delete_file` is set.
    pub fn delete(&self, id: &str, delete_file: bool) -> anyhow::Result<()> {
        let Some(row) = self.store().get(id)? else {
            return Ok(());
        };
        let running = lock(&self.jobs).get(id).map(|job| {
            job.send_replace(Command::Abort {
                delete_files: delete_file,
            })
        });
        self.forget_airing(&row);
        self.stop_finding_ad_breaks(id);
        self.store().delete(id)?;
        // A running recording deletes its own files once ffmpeg has let go of them.
        if running.is_none() && delete_file {
            files::remove(&row.parts);
        }
        self.list_changed();
        Ok(())
    }

    /// Where new recordings go. Existing ones stay where they are.
    pub fn set_recordings_dir(&self, dir: PathBuf) -> anyhow::Result<()> {
        std::fs::create_dir_all(&dir)
            .map_err(|e| anyhow!("The recordings folder couldn't be created: {e}"))?;
        lock(&self.folders).main = dir;
        Ok(())
    }

    /// Where new recordings go: their source's folder, the recordings
    /// folder, or Xorvio's own when neither is there. Existing ones stay
    /// where they are.
    pub fn set_folders(&self, folders: RecordingFolders) {
        *lock(&self.folders) = folders;
    }

    /// Recordings running right now.
    pub fn active_count(&self) -> usize {
        lock(&self.jobs).len()
    }

    // MARK: Scheduling

    fn tick(&self) {
        let now = unix_now();
        let pending = match self.store().pending() {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!("recordings couldn't be read: {error:#}");
                return;
            }
        };
        for row in pending {
            let recording = &row.recording;
            if lock(&self.jobs).contains_key(&recording.id) {
                continue;
            }
            if recording.from_catchup {
                // Interrupted by a restart: it picks up where it stopped.
                self.start_job(row);
            } else if now >= window_end(recording) {
                // Its whole window went by without the recorder.
                if let Err(error) = settle_missed(&self.store(), &row) {
                    tracing::error!("a recording couldn't be updated: {error:#}");
                }
                self.emit(&recording.id);
            } else if recording.status == RecordingStatus::Recording
                || now >= recording.start - recording.padding_before_seconds
            {
                self.start_job(row);
            }
        }
    }

    fn start_job(&self, row: Row) {
        let (Some(runtime), Some(me)) = (self.runtime.get(), self.me.upgrade()) else {
            return;
        };
        let id = row.recording.id.clone();
        if self.tools.is_none() {
            let _ = self
                .store()
                .set_status(&id, RecordingStatus::Failed, Some(NO_FFMPEG));
            self.emit(&id);
            return;
        }
        let (sender, commands) = watch::channel(Command::Run);
        {
            let mut jobs = lock(&self.jobs);
            if jobs.contains_key(&id) {
                return;
            }
            jobs.insert(id.clone(), sender);
        }
        if row.recording.status != RecordingStatus::Recording {
            let _ = self
                .store()
                .set_status(&id, RecordingStatus::Recording, None);
            self.emit(&id);
        }
        if row.recording.from_catchup {
            runtime.spawn(catchup::run(me, row, commands));
        } else {
            runtime.spawn(job::run(me, row, commands));
        }
    }

    fn existing(&self, request: &ScheduleRequest) -> anyhow::Result<Option<Recording>> {
        let found = self.store().pending()?.into_iter().find(|row| {
            let r = &row.recording;
            !r.from_catchup
                && r.source_id == request.source_id
                && r.channel_id == request.channel_id
                && match (&r.program_id, &request.program_id) {
                    (Some(a), Some(b)) => a == b,
                    _ => r.start == request.start && r.end == request.end,
                }
        });
        Ok(found.map(|row| row.recording))
    }

    // MARK: Helpers for jobs

    fn store(&self) -> MutexGuard<'_, Store> {
        lock(&self.store)
    }

    fn row(&self, id: &str) -> anyhow::Result<Row> {
        self.store()
            .get(id)?
            .ok_or_else(|| anyhow!("That recording no longer exists."))
    }

    fn get(&self, id: &str) -> anyhow::Result<Recording> {
        let recording = self.row(id)?.recording;
        Ok(self.with_conflicts_of(recording))
    }

    pub(crate) fn current_row(&self, id: &str) -> Option<Row> {
        self.store().get(id).ok().flatten()
    }

    /// The folder for a recording's first part, noting on the recording
    /// when it isn't the one chosen for it.
    pub(crate) fn folder_for(&self, recording: &Recording) -> PathBuf {
        let folders = lock(&self.folders).clone();
        let (dir, note) = folders.choose(&recording.source_id);
        if let Some(note) = note {
            tracing::info!(recording = %recording.id, "the chosen recordings folder wasn't available");
            if let Err(error) = self.store().set_note(&recording.id, Some(&note)) {
                tracing::error!("a recording's note couldn't be saved: {error:#}");
            }
            self.emit(&recording.id);
        }
        dir
    }

    /// Where a recording's next part goes and the name it's given: next to
    /// its first part, named after it, or for a new one, a fresh folder choice.
    pub(crate) fn part_folder(&self, row: &Row) -> (PathBuf, String) {
        match row.parts.first().map(PathBuf::from) {
            Some(first) => (
                first
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.folder_for(&row.recording)),
                first
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| files::base_name(&row.recording)),
            ),
            None => (
                self.folder_for(&row.recording),
                files::base_name(&row.recording),
            ),
        }
    }

    pub(crate) fn save_progress(&self, id: &str, progress: &Progress) {
        if let Err(error) = self.store().set_progress(id, progress) {
            tracing::error!("a recording's progress couldn't be saved: {error:#}");
        }
    }

    /// Records the outcome, tells the interface and wakes the scheduler.
    pub(crate) fn conclude(&self, id: &str, status: RecordingStatus, error: Option<&str>) {
        lock(&self.jobs).remove(id);
        if let Err(store_error) = self.store().set_status(id, status, error) {
            tracing::error!("a recording's result couldn't be saved: {store_error:#}");
        }
        self.emit(id);
        self.series_recording_ended(id);
        self.recording_ended_for_ads(id, status);
        self.wake.notify_one();
    }

    pub(crate) fn forget_job(&self, id: &str) {
        lock(&self.jobs).remove(id);
        self.wake.notify_one();
    }

    /// One recording with whether it clashes, which depends on the others.
    fn with_conflicts_of(&self, recording: Recording) -> Recording {
        if !matches!(
            recording.status,
            RecordingStatus::Scheduled | RecordingStatus::Recording
        ) {
            return recording;
        }
        let mut pending: Vec<Recording> = match self.store().pending() {
            Ok(rows) => rows.into_iter().map(|row| row.recording).collect(),
            Err(_) => return recording,
        };
        if !pending.iter().any(|r| r.id == recording.id) {
            pending.push(recording.clone());
        }
        let id = recording.id.clone();
        self.with_conflicts(pending)
            .into_iter()
            .find(|r| r.id == id)
            .unwrap_or(recording)
    }

    pub(crate) fn emit(&self, id: &str) {
        let Some(row) = self.current_row(id) else {
            return;
        };
        let callback = self
            .on_change
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(callback) = callback {
            callback(self.with_conflicts_of(row.recording));
        }
    }
}

fn window_end(recording: &Recording) -> i64 {
    recording.end + recording.padding_after_seconds.max(0)
}

fn new_row(request: ScheduleRequest, now: i64, wanted_from: i64) -> Row {
    Row {
        recording: Recording {
            id: uuid::Uuid::new_v4().to_string(),
            source_id: request.source_id,
            channel_id: request.channel_id,
            channel_name: request.channel_name,
            channel_logo_url: request.channel_logo_url,
            title: request.title,
            subtitle: request.subtitle,
            summary: request.summary,
            artwork_url: request.artwork_url,
            program_id: request.program_id,
            start: request.start,
            end: request.end,
            padding_before_seconds: request.padding_before_seconds.max(0),
            padding_after_seconds: request.padding_after_seconds.max(0),
            status: RecordingStatus::Scheduled,
            file_path: None,
            file_size_bytes: None,
            recorded_seconds: None,
            error: None,
            created_at: now,
            rule_id: None,
            episode: None,
            from_catchup: false,
            note: None,
            conflict: None,
        },
        parts: Vec::new(),
        covered_seconds: 0.0,
        wanted_from,
        stopped_at: None,
        episode_key: None,
        airing_key: None,
    }
}

/// Settles a recording the recorder didn't (or couldn't) see through: kept
/// as Partial when something was captured, otherwise Failed.
fn settle_missed(store: &Store, row: &Row) -> anyhow::Result<()> {
    let id = &row.recording.id;
    let progress = Progress {
        parts: row
            .parts
            .iter()
            .filter(|p| files::size_of(p) > 0)
            .cloned()
            .collect(),
        file_size_bytes: row.parts.iter().map(|p| files::size_of(p)).sum(),
        recorded_seconds: row.recording.recorded_seconds.unwrap_or(0.0),
        covered_seconds: row.covered_seconds,
    };
    store.set_progress(id, &progress)?;
    let (status, error) = if progress.file_size_bytes > 0 {
        if row.recording.from_catchup {
            catchup::verdict(row, &progress, Some(CLOSED_PART_WAY))
        } else {
            job::verdict(row, &progress, Some(CLOSED_PART_WAY))
        }
    } else {
        (RecordingStatus::Failed, Some(NOT_RUNNING.to_string()))
    };
    store.set_status(id, status, error.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoChannels;

    impl ChannelResolver for NoChannels {
        fn resolve(&self, _: &str, _: &str) -> anyhow::Result<StreamTarget> {
            bail!("no channels here")
        }
    }

    fn request(start: i64, end: i64) -> ScheduleRequest {
        ScheduleRequest {
            source_id: "s".into(),
            channel_id: "s|ch|1".into(),
            channel_name: "One".into(),
            channel_logo_url: None,
            title: "Show".into(),
            subtitle: None,
            summary: None,
            artwork_url: None,
            program_id: None,
            start,
            end,
            padding_before_seconds: 60,
            padding_after_seconds: 180,
        }
    }

    #[test]
    fn settles_recordings_missed_while_closed() {
        let dir = tempfile::tempdir().unwrap();
        let now = unix_now();
        {
            let store = Store::open(dir.path()).unwrap();
            let mut missed = new_row(request(now - 7200, now - 3600), now - 8000, now - 7200);
            missed.recording.id = "missed".into();
            store.insert(&missed).unwrap();

            let part = dir.path().join("part.ts");
            std::fs::write(&part, vec![0x47; 188 * 10]).unwrap();
            let mut cut = new_row(request(now - 7200, now - 3600), now - 8000, now - 7200);
            cut.recording.id = "cut".into();
            cut.recording.status = RecordingStatus::Recording;
            cut.parts = vec![part.to_string_lossy().into_owned()];
            cut.covered_seconds = 600.0;
            store.insert(&cut).unwrap();

            let mut airing = new_row(request(now - 600, now + 600), now - 700, now - 600);
            airing.recording.id = "airing".into();
            airing.recording.status = RecordingStatus::Recording;
            store.insert(&airing).unwrap();

            // Saving a programme from catch-up isn't missed: it carries on.
            let mut saving = new_row(request(now - 7200, now - 3600), now - 100, now - 7260);
            saving.recording.id = "saving".into();
            saving.recording.status = RecordingStatus::Recording;
            saving.recording.from_catchup = true;
            store.insert(&saving).unwrap();
        }
        let recorder = Recorder::open(
            dir.path(),
            dir.path().join("rec"),
            None,
            Arc::new(NoChannels),
        )
        .unwrap();
        let by_id: HashMap<String, Recording> = recorder
            .list()
            .unwrap()
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();
        assert_eq!(by_id["missed"].status, RecordingStatus::Failed);
        assert_eq!(by_id["missed"].error.as_deref(), Some(NOT_RUNNING));
        assert_eq!(by_id["cut"].status, RecordingStatus::Partial);
        assert!(
            by_id["cut"]
                .error
                .as_deref()
                .unwrap()
                .contains(CLOSED_PART_WAY)
        );
        assert_eq!(by_id["cut"].file_size_bytes, Some(1880));
        assert_eq!(
            by_id["airing"].status,
            RecordingStatus::Recording,
            "still on: the scheduler resumes it"
        );
        assert_eq!(by_id["saving"].status, RecordingStatus::Recording);
    }

    #[test]
    fn schedules_cancels_and_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = Recorder::open(
            dir.path(),
            dir.path().join("rec"),
            None,
            Arc::new(NoChannels),
        )
        .unwrap();
        let now = unix_now();
        assert!(recorder.schedule(request(now - 7200, now - 3600)).is_err());
        let scheduled = recorder.schedule(request(now + 3600, now + 7200)).unwrap();
        assert_eq!(scheduled.status, RecordingStatus::Scheduled);
        assert_eq!(
            recorder
                .schedule(request(now + 3600, now + 7200))
                .unwrap()
                .id,
            scheduled.id,
            "same programme, same recording"
        );
        recorder.cancel(&scheduled.id).unwrap();
        assert_eq!(
            recorder.list().unwrap()[0].status,
            RecordingStatus::Cancelled
        );
        recorder.delete(&scheduled.id, true).unwrap();
        assert!(recorder.list().unwrap().is_empty());
        assert_eq!(recorder.active_count(), 0);
    }

    #[test]
    fn catchup_is_only_for_programmes_that_aired() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = Recorder::open(
            dir.path(),
            dir.path().join("rec"),
            None,
            Arc::new(NoChannels),
        )
        .unwrap();
        let now = unix_now();
        let error = recorder
            .record_catchup(request(now - 600, now + 600))
            .unwrap_err();
        assert!(error.to_string().contains("hasn't finished"));
        let saving = recorder
            .record_catchup(request(now - 7200, now - 3600))
            .unwrap();
        assert!(saving.from_catchup);
        assert_eq!(saving.status, RecordingStatus::Recording);
        assert_eq!(
            recorder
                .record_catchup(request(now - 7200, now - 3600))
                .unwrap()
                .id,
            saving.id,
            "the same programme twice is the same save"
        );
    }
}
