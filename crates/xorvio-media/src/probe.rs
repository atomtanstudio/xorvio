//! Asking ffprobe what's in a stream, to choose between playing it directly
//! and remuxing it, and to know its length.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, bail};
use serde_json::Value;
use xorvio_model::{StreamInfo, StreamTarget, language_code};

use crate::gateway::PlaybackSupport;
use crate::process::{Input, header_args, tool_command};

/// How long ffprobe gets before the stream is treated as unknown. Large
/// files from a debrid service can take ten seconds or more to start.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Probe {
    /// ffprobe's container names, such as "mov,mp4,m4a,3gp,3g2,mj2".
    pub format_name: String,
    pub duration: Option<f64>,
    pub video: Option<VideoStream>,
    /// The audio track to play: the default one, unless another is chosen.
    pub audio: Option<AudioStream>,
    /// Every audio track, in file order.
    pub audio_tracks: Vec<AudioStream>,
    pub subtitles: Vec<SubtitleStream>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct VideoStream {
    pub index: i64,
    pub codec: String,
    /// "hvc1", "hev1", "avc1"…
    pub tag: String,
    /// "Main 10", "High"…
    pub profile: String,
    /// "yuv420p10le" for 10-bit video.
    pub pix_fmt: String,
    pub interlaced: bool,
    pub width: Option<i64>,
    pub height: Option<i64>,
    /// Width over height as shown, which differs from the pixels' for
    /// anamorphic video (16:9 broadcasts in 720×576).
    pub aspect: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct AudioStream {
    pub index: i64,
    pub codec: String,
    pub channels: Option<i64>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct SubtitleStream {
    pub index: i64,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub default: bool,
    pub forced: bool,
    /// Picture subtitles' canvas, usually the video's size.
    pub width: Option<i64>,
    pub height: Option<i64>,
}

impl SubtitleStream {
    /// Text subtitles, which ffmpeg turns into WebVTT. Picture subtitles
    /// (PGS, VobSub, DVB) can only be drawn into the video.
    pub fn is_text(&self) -> bool {
        matches!(self.codec.as_str(), "subrip" | "srt" | "ass" | "ssa" | "webvtt" | "mov_text" | "text")
    }
}

/// Video codecs the web view decodes.
pub(crate) fn is_web_video(codec: &str) -> bool {
    matches!(codec, "h264" | "hevc" | "vp9" | "av1")
}

/// Audio codecs the web view decodes.
pub(crate) fn is_web_audio(codec: &str) -> bool {
    matches!(codec, "aac" | "mp3" | "opus")
}

impl Probe {
    /// The `<video>` element can play it as it is: an MP4/MOV or WebM file
    /// whose video and audio the web view decodes. HEVC also has to be tagged
    /// `hvc1`, which is the only form WebKit plays, and VP9 and AV1 count
    /// only where the interface reports the web view plays them. ffprobe
    /// names MKV and WebM alike, so only WebM's own codecs count as WebM.
    pub fn plays_natively(&self, support: PlaybackSupport) -> bool {
        let mp4 = self.format_name.contains("mp4") || self.format_name.contains("mov");
        let webm = self.format_name.contains("webm")
            && self
                .video
                .as_ref()
                .is_none_or(|v| matches!(v.codec.as_str(), "vp9" | "av1"))
            && self.audio.as_ref().is_none_or(|a| a.codec == "opus");
        let container = mp4 || webm;
        let video_ok = self.video.as_ref().is_none_or(|v| match v.codec.as_str() {
            "hevc" => v.tag == "hvc1",
            "vp9" => support.vp9,
            "av1" => support.av1,
            codec => is_web_video(codec),
        });
        let audio_ok = self.audio.as_ref().is_none_or(|a| is_web_audio(&a.codec));
        container && (self.video.is_some() || self.audio.is_some()) && video_ok && audio_ok
    }

    /// What the stream holds, for the log: no addresses.
    pub fn describe(&self) -> String {
        let video = self.video.as_ref().map_or("none".to_string(), |v| {
            format!("{} {} [{}] {}", v.codec, v.profile, v.tag, v.pix_fmt)
        });
        let audio = self.audio.as_ref().map_or("none".to_string(), |a| {
            format!("{} {}ch", a.codec, a.channels.unwrap_or(0))
        });
        format!(
            "container={} duration={:?} video={video} audio={audio}",
            self.format_name, self.duration
        )
    }

    pub fn is_hls(&self) -> bool {
        self.format_name
            .split(',')
            .any(|name| name == "hls" || name == "applehttp")
    }

    pub fn from_json(value: &Value) -> Probe {
        let format = &value["format"];
        let streams = value["streams"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let text = |v: &Value, key: &str| v[key].as_str().unwrap_or_default().to_string();
        let is_default = |s: &Value| s["disposition"]["default"].as_i64() == Some(1);
        let tag = |s: &Value, key: &str| s["tags"][key].as_str().map(str::trim).filter(|t| !t.is_empty()).map(String::from);
        // Cover art shows up as a video stream; it isn't the film.
        let video = streams
            .iter()
            .filter(|s| {
                s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_i64() != Some(1)
            })
            .min_by_key(|s| !is_default(s))
            .map(|s| VideoStream {
                index: s["index"].as_i64().unwrap_or(0),
                codec: text(s, "codec_name"),
                tag: text(s, "codec_tag_string"),
                profile: text(s, "profile"),
                pix_fmt: text(s, "pix_fmt"),
                interlaced: matches!(s["field_order"].as_str(), Some("tt" | "bb" | "tb" | "bt")),
                width: s["width"].as_i64(),
                height: s["height"].as_i64(),
                aspect: display_aspect(s),
            });
        let audio_tracks: Vec<AudioStream> = streams
            .iter()
            .filter(|s| s["codec_type"] == "audio")
            .map(|s| AudioStream {
                index: s["index"].as_i64().unwrap_or(0),
                codec: text(s, "codec_name"),
                channels: s["channels"].as_i64(),
                language: tag(s, "language").and_then(|l| language_code(&l)),
                title: tag(s, "title"),
                default: is_default(s),
            })
            .collect();
        let audio = audio_tracks.iter().min_by_key(|a| !a.default).cloned();
        let subtitles = streams
            .iter()
            .filter(|s| s["codec_type"] == "subtitle")
            .map(|s| SubtitleStream {
                index: s["index"].as_i64().unwrap_or(0),
                codec: text(s, "codec_name"),
                language: tag(s, "language").and_then(|l| language_code(&l)),
                title: tag(s, "title"),
                default: is_default(s),
                forced: s["disposition"]["forced"].as_i64() == Some(1),
                width: s["width"].as_i64().filter(|width| *width > 0),
                height: s["height"].as_i64().filter(|height| *height > 0),
            })
            .collect();
        let duration = format["duration"]
            .as_str()
            .and_then(|d| d.parse::<f64>().ok())
            .or_else(|| format["duration"].as_f64())
            .filter(|d| d.is_finite() && *d > 0.0);
        Probe {
            format_name: text(format, "format_name"),
            duration,
            video,
            audio,
            audio_tracks,
            subtitles,
        }
    }

    /// The same stream playing another of its audio tracks.
    pub fn with_audio(&self, index: i64) -> Probe {
        let audio = self.audio_tracks.iter().find(|a| a.index == index).cloned().or_else(|| self.audio.clone());
        Probe { audio, ..self.clone() }
    }

    /// For the player's stream information.
    pub fn info(&self) -> StreamInfo {
        let container = self.format_name.split(',').next().unwrap_or_default();
        let container = match container {
            "matroska" => "Matroska",
            "mov" => "MP4",
            "mpegts" => "MPEG-TS",
            "hls" | "applehttp" => "HLS",
            "avi" => "AVI",
            "flv" => "FLV",
            other => other,
        };
        let video = self.video.as_ref().map(|v| {
            let mut parts = vec![codec_name(&v.codec)];
            if !v.profile.is_empty() {
                parts[0] = format!("{} {}", parts[0], v.profile);
            }
            if v.pix_fmt.contains("10") {
                parts.push("10-bit".into());
            }
            if v.interlaced {
                parts.push("interlaced".into());
            }
            parts.join(" · ")
        });
        let audio = self.audio.as_ref().map(|a| match a.channels {
            Some(c) => format!("{} · {}", codec_name(&a.codec), channel_layout(c)),
            None => codec_name(&a.codec),
        });
        StreamInfo {
            container: container.to_string(),
            video,
            audio,
            notes: Vec::new(),
        }
    }
}

/// A video stream's shape as shown: its display aspect ratio ("16:9"), or
/// its pixels' shape times their own (`sample_aspect_ratio`), when sensible.
fn display_aspect(stream: &Value) -> Option<f64> {
    let ratio = |key: &str| {
        let (a, b) = stream[key].as_str()?.split_once(':')?;
        let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
        (a > 0.0 && b > 0.0).then_some(a / b)
    };
    let pixels = match (stream["width"].as_f64(), stream["height"].as_f64()) {
        (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Some(w / h),
        _ => None,
    };
    ratio("display_aspect_ratio")
        .or_else(|| Some(pixels? * ratio("sample_aspect_ratio")?))
        .or(pixels)
        .filter(|aspect| (0.2..=5.0).contains(aspect))
}

/// How codecs are usually written.
pub(crate) fn codec_name(codec: &str) -> String {
    match codec {
        "h264" => "H.264",
        "hevc" => "HEVC",
        "av1" => "AV1",
        "vp9" => "VP9",
        "mpeg2video" => "MPEG-2",
        "aac" => "AAC",
        "ac3" => "Dolby Digital",
        "eac3" => "Dolby Digital Plus",
        "truehd" => "Dolby TrueHD",
        "dts" => "DTS",
        "mp3" => "MP3",
        "opus" => "Opus",
        "flac" => "FLAC",
        other => return other.to_uppercase(),
    }
    .to_string()
}

pub(crate) fn channel_layout(channels: i64) -> String {
    match channels {
        1 => "mono".into(),
        2 => "stereo".into(),
        6 => "5.1".into(),
        8 => "7.1".into(),
        n => format!("{n} channels"),
    }
}

/// Runs ffprobe on the target with its headers, within [`PROBE_TIMEOUT`].
pub(crate) async fn probe(ffprobe: &Path, target: &StreamTarget) -> anyhow::Result<Probe> {
    let Some(input) = Input::of(target) else {
        bail!("not an address ffprobe can open")
    };
    let mut command = tool_command(ffprobe);
    command.args([
        "-v",
        "error",
        "-hide_banner",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ]);
    if input.is_web() {
        command.args(header_args(target));
        command.args(["-rw_timeout", "8000000"]);
    }
    command
        .arg("-i")
        .arg(input.ffmpeg_input())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(PROBE_TIMEOUT, command.output())
        .await
        .context("ffprobe took too long")?
        .context("ffprobe couldn't be started")?;
    if !output.status.success() {
        bail!("ffprobe couldn't read the stream");
    }
    let value: Value =
        serde_json::from_slice(&output.stdout).context("ffprobe's answer couldn't be read")?;
    Ok(Probe::from_json(&value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_streams_and_skips_cover_art() {
        let probe = Probe::from_json(&json!({
            "format": { "format_name": "matroska,webm", "duration": "62.5" },
            "streams": [
                { "index": 0, "codec_type": "video", "codec_name": "mjpeg", "disposition": { "attached_pic": 1 } },
                { "index": 1, "codec_type": "video", "codec_name": "h264", "field_order": "progressive" },
                { "index": 2, "codec_type": "audio", "codec_name": "ac3", "channels": 6 },
                { "index": 3, "codec_type": "audio", "codec_name": "aac", "channels": 2, "disposition": { "default": 1 } }
            ]
        }));
        assert_eq!(probe.duration, Some(62.5));
        assert_eq!(probe.video.as_ref().unwrap().index, 1);
        assert_eq!(probe.audio.as_ref().unwrap().codec, "aac");
        assert!(
            !probe.plays_natively(PlaybackSupport::default()),
            "H.264 in Matroska is MKV, not WebM"
        );
        let webm = Probe {
            video: Some(VideoStream {
                codec: "vp9".into(),
                ..VideoStream::default()
            }),
            audio: Some(AudioStream {
                codec: "opus".into(),
                ..AudioStream::default()
            }),
            ..probe
        };
        let vp9 = PlaybackSupport { vp9: true, ..PlaybackSupport::default() };
        assert!(webm.plays_natively(vp9));
        assert!(!webm.plays_natively(PlaybackSupport::default()), "only where the web view plays VP9");
    }

    #[test]
    fn reads_the_pictures_shape_as_shown() {
        let aspect = |stream: Value| display_aspect(&stream);
        // A 16:9 broadcast in 720×576, said both ways.
        assert_eq!(aspect(json!({ "width": 720, "height": 576, "display_aspect_ratio": "16:9" })), Some(16.0 / 9.0));
        let anamorphic = aspect(json!({ "width": 720, "height": 576, "sample_aspect_ratio": "64:45", "display_aspect_ratio": "0:1" })).unwrap();
        assert!((anamorphic - 16.0 / 9.0).abs() < 1e-9);
        assert_eq!(aspect(json!({ "width": 1920, "height": 1080, "display_aspect_ratio": "N/A" })), Some(1920.0 / 1080.0));
        assert_eq!(aspect(json!({ "codec_name": "h264" })), None);
    }

    #[test]
    fn native_needs_web_codecs() {
        let mp4 = |video: &str, tag: &str, audio: &str| Probe {
            format_name: "mov,mp4,m4a,3gp,3g2,mj2".into(),
            duration: None,
            video: Some(VideoStream {
                index: 0,
                codec: video.into(),
                tag: tag.into(),
                interlaced: false,
                ..VideoStream::default()
            }),
            audio: Some(AudioStream {
                index: 1,
                codec: audio.into(),
                channels: Some(2),
                ..AudioStream::default()
            }),
            ..Probe::default()
        };
        let support = PlaybackSupport::default();
        assert!(mp4("h264", "avc1", "aac").plays_natively(support));
        assert!(mp4("hevc", "hvc1", "mp3").plays_natively(support));
        assert!(!mp4("hevc", "hev1", "aac").plays_natively(support));
        assert!(!mp4("h264", "avc1", "ac3").plays_natively(support));
        assert!(!mp4("mpeg2video", "", "aac").plays_natively(support));
        assert!(
            !Probe {
                format_name: "avi".into(),
                ..mp4("h264", "", "aac")
            }
            .plays_natively(support)
        );
        // AV1 plays as it is only where the hardware decodes it.
        assert!(!mp4("av1", "av01", "aac").plays_natively(support));
        assert!(mp4("av1", "av01", "aac").plays_natively(PlaybackSupport { av1: true, ..support }));
    }
}
