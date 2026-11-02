//! One recording, from its first part to its result: run ffmpeg until the
//! end time, reopen the channel into a new part whenever the stream drops,
//! and keep count of what was captured.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin};
use tokio::sync::watch;
use xorvio_model::{RecordingStatus, StreamTarget};

use super::store::{Progress, Row};
use super::{Command, Recorder, files, folders, unix_now};
use crate::process::{Input, drain_stderr, header_args, redact, tool_command};
use crate::tools::Tools;

const MIN_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(10);
/// How often the file is measured.
const SAMPLE: Duration = Duration::from_secs(1);
/// How often progress reaches the interface while recording.
const REPORT: Duration = Duration::from_secs(5);
/// ffmpeg writes in bursts (and nothing at all while it spends its first
/// few seconds analysing a live input, whose packets it then writes at
/// once), so up to this long between writes still counts as recording.
/// Anything longer is a stall, and only this much of it is counted.
const STALL: f64 = 10.0;
/// A part that ran this long reconnected from a real drop, not a refusal.
const PRODUCTIVE: f64 = 30.0;

enum Capture {
    /// The end time came.
    Finished,
    /// Stopped or deleted.
    Told(Command),
    /// ffmpeg gave up before the end.
    Exited { problem: String, productive: bool },
}

pub(super) async fn run(recorder: Arc<Recorder>, row: Row, mut commands: watch::Receiver<Command>) {
    let id = row.recording.id.clone();
    let Some(tools) = recorder.tools.clone() else {
        return;
    };
    let mut progress = Progress {
        parts: row.parts.clone(),
        file_size_bytes: row.parts.iter().map(|p| files::size_of(p)).sum(),
        recorded_seconds: row.recording.recorded_seconds.unwrap_or(0.0),
        covered_seconds: row.covered_seconds,
    };
    let (mut dir, base) = recorder.part_folder(&row);
    let mut backoff = MIN_BACKOFF;
    let mut problem: Option<String> = None;

    let outcome = loop {
        let command = *commands.borrow();
        if command != Command::Run {
            break command;
        }
        let Some(current) = recorder.current_row(&id) else {
            break Command::Abort {
                delete_files: false,
            };
        };
        let window_end = current.recording.end + current.recording.padding_after_seconds;
        if unix_now() >= window_end {
            break Command::Run;
        }

        let resolver = recorder.resolver.clone();
        let (source, channel) = (
            current.recording.source_id.clone(),
            current.recording.channel_id.clone(),
        );
        let target =
            match tokio::task::spawn_blocking(move || resolver.resolve(&source, &channel)).await {
                Ok(Ok(target)) => target,
                Ok(Err(error)) => {
                    problem = Some(format!(
                        "The channel couldn't be opened: {}",
                        redact(&format!("{error:#}"))
                    ));
                    match pause(backoff, window_end, &mut commands).await {
                        Some(command) => break command,
                        None => {
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    }
                }
                Err(_) => {
                    break Command::Abort {
                        delete_files: false,
                    };
                }
            };

        // The folder went away part-way (a drive unplugged): carry on in another.
        if !folders::writable(&dir) {
            dir = recorder.folder_for(&current.recording);
            if !folders::writable(&dir) {
                problem = Some("The recordings folder couldn't be used.".into());
                break Command::Run;
            }
        }
        let path = files::next_part(&dir, &base, progress.parts.len() + 1);
        progress.parts.push(path.to_string_lossy().into_owned());
        recorder.save_progress(&id, &progress);

        let capture = capture(
            &recorder,
            &current,
            &tools,
            &target,
            &path,
            window_end,
            &mut progress,
            &mut commands,
        )
        .await;

        // A part that never received anything isn't kept.
        if files::size_of(&path.to_string_lossy()) == 0 {
            files::remove(&[path.to_string_lossy().into_owned()]);
            progress.parts.pop();
            recorder.save_progress(&id, &progress);
        }
        match capture {
            Capture::Finished => break Command::Run,
            Capture::Told(command) => break command,
            Capture::Exited {
                problem: why,
                productive,
            } => {
                tracing::info!(recording = %id, "the stream dropped: {why}");
                problem = Some(why);
                if productive {
                    backoff = MIN_BACKOFF;
                }
                if let Some(command) = pause(backoff, window_end, &mut commands).await {
                    break command;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
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

/// Completed, Partial (time was lost) or Failed (nothing was captured).
pub(super) fn verdict(
    row: &Row,
    progress: &Progress,
    problem: Option<&str>,
) -> (RecordingStatus, Option<String>) {
    if progress.file_size_bytes == 0 {
        let why = problem.unwrap_or("No video arrived from the channel.");
        return (RecordingStatus::Failed, Some(why.to_string()));
    }
    let until = row
        .stopped_at
        .map_or(row.recording.end, |stopped| stopped.min(row.recording.end));
    let wanted = (until - row.wanted_from).max(0) as f64;
    let lost = (wanted - progress.covered_seconds).max(0.0);
    // A few seconds go to every reconnect; that isn't worth a warning.
    let tolerance = (wanted * 0.02).max(3.0);
    if lost > tolerance {
        let mut message = format!(
            "About {} of the programme wasn't recorded.",
            duration_text(lost)
        );
        if let Some(problem) = problem {
            message.push(' ');
            message.push_str(problem);
        }
        (RecordingStatus::Partial, Some(message))
    } else {
        (RecordingStatus::Completed, None)
    }
}

pub(super) fn duration_text(seconds: f64) -> String {
    let rounded = |value: f64, unit: &str| {
        let n = value.round().max(1.0) as i64;
        format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
    };
    match seconds {
        s if s < 90.0 => rounded(s, "second"),
        s if s < 90.0 * 60.0 => rounded(s / 60.0, "minute"),
        s => rounded(s / 3600.0, "hour"),
    }
}

/// Waits before reopening the stream; a Stop or Abort cuts the wait short.
async fn pause(
    wait: Duration,
    window_end: i64,
    commands: &mut watch::Receiver<Command>,
) -> Option<Command> {
    let left = Duration::from_secs((window_end - unix_now()).max(0) as u64);
    let wait = wait.min(left);
    tokio::select! {
        _ = tokio::time::sleep(wait) => None,
        changed = commands.wait_for(|c| *c != Command::Run) => Some(changed.map(|c| *c).unwrap_or(Command::Abort { delete_files: false })),
    }
}

fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn record_args(target: &StreamTarget, input: &Input, path: &Path) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-nostats", "-loglevel", "error", "-y"]
        .map(String::from)
        .to_vec();
    if input.is_web() {
        args.extend(header_args(target));
        args.extend(
            [
                "-reconnect",
                "1",
                "-reconnect_streamed",
                "1",
                "-reconnect_delay_max",
                "10",
                "-rw_timeout",
                "15000000",
            ]
            .map(String::from),
        );
    }
    args.extend(["-i".to_string(), input.ffmpeg_input()]);
    args.extend(
        [
            "-map",
            "0",
            "-ignore_unknown",
            "-c",
            "copy",
            "-flush_packets",
            "1",
            "-f",
            "mpegts",
        ]
        .map(String::from),
    );
    args.push(path.to_string_lossy().into_owned());
    args
}

/// Counts what the file gained since the last look.
struct Meter {
    part_start_size: i64,
    last_size: i64,
    /// Up to when time has been counted (or skipped as a stall).
    counted_until: f64,
    wanted_from: f64,
    wanted_until: f64,
    first_growth: Option<f64>,
}

impl Meter {
    fn sample(&mut self, path: &Path, progress: &mut Progress) {
        let now = now_seconds();
        let size = files::size_of(&path.to_string_lossy());
        if size > self.last_size {
            let from = self.counted_until.max(now - STALL);
            progress.recorded_seconds += now - from;
            progress.covered_seconds +=
                (now.min(self.wanted_until) - from.max(self.wanted_from)).max(0.0);
            self.counted_until = now;
            self.first_growth.get_or_insert(now);
        } else if now - self.counted_until > STALL {
            self.counted_until = now - STALL;
        }
        self.last_size = size;
        progress.file_size_bytes = self.part_start_size + size;
    }
}

#[allow(clippy::too_many_arguments)]
async fn capture(
    recorder: &Recorder,
    row: &Row,
    tools: &Tools,
    target: &StreamTarget,
    path: &Path,
    window_end: i64,
    progress: &mut Progress,
    commands: &mut watch::Receiver<Command>,
) -> Capture {
    let id = &row.recording.id;
    let Some(input) = Input::of(target) else {
        return Capture::Exited {
            problem: "The channel's address isn't one Xorvio can record.".into(),
            productive: false,
        };
    };
    let spawned = tool_command(&tools.ffmpeg)
        .args(record_args(target, &input, path))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            tracing::error!("ffmpeg couldn't be started: {error}");
            return Capture::Exited {
                problem: "ffmpeg couldn't be started.".into(),
                productive: false,
            };
        }
    };
    let errors = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(drain_stderr(stderr, "record")));
    // Held apart from `child`: waiting on a tokio child closes its stdin, and
    // stdin is how ffmpeg is asked to finish.
    let mut stdin = child.stdin.take();
    let started = now_seconds();
    let mut meter = Meter {
        part_start_size: progress.file_size_bytes,
        last_size: 0,
        counted_until: started,
        wanted_from: row.wanted_from as f64,
        wanted_until: row.recording.end as f64,
        first_growth: None,
    };
    let mut sampler = tokio::time::interval(SAMPLE);
    sampler.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_report = Instant::now();

    let ended = loop {
        tokio::select! {
            status = child.wait() => break Ended::Exited(status.ok().and_then(|s| s.code())),
            _ = sampler.tick() => {
                meter.sample(path, progress);
                if last_report.elapsed() >= REPORT {
                    recorder.save_progress(id, progress);
                    recorder.emit(id);
                    last_report = Instant::now();
                }
                if unix_now() >= window_end {
                    finish(&mut child, stdin.take()).await;
                    break Ended::Finished;
                }
            }
            changed = commands.changed() => {
                let command = if changed.is_ok() { *commands.borrow() } else { Command::Abort { delete_files: false } };
                match command {
                    Command::Run => {}
                    Command::Stop => {
                        finish(&mut child, stdin.take()).await;
                        break Ended::Told(command);
                    }
                    Command::Abort { .. } => {
                        let _ = child.kill().await;
                        break Ended::Told(command);
                    }
                }
            }
        }
    };
    meter.sample(path, progress);
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
        Ended::Finished => Capture::Finished,
        Ended::Told(command) => Capture::Told(command),
        Ended::Exited(code) => {
            let productive = meter
                .first_growth
                .is_some_and(|first| meter.counted_until - first >= PRODUCTIVE);
            Capture::Exited {
                problem: explain(&tail, code),
                productive,
            }
        }
    }
}

enum Ended {
    Finished,
    Told(Command),
    Exited(Option<i32>),
}

/// Asks ffmpeg to finish its file (`q`), and kills it if it doesn't.
pub(super) async fn finish(child: &mut Child, stdin: Option<ChildStdin>) {
    if let Some(mut stdin) = stdin {
        let _ = stdin.write_all(b"q").await;
        let _ = stdin.flush().await;
    }
    if tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
}

/// ffmpeg's complaint in words for people.
pub(super) fn explain(tail: &[String], code: Option<i32>) -> String {
    let text = tail.join("\n").to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
    if has(&["401", "403", "forbidden", "unauthorized"]) {
        "The provider refused the stream. The subscription may have expired or reached its connection limit.".into()
    } else if has(&["404", "not found"]) {
        "The provider said the channel isn't available right now.".into()
    } else if has(&["server returned 5", "5xx"]) {
        "The provider's server had a problem.".into()
    } else if has(&[
        // ffmpeg's line for any connection that failed, whatever the reason
        // (on Windows, "Error number -10061 occurred" for one refused), with
        // its address already taken out.
        "connection to <address> failed",
        "connection refused",
        "timed out",
        "timeout",
        "resolve",
        "unreachable",
        "no route",
        "connection reset",
        "broken pipe",
    ]) {
        "The channel's stream couldn't be reached.".into()
    } else if has(&["invalid data", "could not find codec", "no such file"]) {
        "The channel sent something that isn't video.".into()
    } else if code.is_some_and(|c| c != 0) || !text.is_empty() {
        "The channel's stream stopped.".into()
    } else {
        "The channel's stream ended.".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xorvio_model::Recording;

    fn row(start: i64, end: i64, wanted_from: i64, stopped_at: Option<i64>) -> Row {
        Row {
            recording: Recording {
                id: "r".into(),
                source_id: "s".into(),
                channel_id: "c".into(),
                channel_name: "C".into(),
                channel_logo_url: None,
                title: "T".into(),
                subtitle: None,
                summary: None,
                artwork_url: None,
                program_id: None,
                start,
                end,
                padding_before_seconds: 0,
                padding_after_seconds: 0,
                status: RecordingStatus::Recording,
                file_path: None,
                file_size_bytes: None,
                recorded_seconds: None,
                error: None,
                created_at: start,
                rule_id: None,
                episode: None,
                from_catchup: false,
                note: None,
                conflict: None,
            },
            parts: vec![],
            covered_seconds: 0.0,
            wanted_from,
            stopped_at,
            episode_key: None,
            airing_key: None,
        }
    }

    fn progress(bytes: i64, covered: f64) -> Progress {
        Progress {
            parts: vec![],
            file_size_bytes: bytes,
            recorded_seconds: covered,
            covered_seconds: covered,
        }
    }

    #[test]
    fn verdicts() {
        let hour = row(0, 3600, 0, None);
        assert_eq!(
            verdict(&hour, &progress(0, 0.0), Some("Gone.")),
            (RecordingStatus::Failed, Some("Gone.".into()))
        );
        assert_eq!(
            verdict(&hour, &progress(10, 3590.0), None).0,
            RecordingStatus::Completed
        );
        let (status, message) = verdict(
            &hour,
            &progress(10, 3000.0),
            Some("The channel's stream stopped."),
        );
        assert_eq!(status, RecordingStatus::Partial);
        assert_eq!(
            message.unwrap(),
            "About 10 minutes of the programme wasn't recorded. The channel's stream stopped."
        );
        // Stopped early by hand: only the time until the stop counts.
        assert_eq!(
            verdict(&row(0, 3600, 0, Some(600)), &progress(10, 598.0), None).0,
            RecordingStatus::Completed
        );
        // Asked for part-way through: the earlier part wasn't wanted.
        assert_eq!(
            verdict(&row(0, 3600, 1800, None), &progress(10, 1799.0), None).0,
            RecordingStatus::Completed
        );
    }

    #[test]
    fn explains_ffmpeg() {
        // As the recorder hands them over: with their addresses taken out.
        let say = |line: &str| explain(&[redact(line)], Some(1));
        assert!(
            say("<address>: Server returned 403 Forbidden (access denied)").contains("refused")
        );
        assert!(say("<address>: Server returned 404 Not Found").contains("isn't available"));
        assert!(
            say("Connection to tcp://x failed: Connection refused").contains("couldn't be reached")
        );
        assert!(
            say("Connection to tcp://127.0.0.1:1 failed: Error number -10061 occurred")
                .contains("couldn't be reached")
        );
        assert_eq!(explain(&[], Some(0)), "The channel's stream ended.");
    }

    #[test]
    fn record_args_copy_everything_into_mpegts() {
        let target = StreamTarget {
            url: "http://a.example/live/1.ts".into(),
            user_agent: Some("UA".into()),
            headers: vec![],
            is_live: true,
        };
        let input = Input::of(&target).unwrap();
        let args = record_args(&target, &input, Path::new("/tmp/x.ts")).join(" ");
        assert!(
            args.contains(
                "-user_agent UA -reconnect 1 -reconnect_streamed 1 -reconnect_delay_max 10"
            )
        );
        assert!(args.ends_with("-i http://a.example/live/1.ts -map 0 -ignore_unknown -c copy -flush_packets 1 -f mpegts /tmp/x.ts"));
    }

    #[test]
    fn durations_read_well() {
        assert_eq!(duration_text(1.2), "1 second");
        assert_eq!(duration_text(45.0), "45 seconds");
        assert_eq!(duration_text(600.0), "10 minutes");
        assert_eq!(duration_text(7200.0), "2 hours");
    }
}
