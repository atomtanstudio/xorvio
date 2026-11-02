//! A frame from a video file, for the card of a video with no artwork of its
//! own (a media folder's home videos, clips or music videos). It's taken
//! about 15% of the way in, past any black or titles at the start, at 360
//! lines, and kept as a JPEG named for the file's path, size and change
//! time, so a changed file gets a new one and an unchanged one is never made
//! twice. A few are made at once, with ffmpeg at low priority; the video file
//! is only read.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::process::background;
use crate::tools::Tools;

/// The scheme the app serves stills under (see [`still_url`]).
pub const STILL_SCHEME: &str = "xorvio-still";

/// Stills made at once: local files, so a few, but never enough to slow playback.
const AT_ONCE: usize = 3;
/// A file that takes longer than this won't give a frame.
const LONGEST: Duration = Duration::from_secs(20);
/// How far in the frame is taken, and never further than this.
const PART_WAY: f64 = 0.15;
const FURTHEST_SECONDS: f64 = 90.0;

pub struct Stills {
    dir: PathBuf,
    tools: Option<Tools>,
    turns: Semaphore,
}

impl Stills {
    pub fn new(dir: PathBuf, tools: Option<Tools>) -> Stills {
        Stills { dir, tools, turns: Semaphore::new(AT_ONCE) }
    }

    /// The still of `file` ("<name>.jpg"), made now if there's none yet.
    /// `None` when there's no ffmpeg, or the file gives no frame.
    pub async fn still(&self, file: &Path) -> Option<String> {
        let name = still_name(file)?;
        let path = self.dir.join(&name);
        if path.is_file() {
            return Some(name);
        }
        let tools = self.tools.as_ref()?;
        let _turn = self.turns.acquire().await.ok()?;
        // Made meanwhile, for another list showing the same video.
        if path.is_file() {
            return Some(name);
        }
        std::fs::create_dir_all(&self.dir).ok()?;
        let at = match duration(&tools.ffprobe, file).await {
            Some(seconds) if seconds > 2.0 => (seconds * PART_WAY).min(FURTHEST_SECONDS),
            _ => 0.0,
        };
        // Written beside it and then moved into place, so a half-written one is never served.
        let partial = self.dir.join(format!("{name}.part"));
        let made = frame(&tools.ffmpeg, file, at, &partial).await;
        if made && std::fs::rename(&partial, &path).is_ok() {
            return Some(name);
        }
        let _ = std::fs::remove_file(&partial);
        if !made {
            tracing::debug!("no frame could be taken from a video for its card");
        }
        None
    }

    /// The file of a still named by [`Stills::still`]; `None` for any other name.
    pub fn path(&self, name: &str) -> Option<PathBuf> {
        let (stem, extension) = name.split_once('.')?;
        let ours = extension == "jpg" && stem.len() == 32 && stem.chars().all(|c| c.is_ascii_hexdigit());
        ours.then(|| self.dir.join(name)).filter(|path| path.is_file())
    }
}

/// The address the interface loads a still from.
pub fn still_url(name: &str) -> String {
    if cfg!(windows) { format!("http://{STILL_SCHEME}.localhost/{name}") } else { format!("{STILL_SCHEME}://localhost/{name}") }
}

/// A name for the still of `file` as it is now: its path, size and change time, hashed.
fn still_name(file: &Path) -> Option<String> {
    let meta = std::fs::metadata(file).ok().filter(|meta| meta.is_file())?;
    let changed = meta.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok()).map(|at| at.as_secs()).unwrap_or(0);
    let mut hash = Sha256::new();
    hash.update(file.to_string_lossy().as_bytes());
    hash.update(meta.len().to_le_bytes());
    hash.update(changed.to_le_bytes());
    let digest = hash.finalize();
    Some(format!("{}.jpg", digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect::<String>()))
}

/// The video's length in seconds, from its header.
async fn duration(ffprobe: &Path, file: &Path) -> Option<f64> {
    let mut command = background(ffprobe);
    command
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(LONGEST, command.output()).await.ok()?.ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse::<f64>().ok().filter(|seconds| seconds.is_finite())
}

/// One frame at `at` seconds, 360 lines high, as a JPEG at `out`.
async fn frame(ffmpeg: &Path, file: &Path, at: f64, out: &Path) -> bool {
    let mut command = background(ffmpeg);
    command
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-ss", &format!("{at:.2}"), "-i"])
        .arg(file)
        .args(["-map", "0:v:0", "-an", "-sn", "-dn", "-frames:v", "1", "-vf", "scale=-2:360", "-c:v", "mjpeg", "-q:v", "5", "-f", "image2", "-y"])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(LONGEST, command.status()).await {
        Ok(Ok(status)) => status.success() && std::fs::metadata(out).is_ok_and(|meta| meta.len() > 0),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_file_and_only_ours_are_served() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("clip.mp4");
        std::fs::write(&video, b"not really a video").unwrap();
        let name = still_name(&video).unwrap();
        assert_eq!(still_name(&video).unwrap(), name, "the same file, the same name");
        std::fs::write(&video, b"a longer file than before").unwrap();
        assert_ne!(still_name(&video).unwrap(), name, "a changed file, a new still");
        assert_eq!(still_name(&dir.path().join("missing.mp4")), None);

        let stills = Stills::new(dir.path().to_path_buf(), None);
        std::fs::write(dir.path().join(&name), b"jpeg").unwrap();
        assert!(stills.path(&name).is_some());
        assert_eq!(stills.path("../clip.mp4"), None);
        assert_eq!(stills.path("clip.mp4"), None);
        assert!(still_url(&name).ends_with(&format!("/{name}")));
    }
}
