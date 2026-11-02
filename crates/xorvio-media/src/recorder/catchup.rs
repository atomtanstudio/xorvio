//! Saving a programme that already aired from the provider's catch-up.
//! ffmpeg reads the replay as fast as the provider sends it (it isn't tied
//! to the clock as a live recording is), copying it into MPEG-TS for the
//! programme's length. A replay that breaks off is asked for again from
//! where it stopped, into a new part, a few times at most; the parts are
//! joined at the end like a live recording's.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStdout;
use tokio::sync::watch;
use xorvio_model::{RecordingStatus, StreamTarget};

use super::job::{duration_text, explain, finish};
use super::store::{Progress, Row};
use super::{Command, Recorder, files, folders};
use crate::process::{Input, drain_stderr, header_args, redact, tool_command};
use crate::tools::Tools;

/// Replays that give nothing are asked for this many times in a row.
const ATTEMPTS: u32 = 3;
const RETRY: Duration = Duration::from_secs(3);
/// How often the file is measured, and how often that reaches the interface.
const SAMPLE: Duration = Duration::from_secs(1);
const REPORT: Duration = Duration::from_secs(5);
/// Less than this left to save is close enough to the end.
const CLOSE_ENOUGH: f64 = 2.0;

enum Part {
    /// ffmpeg finished: the replay ended or the length was reached.
    Ended { problem: Option<String> },
    /// Stopped or deleted.
    Told(Command),
}

pub(super) async fn run(recorder: Arc<Recorder>, row: Row, mut commands: watch::Receiver<Command>) {
    let id = row.recording.id.clone();
    let Some(tools) = recorder.tools.clone() else {
        return;
    };
    let from = row.recording.start - row.recording.padding_before_seconds;
    let until = row.recording.end + row.recording.padding_after_seconds;
    let wanted = (until - from).max(1) as f64;
    let mut progress = Progress {
        parts: row.parts.clone(),
        file_size_bytes: row.parts.iter().map(|p| files::size_of(p)).sum(),
        recorded_seconds: row.recording.recorded_seconds.unwrap_or(0.0),
        covered_seconds: row.covered_seconds,
    };
    let (mut dir, base) = recorder.part_folder(&row);
    let mut failures = 0;
    let mut problem: Option<String> = None;

    let outcome = loop {
        let command = *commands.borrow();
        if command != Command::Run {
            break command;
        }
        let left = wanted - progress.covered_seconds;
        if left < CLOSE_ENOUGH || failures >= ATTEMPTS {
            break Command::Run;
        }
        if recorder.current_row(&id).is_none() {
            break Command::Abort {
                delete_files: false,
            };
        }

        // Asked for from where the last part stopped.
        let ask_from = from + progress.covered_seconds.floor() as i64;
        let resolver = recorder.resolver.clone();
        let (source, channel) = (
            row.recording.source_id.clone(),
            row.recording.channel_id.clone(),
        );
        let target = match tokio::task::spawn_blocking(move || {
            resolver.replay(&source, &channel, ask_from, until)
        })
        .await
        {
            Ok(Ok(target)) => target,
            Ok(Err(error)) => {
                problem = Some(format!(
                    "The programme couldn't be found in catch-up: {}",
                    redact(&format!("{error:#}"))
                ));
                failures += 1;
                if let Some(command) = wait(&mut commands).await {
                    break command;
                }
                continue;
            }
            Err(_) => {
                break Command::Abort {
                    delete_files: false,
                };
            }
        };

        if !folders::writable(&dir) {
            dir = recorder.folder_for(&row.recording);
            if !folders::writable(&dir) {
                problem = Some("The recordings folder couldn't be used.".into());
                break Command::Run;
            }
        }
        let path = files::next_part(&dir, &base, progress.parts.len() + 1);
        progress.parts.push(path.to_string_lossy().into_owned());
        recorder.save_progress(&id, &progress);

        let before = progress.covered_seconds;
        let part = save_part(
            &recorder,
            &id,
            &tools,
            &target,
            &path,
            left,
            &mut progress,
            &mut commands,
        )
        .await;

        if files::size_of(&path.to_string_lossy()) == 0 {
            files::remove(&[path.to_string_lossy().into_owned()]);
            progress.parts.pop();
            recorder.save_progress(&id, &progress);
        }
        match part {
            Part::Told(command) => break command,
            Part::Ended { problem: why } => {
                if progress.covered_seconds - before < 1.0 {
                    failures += 1;
                } else {
                    failures = 0;
                }
                if wanted - progress.covered_seconds >= CLOSE_ENOUGH {
                    tracing::info!(recording = %id, "a catch-up replay ended early");
                    problem = why.or(problem);
                    if let Some(command) = wait(&mut commands).await {
                        break command;
                    }
                }
            }
        }
    };

    if let Command::Abort { delete_files } = outcome {
        if delete_files {
            files::remove(&progress.parts);
        }
        recorder.forget_job(&id);
        return;
    }
    let Some(row) = recorder.current_row(&id) else {
        recorder.forget_job(&id);
        return;
    };
    progress.parts.retain(|p| files::size_of(p) > 0);
    if progress.parts.len() > 1
        && let Some(joined) = files::join(&tools.ffmpeg, &progress.parts).await
    {
        progress.parts = vec![joined];
    }
    progress.file_size_bytes = progress.parts.iter().map(|p| files::size_of(p)).sum();
    recorder.save_progress(&id, &progress);
    let (status, error) = verdict(&row, &progress, problem.as_deref());
    recorder.conclude(&id, status, error.as_deref());
}

/// Completed, Partial (not all of it came, or it was stopped first) or
/// Failed (nothing came).
pub(super) fn verdict(
    row: &Row,
    progress: &Progress,
    problem: Option<&str>,
) -> (RecordingStatus, Option<String>) {
    if progress.file_size_bytes == 0 {
        let why = problem.unwrap_or("The provider didn't send any of the programme.");
        return (RecordingStatus::Failed, Some(why.to_string()));
    }
    let r = &row.recording;
    let wanted =
        ((r.end + r.padding_after_seconds) - (r.start - r.padding_before_seconds)).max(1) as f64;
    let lost = (wanted - progress.covered_seconds).max(0.0);
    let tolerance = (wanted * 0.02).max(3.0);
    if lost <= tolerance {
        return (RecordingStatus::Completed, None);
    }
    let message = if row.stopped_at.is_some() {
        "Stopped before all of it was saved.".to_string()
    } else {
        let mut message = format!(
            "About {} of the programme wasn't saved.",
            duration_text(lost)
        );
        if let Some(problem) = problem {
            message.push(' ');
            message.push_str(problem);
        }
        message
    };
    (RecordingStatus::Partial, Some(message))
}

/// Waits before asking again; a Stop or Abort cuts the wait short.
async fn wait(commands: &mut watch::Receiver<Command>) -> Option<Command> {
    tokio::select! {
        _ = tokio::time::sleep(RETRY) => None,
        changed = commands.wait_for(|c| *c != Command::Run) => Some(changed.map(|c| *c).unwrap_or(Command::Abort { delete_files: false })),
    }
}

pub(super) fn catchup_args(
    target: &StreamTarget,
    input: &Input,
    seconds: f64,
    path: &Path,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "error",
        "-y",
        "-progress",
        "pipe:1",
    ]
    .map(String::from)
    .to_vec();
    if input.is_web() {
        args.extend(header_args(target));
        // No reconnecting inside ffmpeg: a replay that's opened again starts
        // over, so a break is asked for again from where it stopped instead.
        args.extend(["-rw_timeout", "15000000"].map(String::from));
    }
    args.extend(["-i".to_string(), input.ffmpeg_input()]);
    args.extend(["-t".to_string(), format!("{seconds:.3}")]);
    args.extend(["-map", "0", "-ignore_unknown", "-c", "copy", "-f", "mpegts"].map(String::from));
    args.push(path.to_string_lossy().into_owned());
    args
}

/// Follows ffmpeg's progress report for how much of the programme is in
/// the file so far, in microseconds.
async fn read_progress(stdout: ChildStdout, written: Arc<AtomicI64>) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let value = line
            .strip_prefix("out_time_us=")
            .or_else(|| line.strip_prefix("out_time_ms="));
        if let Some(micros) = value.and_then(|v| v.trim().parse::<i64>().ok())
            && micros >= 0
        {
            written.fetch_max(micros, Ordering::Relaxed);
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn save_part(
    recorder: &Recorder,
    id: &str,
    tools: &Tools,
    target: &StreamTarget,
    path: &Path,
    seconds: f64,
    progress: &mut Progress,
    commands: &mut watch::Receiver<Command>,
) -> Part {
    let Some(input) = Input::of(target) else {
        return Part::Ended {
            problem: Some("The channel's catch-up address isn't one Xorvio can save.".into()),
        };
    };
    let spawned = tool_command(&tools.ffmpeg)
        .args(catchup_args(target, &input, seconds, path))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            tracing::error!("ffmpeg couldn't be started: {error}");
            return Part::Ended {
                problem: Some("ffmpeg couldn't be started.".into()),
            };
        }
    };
    let errors = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(drain_stderr(stderr, "catch-up")));
    let written = Arc::new(AtomicI64::new(0));
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(read_progress(stdout, written.clone()));
    }
    let mut stdin = child.stdin.take();
    let (base_covered, base_recorded, base_size) = (
        progress.covered_seconds,
        progress.recorded_seconds,
        progress.file_size_bytes,
    );
    let measure = |progress: &mut Progress| {
        let part_seconds = (written.load(Ordering::Relaxed) as f64 / 1_000_000.0).min(seconds);
        progress.covered_seconds = base_covered + part_seconds;
        progress.recorded_seconds = base_recorded + part_seconds;
        progress.file_size_bytes = base_size + files::size_of(&path.to_string_lossy());
    };
    let mut sampler = tokio::time::interval(SAMPLE);
    sampler.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_report = Instant::now();

    let ended = loop {
        tokio::select! {
            status = child.wait() => break Ok(status.ok().and_then(|s| s.code())),
            _ = sampler.tick() => {
                measure(progress);
                if last_report.elapsed() >= REPORT {
                    recorder.save_progress(id, progress);
                    recorder.emit(id);
                    last_report = Instant::now();
                }
            }
            changed = commands.changed() => {
                let command = if changed.is_ok() { *commands.borrow() } else { Command::Abort { delete_files: false } };
                match command {
                    Command::Run => {}
                    Command::Stop => {
                        finish(&mut child, stdin.take()).await;
                        break Err(command);
                    }
                    Command::Abort { .. } => {
                        let _ = child.kill().await;
                        break Err(command);
                    }
                }
            }
        }
    };
    // The last progress line comes as ffmpeg exits.
    tokio::time::sleep(Duration::from_millis(100)).await;
    measure(progress);
    recorder.save_progress(id, progress);
    let tail = match errors {
        Some(task) => tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    match ended {
        Err(command) => Part::Told(command),
        Ok(code) => Part::Ended {
            problem: (code != Some(0) || !tail.is_empty()).then(|| explain(&tail, code)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xorvio_model::ScheduleRequest;

    fn row(stopped: Option<i64>) -> Row {
        let mut row = super::super::new_row(
            ScheduleRequest {
                source_id: "s".into(),
                channel_id: "c".into(),
                channel_name: "C".into(),
                channel_logo_url: None,
                title: "T".into(),
                subtitle: None,
                summary: None,
                artwork_url: None,
                program_id: None,
                start: 1000,
                end: 4600,
                padding_before_seconds: 60,
                padding_after_seconds: 180,
            },
            5000,
            940,
        );
        row.recording.from_catchup = true;
        row.stopped_at = stopped;
        row
    }

    fn saved(bytes: i64, seconds: f64) -> Progress {
        Progress {
            parts: vec![],
            file_size_bytes: bytes,
            recorded_seconds: seconds,
            covered_seconds: seconds,
        }
    }

    #[test]
    fn verdicts_judge_the_whole_padded_programme() {
        let r = row(None);
        assert_eq!(
            verdict(
                &r,
                &saved(0, 0.0),
                Some("The programme couldn't be found in catch-up.")
            ),
            (
                RecordingStatus::Failed,
                Some("The programme couldn't be found in catch-up.".into())
            )
        );
        assert_eq!(
            verdict(&r, &saved(10, 3838.0), None).0,
            RecordingStatus::Completed
        );
        let (status, message) = verdict(&r, &saved(10, 3240.0), None);
        assert_eq!(status, RecordingStatus::Partial);
        assert_eq!(
            message.unwrap(),
            "About 10 minutes of the programme wasn't saved."
        );
        assert_eq!(
            verdict(&row(Some(2000)), &saved(10, 600.0), None),
            (
                RecordingStatus::Partial,
                Some("Stopped before all of it was saved.".into())
            )
        );
    }

    #[test]
    fn reads_the_replay_without_reconnecting_for_its_length() {
        let target = StreamTarget {
            url: "http://a.example/timeshift/u/p/64/2026-10-30:20-00/1.ts".into(),
            user_agent: Some("UA".into()),
            headers: vec![],
            is_live: false,
        };
        let input = Input::of(&target).unwrap();
        let args = catchup_args(&target, &input, 3840.0, Path::new("/tmp/x.ts")).join(" ");
        assert!(args.contains("-progress pipe:1"));
        assert!(!args.contains("-reconnect"), "{args}");
        assert!(!args.contains("-re "), "not tied to the clock");
        assert!(args.ends_with(
            "-i http://a.example/timeshift/u/p/64/2026-10-30:20-00/1.ts -t 3840.000 -map 0 -ignore_unknown -c copy -f mpegts /tmp/x.ts"
        ));
    }
}
