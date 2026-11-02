//! What a video file holds, for a media folder's library page: how long it
//! runs, its picture's size, whether it has sound, and its size on disk.
//! Read from the file's header with ffprobe at low priority; the file is
//! only read.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use xorvio_model::FileFacts;

use crate::process::background;

/// A file that takes longer than this to read has nothing to say.
const LONGEST: Duration = Duration::from_secs(20);

/// What `file` holds; `None` when ffprobe couldn't read it at all.
pub async fn file_facts(ffprobe: &Path, file: &Path) -> Option<FileFacts> {
    let mut command = background(ffprobe);
    command
        .args(["-v", "error", "-show_entries", "format=duration,size:stream=codec_type,width,height", "-of", "json"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(LONGEST, command.output()).await.ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    read(&serde_json::from_slice(&output.stdout).ok()?)
}

fn read(reply: &Value) -> Option<FileFacts> {
    let streams = reply.get("streams")?.as_array()?;
    let video = streams.iter().find(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("video"));
    let number = |value: Option<&Value>| match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    let format = reply.get("format");
    Some(FileFacts {
        duration_seconds: number(format.and_then(|format| format.get("duration"))).filter(|seconds| seconds.is_finite() && *seconds > 0.0),
        width: number(video.and_then(|video| video.get("width"))).map(|width| width as i64).filter(|width| *width > 0),
        height: number(video.and_then(|video| video.get("height"))).map(|height| height as i64).filter(|height| *height > 0),
        audio: streams.iter().any(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("audio")),
        size_bytes: number(format.and_then(|format| format.get("size"))).map(|bytes| bytes as i64).filter(|bytes| *bytes > 0),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use xorvio_model::ClipKind;

    use super::*;

    #[test]
    fn ffprobes_reply_is_read_and_told_apart() {
        // A GIF made into an MP4: silent, short and small.
        let gif = read(&json!({"streams": [{"codec_type": "video", "width": 498, "height": 280}], "format": {"duration": "2.4", "size": "103424"}})).unwrap();
        assert_eq!((gif.duration_seconds, gif.width, gif.audio, gif.size_bytes), (Some(2.4), Some(498), false, Some(103_424)));
        assert_eq!(gif.kind(), Some(ClipKind::Gif));
        // An AI clip: silent and short, but large and sharp.
        let animation = read(&json!({"streams": [{"codec_type": "video", "width": 480, "height": 720}], "format": {"duration": "4.1", "size": "1046528"}})).unwrap();
        assert_eq!(animation.kind(), Some(ClipKind::Animation));
        // Sound, or a long run, makes a video.
        let video = read(&json!({"streams": [{"codec_type": "video", "width": 710, "height": 480}, {"codec_type": "audio"}], "format": {"duration": "6.1", "size": "1806336"}})).unwrap();
        assert_eq!(video.kind(), Some(ClipKind::Video));
        let long = read(&json!({"streams": [{"codec_type": "video", "width": 320, "height": 240}], "format": {"duration": "45", "size": "200000"}})).unwrap();
        assert_eq!(long.kind(), Some(ClipKind::Video));
        // Without a length, it's not known yet what it is.
        assert_eq!(read(&json!({"streams": [], "format": {}})).unwrap().kind(), None);
        assert_eq!(read(&json!({})), None);
    }
}
