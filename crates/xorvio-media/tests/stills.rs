//! A frame from a video file for its card: made once, kept, and made again
//! only when the file changes. Skips itself without ffmpeg.

mod common;

use common::{make_ts, tools};
use xorvio_media::Stills;

#[tokio::test]
async fn a_video_gives_a_frame_once_and_a_file_that_isnt_one_none() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("Beach Trip - video 00001.ts");
    make_ts(&tools, &video, 12);
    let stills = Stills::new(dir.path().join("Stills"), Some(tools.clone()));

    let name = stills.still(&video).await.expect("a frame");
    let path = stills.path(&name).expect("kept");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "a JPEG");
    let made_at = std::fs::metadata(&path).unwrap().modified().unwrap();

    // Asked again, it's the one already made.
    assert_eq!(stills.still(&video).await.as_deref(), Some(name.as_str()));
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), made_at);

    // A file that isn't a video gives none, and leaves nothing half-made.
    let text = dir.path().join("notes.mp4");
    std::fs::write(&text, b"these are notes, not a video").unwrap();
    assert_eq!(stills.still(&text).await, None);
    let left: Vec<_> = std::fs::read_dir(dir.path().join("Stills")).unwrap().flatten().map(|entry| entry.file_name()).collect();
    assert_eq!(left.len(), 1, "only the real still: {left:?}");

    // Without ffmpeg, none is made.
    let without = Stills::new(dir.path().join("Other"), None);
    assert_eq!(without.still(&video).await, None);
}

#[tokio::test]
async fn a_files_facts_are_read_from_it() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("clip.ts");
    make_ts(&tools, &video, 6);
    let facts = xorvio_media::file_facts(&tools.ffprobe, &video).await.expect("facts");
    assert!(facts.duration_seconds.is_some_and(|seconds| (5.0..7.5).contains(&seconds)), "{facts:?}");
    assert!(facts.width.is_some() && facts.height.is_some() && facts.size_bytes.is_some());
    assert!(facts.audio, "the test video has a tone");
    let text = dir.path().join("notes.mp4");
    std::fs::write(&text, b"not a video").unwrap();
    assert_eq!(xorvio_media::file_facts(&tools.ffprobe, &text).await, None);
}
