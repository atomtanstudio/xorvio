//! Recording files: their names, their parts, and joining the parts.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use chrono::{Local, TimeZone};
use xorvio_model::Recording;

use crate::process::tool_command;

/// "<Channel> - <Title> - <YYYY-MM-DD HHMM>", in local time, with anything a
/// file system could object to taken out.
pub(crate) fn base_name(recording: &Recording) -> String {
    let when = Local
        .timestamp_opt(recording.start, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H%M").to_string())
        .unwrap_or_default();
    let name = format!(
        "{} - {} - {}",
        recording.channel_name, recording.title, when
    );
    safe_name(&name)
}

pub(crate) fn safe_name(text: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|c| {
            if c.is_control() || r#"/\:*?"<>|"#.contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut name = collapsed
        .trim_matches(|c: char| c == '.' || c == ' ' || c == '-')
        .to_string();
    if name.chars().count() > 150 {
        name = name
            .chars()
            .take(150)
            .collect::<String>()
            .trim_end()
            .to_string();
    }
    if name.is_empty() {
        "Recording".to_string()
    } else {
        name
    }
}

/// The file for the next part: the plain name for the first, "(part N)"
/// after it for later ones, and " (2)", " (3)"… when a file already has
/// the name.
pub(crate) fn next_part(dir: &Path, base: &str, part_number: usize) -> PathBuf {
    let stem = if part_number <= 1 {
        base.to_string()
    } else {
        format!("{base} (part {part_number})")
    };
    let mut candidate = dir.join(format!("{stem}.ts"));
    let mut copy = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({copy}).ts"));
        copy += 1;
    }
    candidate
}

pub(crate) fn size_of(path: &str) -> i64 {
    std::fs::metadata(path).map_or(0, |m| m.len() as i64)
}

/// Recordings are always MPEG-TS files. A recording's list of files could
/// hold any path (a backup made elsewhere, say), so only those are ever
/// deleted or moved to the Trash.
fn is_recording_file(path: &str) -> bool {
    let keep = Path::new(path).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("ts"));
    if !keep {
        tracing::warn!("a recording listed a file that isn't a recording, so it was left alone");
    }
    keep
}

pub(crate) fn remove(paths: &[String]) {
    for path in paths.iter().filter(|path| is_recording_file(path)) {
        if let Err(error) = std::fs::remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("a recording file couldn't be deleted: {error}");
        }
    }
}

/// Moves files to the Trash (the Recycle Bin on Windows), for recordings a
/// series clears out or that go with a deleted series. Where there's no
/// Trash to move them to (some network drives), they're deleted, as
/// deleting a recording by hand does.
pub(crate) fn trash(paths: &[String]) {
    for path in paths.iter().filter(|p| is_recording_file(p) && Path::new(p).exists()) {
        if let Err(error) = move_to_trash(Path::new(path)) {
            tracing::warn!("a recording couldn't go to the Trash, so it was deleted: {error}");
            remove(std::slice::from_ref(path));
        }
    }
}

#[cfg(not(test))]
fn move_to_trash(path: &Path) -> Result<(), trash::Error> {
    #[allow(unused_mut)]
    let mut context = trash::TrashContext::default();
    // Asking the Finder would need permission to control it, and plays its
    // sound; the file manager does neither.
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        context.set_delete_method(DeleteMethod::NsFileManager);
    }
    context.delete(path)
}

/// Tests don't fill the Trash of whoever runs them.
#[cfg(test)]
fn move_to_trash(path: &Path) -> Result<(), std::io::Error> {
    std::fs::remove_file(path)
}

/// Joins the parts of a recording that reconnected into its first file, so
/// the recording plays as one. On failure the parts stay as they are.
pub(crate) async fn join(ffmpeg: &Path, parts: &[String]) -> Option<String> {
    let first = PathBuf::from(parts.first()?);
    let dir = first.parent()?;
    let list = dir.join(format!(".{}.parts.txt", uuid::Uuid::new_v4().simple()));
    let joined = dir.join(format!(".{}.joined.ts", uuid::Uuid::new_v4().simple()));
    let entries: String = parts
        .iter()
        .map(|p| format!("file '{}'\n", p.replace('\'', r"'\''")))
        .collect();
    std::fs::write(&list, entries).ok()?;
    let status = tool_command(ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
        ])
        .arg(&list)
        .args(["-map", "0", "-c", "copy", "-f", "mpegts"])
        .arg(&joined)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await;
    let _ = std::fs::remove_file(&list);
    let joined_ok =
        matches!(status, Ok(s) if s.success()) && size_of(&joined.to_string_lossy()) > 0;
    if !joined_ok {
        let _ = std::fs::remove_file(&joined);
        return None;
    }
    remove(parts);
    std::fs::rename(&joined, &first).ok()?;
    Some(first.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe() {
        assert_eq!(
            safe_name("News: 24/7 \"Live\" - Late?"),
            "News 24 7 Live - Late"
        );
        assert_eq!(safe_name("..."), "Recording");
        assert_eq!(safe_name(&"x".repeat(300)).len(), 150);
    }

    #[test]
    fn parts_get_free_names() {
        let dir = tempfile::tempdir().unwrap();
        let first = next_part(dir.path(), "Show", 1);
        assert!(first.ends_with("Show.ts"));
        std::fs::write(&first, b"x").unwrap();
        assert!(next_part(dir.path(), "Show", 1).ends_with("Show (2).ts"));
        assert!(next_part(dir.path(), "Show", 2).ends_with("Show (part 2).ts"));
    }

    #[test]
    fn deletes_only_recording_files() {
        let dir = tempfile::tempdir().unwrap();
        let recording = dir.path().join("Show.ts");
        let other = dir.path().join("Holiday.mov");
        std::fs::write(&recording, b"x").unwrap();
        std::fs::write(&other, b"x").unwrap();
        let paths = [&recording, &other].map(|p| p.to_string_lossy().into_owned());
        remove(&paths);
        assert!(!recording.exists());
        assert!(other.exists());
        std::fs::write(&recording, b"x").unwrap();
        trash(&paths);
        assert!(!recording.exists());
        assert!(other.exists());
    }
}
