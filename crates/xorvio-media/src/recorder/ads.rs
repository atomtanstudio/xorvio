//! Finding recordings' ad breaks: by itself once a recording finishes (when
//! Settings › Recording's ad breaks aren't off), or when asked for an older
//! one. One recording at a time, at a low priority, in the background; what's
//! waiting is noted in recordings.sqlite, so it carries on after a restart.
//! The breaks are kept with the recording, which is only ever read.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use anyhow::bail;
use tokio::sync::{Notify, watch};
use xorvio_model::RecordingStatus;

use super::{Recorder, files, lock};
use crate::adbreaks::{self, AdBreaks};

/// How long the finder sleeps when there's nothing to do before checking
/// the recorder is still there.
const IDLE: Duration = Duration::from_secs(60);

pub(super) struct AdFinder {
    queue: Mutex<Queue>,
    wake: Arc<Notify>,
}

struct Queue {
    waiting: VecDeque<String>,
    /// The recording being looked through, and how to stop it.
    running: Option<(String, watch::Sender<bool>)>,
    /// Its files, when it was deleted meanwhile: they go once ffmpeg (or
    /// comskip) has let go of them, as Windows can't delete a file another
    /// program has open.
    deleted: Vec<String>,
    /// Look through recordings by themselves when they finish.
    automatic: bool,
    comskip: Option<PathBuf>,
}

impl AdFinder {
    pub fn new() -> AdFinder {
        AdFinder {
            queue: Mutex::new(Queue {
                waiting: VecDeque::new(),
                running: None,
                deleted: Vec::new(),
                automatic: true,
                comskip: crate::tools::comskip(),
            }),
            wake: Arc::new(Notify::new()),
        }
    }
}

impl Recorder {
    /// Whether recordings are looked through for ad breaks by themselves
    /// when they finish (on unless Settings › Recording turns ad breaks off).
    pub fn set_find_ad_breaks(&self, automatic: bool) {
        lock(&self.ads.queue).automatic = automatic;
    }

    /// Uses this comskip (or none) rather than the one found at launch.
    #[doc(hidden)]
    pub fn set_comskip(&self, comskip: Option<PathBuf>) {
        lock(&self.ads.queue).comskip = comskip;
    }

    /// What's known of a recording's ad breaks.
    pub fn ad_breaks(&self, id: &str) -> anyhow::Result<AdBreaks> {
        self.store()
            .ad_breaks(id)?
            .ok_or_else(|| anyhow::anyhow!("That recording no longer exists."))
    }

    /// Looks for a finished recording's ad breaks now, ahead of any waiting.
    /// They're found in the background; [`Recorder::ad_breaks`] says when.
    pub fn find_ad_breaks(&self, id: &str) -> anyhow::Result<AdBreaks> {
        let row = self.row(id)?;
        match row.recording.status {
            RecordingStatus::Completed | RecordingStatus::Partial => {}
            RecordingStatus::Recording => bail!("This is still recording. Its ad breaks can be found once it's finished."),
            _ => bail!("This recording has nothing to look through."),
        }
        if !row.recording.file_path.as_deref().is_some_and(|path| Path::new(path).is_file()) {
            bail!("The recording's file isn't there any more.");
        }
        if self.tools.is_none() {
            bail!("Finding ad breaks needs ffmpeg, and Xorvio couldn't find it.");
        }
        self.queue_ads(id, true)?;
        self.ad_breaks(id)
    }

    /// A recording finished: its breaks are looked for, when that's on.
    pub(super) fn recording_ended_for_ads(&self, id: &str, status: RecordingStatus) {
        let automatic = lock(&self.ads.queue).automatic;
        if automatic
            && matches!(status, RecordingStatus::Completed | RecordingStatus::Partial)
            && self.tools.is_some()
            && let Err(error) = self.queue_ads(id, false)
        {
            tracing::warn!("a recording couldn't be queued for its ad breaks: {error:#}");
        }
    }

    /// A recording was deleted: it isn't looked through any more. When it's
    /// being looked through right now, its `files` are deleted once that has
    /// stopped, and this says so; otherwise they're the caller's to delete.
    pub(super) fn stop_finding_ad_breaks(&self, id: &str, files: &[String]) -> bool {
        let mut queue = lock(&self.ads.queue);
        queue.waiting.retain(|waiting| waiting != id);
        let Some((running, stop)) = &queue.running else {
            return false;
        };
        if running != id {
            return false;
        }
        stop.send_replace(true);
        queue.deleted.extend_from_slice(files);
        true
    }

    /// Starts the finder on `runtime`, with what was waiting when Xorvio last quit.
    pub(super) fn start_finding_ad_breaks(self: &Arc<Self>, runtime: &tokio::runtime::Handle) {
        match self.store().queued_ad_scans() {
            Ok(ids) => lock(&self.ads.queue).waiting.extend(ids),
            Err(error) => tracing::warn!("recordings waiting for their ad breaks couldn't be read: {error:#}"),
        }
        runtime.spawn(find_loop(Arc::downgrade(self), self.ads.wake.clone()));
    }

    fn queue_ads(&self, id: &str, first: bool) -> anyhow::Result<()> {
        {
            let mut queue = lock(&self.ads.queue);
            let busy = queue.running.as_ref().is_some_and(|(running, _)| running == id);
            if busy || queue.waiting.iter().any(|waiting| waiting == id) {
                return Ok(());
            }
            if first {
                queue.waiting.push_front(id.to_string());
            } else {
                queue.waiting.push_back(id.to_string());
            }
        }
        self.store().queue_ad_scan(id)?;
        self.ads.wake.notify_one();
        Ok(())
    }

    /// Takes the next recording to look through, marking it running.
    fn next_for_ads(&self) -> Option<(String, watch::Receiver<bool>)> {
        let mut queue = lock(&self.ads.queue);
        if queue.running.is_some() {
            return None;
        }
        let id = queue.waiting.pop_front()?;
        let (stop, stopped) = watch::channel(false);
        queue.running = Some((id.clone(), stop));
        Some((id, stopped))
    }

    async fn look_through(&self, id: &str, stopped: watch::Receiver<bool>) {
        let row = self.current_row(id);
        let file = row
            .as_ref()
            .and_then(|row| row.recording.file_path.clone())
            .filter(|path| Path::new(path).is_file());
        let comskip = lock(&self.ads.queue).comskip.clone();
        let outcome = match (&self.tools, file) {
            _ if row.is_none() => None,
            (None, _) => Some(Err("Finding ad breaks needs ffmpeg, and Xorvio couldn't find it.".to_string())),
            (_, None) => Some(Err("The recording's file isn't there any more.".to_string())),
            (Some(tools), Some(file)) => {
                let started = Instant::now();
                match adbreaks::find(&tools.ffmpeg, comskip.as_deref(), Path::new(&file), stopped.clone()).await {
                    Ok((breaks, by)) => {
                        let seconds = started.elapsed().as_secs_f32();
                        tracing::info!(recording = %id, "found {} ad breaks with {by} in {seconds:.0}s", breaks.len());
                        Some(Ok((breaks, by)))
                    }
                    // Deleted meanwhile: there's nothing to keep them with.
                    Err(_) if *stopped.borrow() => None,
                    Err(error) => Some(Err(format!("{error:#}"))),
                }
            }
        };
        // Saved and finished under the queue's lock, so asking again from
        // now on looks again, and asking meanwhile doesn't.
        let mut queue = lock(&self.ads.queue);
        let saved = match outcome {
            Some(Ok((breaks, by))) => self.store().set_ad_breaks(id, &breaks, by),
            Some(Err(why)) => {
                tracing::info!(recording = %id, "ad breaks couldn't be found: {why}");
                self.store().set_ad_scan_failed(id, &why)
            }
            None => Ok(()),
        };
        if let Err(error) = saved {
            tracing::error!("a recording's ad breaks couldn't be saved: {error:#}");
        }
        queue.running = None;
        let deleted = std::mem::take(&mut queue.deleted);
        drop(queue);
        files::remove(&deleted);
    }
}

/// One recording after another, while there are any.
async fn find_loop(recorder: Weak<Recorder>, wake: Arc<Notify>) {
    loop {
        let Some(me) = recorder.upgrade() else { break };
        match me.next_for_ads() {
            Some((id, stopped)) => me.look_through(&id, stopped).await,
            None => {
                drop(me);
                let _ = tokio::time::timeout(IDLE, wake.notified()).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use xorvio_model::{ScheduleRequest, StreamTarget};

    use super::super::{ChannelResolver, new_row, unix_now};
    use super::*;
    use crate::adbreaks::AdBreakState;
    use crate::tools::Tools;

    struct NoChannels;

    impl ChannelResolver for NoChannels {
        fn resolve(&self, _: &str, _: &str) -> anyhow::Result<StreamTarget> {
            bail!("no channels here")
        }
    }

    /// A recorder (not started, so nothing runs) with a recording in it.
    fn recorder(dir: &Path) -> Arc<Recorder> {
        // Never run: only whether there is an ffmpeg matters here.
        let tools = Tools { ffmpeg: "/nonexistent/ffmpeg".into(), ffprobe: "/nonexistent/ffprobe".into() };
        let recorder = Recorder::open(dir, dir.join("rec"), Some(tools), Arc::new(NoChannels)).unwrap();
        let now = unix_now();
        let request = ScheduleRequest {
            source_id: "s".into(),
            channel_id: "s|ch|1".into(),
            channel_name: "One".into(),
            channel_logo_url: None,
            title: "Show".into(),
            subtitle: None,
            summary: None,
            artwork_url: None,
            program_id: None,
            start: now - 3600,
            end: now - 60,
            padding_before_seconds: 0,
            padding_after_seconds: 0,
        };
        let mut row = new_row(request, now - 3600, now - 3600);
        row.recording.id = "show".into();
        row.recording.status = RecordingStatus::Recording;
        recorder.store().insert(&row).unwrap();
        recorder
    }

    #[test]
    fn finished_recordings_are_looked_through_unless_ad_breaks_are_off() {
        let dir = tempfile::tempdir().unwrap();
        let on = recorder(dir.path());
        assert_eq!(on.ad_breaks("show").unwrap().state, AdBreakState::Never);
        on.conclude("show", RecordingStatus::Completed, None);
        assert_eq!(on.ad_breaks("show").unwrap().state, AdBreakState::Finding);
        assert_eq!(lock(&on.ads.queue).waiting, ["show"]);
        // Deleting it takes it out of the queue.
        on.delete("show", false).unwrap();
        assert!(lock(&on.ads.queue).waiting.is_empty());

        let dir = tempfile::tempdir().unwrap();
        let off = recorder(dir.path());
        off.set_find_ad_breaks(false);
        off.conclude("show", RecordingStatus::Partial, None);
        assert_eq!(off.ad_breaks("show").unwrap().state, AdBreakState::Never, "off: only when asked");

        let dir = tempfile::tempdir().unwrap();
        let failed = recorder(dir.path());
        failed.conclude("show", RecordingStatus::Failed, Some("No video arrived."));
        assert_eq!(failed.ad_breaks("show").unwrap().state, AdBreakState::Never, "nothing to look through");
        assert!(failed.find_ad_breaks("show").is_err());
    }

    #[tokio::test]
    async fn a_recording_deleted_while_looked_through_goes_once_the_finder_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let busy = recorder(dir.path());
        let file = dir.path().join("Show.ts");
        std::fs::write(&file, b"recorded").unwrap();
        let path = file.to_string_lossy().into_owned();
        let progress = super::super::store::Progress { parts: vec![path.clone()], file_size_bytes: 8, recorded_seconds: 1.0, covered_seconds: 1.0 };
        busy.store().set_progress("show", &progress).unwrap();
        busy.store().set_status("show", RecordingStatus::Completed, None).unwrap();
        let (stop, stopped) = watch::channel(false);
        lock(&busy.ads.queue).running = Some(("show".into(), stop));

        busy.delete("show", true).unwrap();
        assert!(*stopped.borrow(), "the finder is told to stop");
        assert!(file.exists(), "left for the finder, which may still have it open");
        assert_eq!(lock(&busy.ads.queue).deleted, std::slice::from_ref(&path));

        // The finder stops (its ffmpeg gone), then deletes it.
        busy.look_through("show", stopped).await;
        assert!(!file.exists());
        assert!(lock(&busy.ads.queue).running.is_none());
        assert!(lock(&busy.ads.queue).deleted.is_empty());

        // One that isn't being looked through goes at once.
        let dir = tempfile::tempdir().unwrap();
        let idle = recorder(dir.path());
        let file = dir.path().join("Other.ts");
        std::fs::write(&file, b"recorded").unwrap();
        let progress = super::super::store::Progress { parts: vec![file.to_string_lossy().into_owned()], file_size_bytes: 8, recorded_seconds: 1.0, covered_seconds: 1.0 };
        idle.store().set_progress("show", &progress).unwrap();
        idle.store().set_status("show", RecordingStatus::Completed, None).unwrap();
        idle.delete("show", true).unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn asking_needs_a_finished_recording_whose_file_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path());
        let still = recorder.find_ad_breaks("show").unwrap_err();
        assert!(still.to_string().contains("still recording"), "{still}");
        recorder.store().set_status("show", RecordingStatus::Completed, None).unwrap();
        let missing = recorder.find_ad_breaks("show").unwrap_err();
        assert!(missing.to_string().contains("isn't there"), "{missing}");
        assert!(recorder.find_ad_breaks("gone").is_err());
    }
}
