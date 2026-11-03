//! Finding the ad breaks in a recording, so the player can mark and skip
//! them. With comskip installed (a separate GPL program, never bundled), its
//! edit decision list is used; otherwise, or when it fails, ffmpeg's
//! `blackdetect` and `silencedetect` read the whole recording once and the
//! rules in rules.rs pick out the breaks. Either way the recording file is
//! only read: comskip writes into a folder of its own, ffmpeg nowhere.

mod rules;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, anyhow, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;

use crate::process::{background, redact};

/// A stretch of a recording that's adverts, in seconds from its start (as
/// the player counts them).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdBreak {
    pub start_seconds: f64,
    pub end_seconds: f64,
}

/// Whether a recording's breaks have been looked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AdBreakState {
    /// Not looked for (yet).
    Never,
    /// Waiting, or being looked for now.
    Finding,
    Found,
    Failed,
}

/// A recording's ad breaks, as far as they're known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdBreaks {
    pub state: AdBreakState,
    pub breaks: Vec<AdBreak>,
    /// "comskip" or "Xorvio", once found.
    pub found_by: Option<String>,
    /// Why they couldn't be found.
    pub error: Option<String>,
}

impl AdBreaks {
    pub fn never() -> AdBreaks {
        AdBreaks { state: AdBreakState::Never, breaks: Vec::new(), found_by: None, error: None }
    }
}

/// Who found a recording's breaks.
pub(crate) const BY_COMSKIP: &str = "comskip";
pub(crate) const BY_XORVIO: &str = "Xorvio";

const STOPPED: &str = "Looking for ad breaks was stopped.";

/// Finds the breaks in `file`, with comskip when there is one. Moving `stop`
/// to true stops whichever is running.
pub(crate) async fn find(
    ffmpeg: &Path,
    comskip: Option<&Path>,
    file: &Path,
    stop: watch::Receiver<bool>,
) -> anyhow::Result<(Vec<AdBreak>, &'static str)> {
    if let Some(comskip) = comskip {
        match with_comskip(comskip, file, stop.clone()).await {
            Ok(breaks) => return Ok((breaks, BY_COMSKIP)),
            Err(error) if *stop.borrow() => return Err(error),
            Err(error) => tracing::info!("comskip couldn't read a recording, so Xorvio looks itself: {error:#}"),
        }
    }
    Ok((with_ffmpeg(ffmpeg, file, stop).await?, BY_XORVIO))
}

/// One pass of ffmpeg over the recording, black and silence noted as it goes.
async fn with_ffmpeg(ffmpeg: &Path, file: &Path, mut stop: watch::Receiver<bool>) -> anyhow::Result<Vec<AdBreak>> {
    let mut child = background(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "info", "-i"])
        .arg(file)
        .args(["-map", "0:V:0", "-map", "0:a:0?", "-vf", rules::BLACK_FILTER, "-af", rules::SILENCE_FILTER])
        .args(["-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("ffmpeg couldn't be started")?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow!("ffmpeg has no output"))?;
    let mut lines = BufReader::new(stderr).lines();
    let mut detected = rules::Detected::default();
    let mut tail: Vec<String> = Vec::new();
    let stopped = loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    detected.read(&line);
                    if tail.len() == 6 {
                        tail.remove(0);
                    }
                    tail.push(line);
                }
                _ => break false,
            },
            _ = stop.wait_for(|stopped| *stopped) => break true,
        }
    };
    if stopped {
        // Until it has gone, so the recording is free to delete: Windows
        // can't delete a file another program has open.
        let _ = child.kill().await;
        bail!(STOPPED);
    }
    let status = child.wait().await?;
    // A recording cut off part-way can end in a way ffmpeg complains of
    // after reading the rest; what it read until then still counts.
    if !status.success() && !detected.read_anything() {
        let why = tail.last().map(|line| redact(line.trim())).unwrap_or_default();
        bail!("ffmpeg couldn't read the recording ({why}).");
    }
    Ok(detected.breaks())
}

/// comskip into a folder of its own (never beside the recording), its EDL read back.
async fn with_comskip(comskip: &Path, file: &Path, mut stop: watch::Receiver<bool>) -> anyhow::Result<Vec<AdBreak>> {
    let work = Work::new()?;
    let ini = work.0.join("comskip.ini");
    std::fs::write(&ini, "output_edl=1\noutput_txt=0\nverbose=0\n").context("comskip's settings couldn't be written")?;
    let mut child = background(comskip)
        .arg(format!("--ini={}", ini.display()))
        .arg(format!("--output={}", work.0.display()))
        .arg(file)
        .current_dir(&work.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("comskip couldn't be started")?;
    let finished = tokio::select! {
        status = child.wait() => Some(status?),
        _ = stop.wait_for(|stopped| *stopped) => None,
    };
    let Some(status) = finished else {
        let _ = child.kill().await;
        bail!(STOPPED);
    };
    // 0: breaks found; 1: none found. Anything else went wrong.
    if !matches!(status.code(), Some(0 | 1)) {
        bail!("comskip stopped with {status}");
    }
    let edl = std::fs::read_dir(&work.0)?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|e| e.eq_ignore_ascii_case("edl")));
    match edl {
        Some(path) => Ok(rules::read_edl(&std::fs::read_to_string(path)?)),
        None if status.code() == Some(1) => Ok(Vec::new()),
        None => bail!("comskip wrote no list of breaks"),
    }
}

/// A temporary folder, removed when done with.
struct Work(PathBuf);

impl Work {
    fn new() -> anyhow::Result<Work> {
        let dir = crate::scratch::temp_root().join(format!("xorvio-comskip-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).context("a folder for comskip couldn't be made")?;
        Ok(Work(dir))
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interface_reads_them_as_types_ts_says() {
        let found = AdBreaks {
            state: AdBreakState::Found,
            breaks: vec![AdBreak { start_seconds: 600.0, end_seconds: 780.5 }],
            found_by: Some(BY_XORVIO.into()),
            error: None,
        };
        assert_eq!(
            serde_json::to_value(found).unwrap(),
            serde_json::json!({ "state": "found", "breaks": [{ "startSeconds": 600.0, "endSeconds": 780.5 }], "foundBy": "Xorvio", "error": null })
        );
        assert_eq!(serde_json::to_value(AdBreaks::never()).unwrap()["state"], "never");
    }
}
