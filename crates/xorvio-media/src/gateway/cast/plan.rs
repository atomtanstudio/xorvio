//! What a TV gets for each kind of stream. Google's Default Media Receiver
//! plays HLS and progressive MP4 and WebM, decoding H.264, VP8/VP9 (in WebM)
//! and AAC, MP3, Opus and Vorbis on every Chromecast. HEVC, AC-3 and the
//! rest depend on the device and the TV behind it, which a sender can't ask
//! about, so they're converted.
//!
//! - Films and episodes that are already MP4 or WebM with those codecs go
//!   as they are, ranges and all, so the TV seeks by itself.
//! - Everything else on demand (MKV, AVI, MPEG-TS, HEVC, Dolby or DTS
//!   audio, HLS) is remuxed by ffmpeg into fragmented MP4, streamed as it's
//!   made: H.264 copied, other video encoded to H.264 (1080p at most), AAC
//!   copied, other audio made AAC. It can't be seeked into, so seeking
//!   starts it again from there, as the player's own remux does.
//! - Live channels go through a live buffer (HLS of fragmented MP4 from
//!   ffmpeg, as for pausing live TV), which survives the TV reconnecting
//!   and lets it pause for a while.

use xorvio_model::StreamTarget;

use crate::process::{Input, header_args};
use crate::probe::Probe;

/// Taller than this is made 1080p when encoding, which keeps up in software.
const MOST_ENCODED_LINES: i64 = 1080;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Route {
    /// The file as it is, with its own content type.
    Direct { content_type: &'static str },
    /// ffmpeg's fragmented MP4.
    Mp4(CastPlan),
    /// A live buffer's HLS.
    Live,
}

impl Route {
    /// The letter in the address, which the listener checks against the entry.
    pub fn kind(&self) -> &'static str {
        match self {
            Route::Direct { .. } => "s",
            Route::Mp4(_) => "m",
            Route::Live => "l",
        }
    }
}

pub(crate) const NEEDS_FFMPEG: &str = "Xorvio needs ffmpeg to send this to a TV.";

/// How a film, an episode or a recording goes to the TV. `ext` is the
/// container's extension, when known.
pub(crate) fn route(probe: Option<&Probe>, ext: Option<&str>, has_ffmpeg: bool) -> Result<Route, &'static str> {
    let hls = matches!(ext, Some("m3u8" | "m3u")) || probe.is_some_and(Probe::is_hls);
    if !hls {
        if let Some(probe) = probe {
            if let Some(content_type) = plays_directly(probe) {
                return Ok(Route::Direct { content_type });
            }
        } else {
            match ext {
                Some("mp4" | "m4v" | "mov") => return Ok(Route::Direct { content_type: "video/mp4" }),
                Some("webm") => return Ok(Route::Direct { content_type: "video/webm" }),
                _ => {}
            }
        }
    }
    if !has_ffmpeg {
        return Err(NEEDS_FFMPEG);
    }
    Ok(Route::Mp4(CastPlan::new(probe, hls)))
}

/// The content type a receiver plays this file with as it is, if it can.
fn plays_directly(probe: &Probe) -> Option<&'static str> {
    let mp4 = probe.format_name.contains("mp4") || probe.format_name.contains("mov");
    // ffprobe names MKV and WebM alike, so only WebM's own codecs count.
    let webm = probe.format_name.contains("webm")
        && probe.video.as_ref().is_none_or(|v| matches!(v.codec.as_str(), "vp8" | "vp9"))
        && probe.audio.as_ref().is_none_or(|a| matches!(a.codec.as_str(), "opus" | "vorbis"));
    if probe.video.is_none() && probe.audio.is_none() {
        return None;
    }
    let eight_bit = |pix_fmt: &str| matches!(pix_fmt, "" | "yuv420p" | "yuvj420p");
    if webm {
        let video_ok = probe.video.as_ref().is_none_or(|v| v.codec == "vp8" || (v.codec == "vp9" && eight_bit(&v.pix_fmt)));
        return video_ok.then_some("video/webm");
    }
    if mp4 {
        let video_ok = probe.video.as_ref().is_none_or(|v| v.codec == "h264" && eight_bit(&v.pix_fmt));
        let audio_ok = probe.audio.as_ref().is_none_or(|a| matches!(a.codec.as_str(), "aac" | "mp3"));
        return (video_ok && audio_ok).then_some("video/mp4");
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
enum Video {
    Copy,
    Encode { deinterlace: bool, downscale: bool },
}

#[derive(Debug, Clone, PartialEq)]
enum Audio {
    Copy,
    /// To AAC in a standard layout, as the player's remux does.
    Aac { surround: bool },
}

/// ffmpeg's work for a TV: what's copied and what's converted.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CastPlan {
    hls_input: bool,
    video_map: String,
    video: Video,
    audio_map: Option<String>,
    audio: Audio,
}

impl CastPlan {
    /// The plan for a probed stream, or a cautious one (video copied, audio
    /// made AAC) when it couldn't be probed.
    pub fn new(probe: Option<&Probe>, hls_input: bool) -> CastPlan {
        let Some(probe) = probe else {
            return CastPlan {
                hls_input,
                video_map: "0:V:0?".into(),
                video: Video::Copy,
                audio_map: Some("0:a:0?".into()),
                audio: Audio::Aac { surround: true },
            };
        };
        let (video_map, video) = match &probe.video {
            Some(v) if v.codec == "h264" && matches!(v.pix_fmt.as_str(), "" | "yuv420p" | "yuvj420p") => (format!("0:{}", v.index), Video::Copy),
            Some(v) => (
                format!("0:{}", v.index),
                Video::Encode {
                    deinterlace: v.interlaced,
                    downscale: v.height.is_some_and(|height| height > MOST_ENCODED_LINES),
                },
            ),
            None => ("0:V:0?".into(), Video::Copy),
        };
        let (audio_map, audio) = match &probe.audio {
            Some(a) if a.codec == "aac" => (Some(format!("0:{}", a.index)), Audio::Copy),
            Some(a) => (Some(format!("0:{}", a.index)), Audio::Aac { surround: a.channels.is_some_and(|c| c > 2) }),
            None => (None, Audio::Copy),
        };
        CastPlan { hls_input, video_map, video, audio_map, audio }
    }

    /// What was done, for the log and the interface.
    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if let Video::Encode { downscale, .. } = self.video {
            notes.push(if downscale { "Video encoded to H.264 at 1080p" } else { "Video encoded to H.264" }.to_string());
        }
        if let Audio::Aac { surround } = self.audio {
            notes.push(format!("Audio converted to AAC {}", if surround { "5.1" } else { "stereo" }));
        }
        notes
    }

    /// ffmpeg's arguments, writing fragmented MP4 to stdout from `offset`
    /// seconds in.
    pub fn args(&self, target: &StreamTarget, input: &Input, offset: f64) -> Vec<String> {
        let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-nostats", "-loglevel", "error"].map(String::from).to_vec();
        if input.is_web() {
            args.extend(header_args(target));
            args.extend(["-reconnect", "1", "-reconnect_delay_max", "5", "-rw_timeout", "15000000"].map(String::from));
        }
        if self.hls_input {
            // Providers name segments anything (.php, .jpg, none).
            args.extend(["-allowed_extensions", "ALL", "-allowed_segment_extensions", "ALL", "-extension_picky", "0"].map(String::from));
        }
        if offset > 0.0 {
            args.extend(["-ss".to_string(), format!("{offset:.3}")]);
        }
        args.extend(["-fflags".to_string(), "+genpts".to_string(), "-i".to_string(), input.ffmpeg_input()]);
        args.extend(["-map".to_string(), self.video_map.clone()]);
        if let Some(map) = &self.audio_map {
            args.extend(["-map".to_string(), map.clone()]);
        }
        match self.video {
            Video::Copy => args.extend(["-c:v", "copy"].map(String::from)),
            Video::Encode { deinterlace, downscale } => {
                let filters: Vec<String> = [deinterlace.then(|| "yadif".to_string()), downscale.then(|| format!("scale=-2:{MOST_ENCODED_LINES}"))]
                    .into_iter()
                    .flatten()
                    .collect();
                if !filters.is_empty() {
                    args.extend(["-vf".to_string(), filters.join(",")]);
                }
                args.extend(
                    [
                        "-c:v", "libx264", "-preset", "veryfast", "-crf", "21", "-pix_fmt", "yuv420p", "-profile:v", "high",
                        // A keyframe every two seconds, so fragments come steadily.
                        "-force_key_frames", "expr:gte(t,n_forced*2)",
                    ]
                    .map(String::from),
                );
            }
        }
        match self.audio {
            // ADTS (from MPEG-TS) becomes the form MP4 carries; MP4's own AAC passes as it is.
            Audio::Copy if self.audio_map.is_some() => args.extend(["-c:a", "copy", "-bsf:a", "aac_adtstoasc"].map(String::from)),
            Audio::Copy => {}
            Audio::Aac { surround } => args.extend(
                ["-af", "aformat=channel_layouts=5.1|stereo|mono", "-c:a", "aac", "-b:a", if surround { "384k" } else { "192k" }].map(String::from),
            ),
        }
        args.extend(
            [
                "-sn",
                "-dn",
                "-avoid_negative_ts",
                "make_zero",
                "-f",
                "mp4",
                // Fragmented, so it can be streamed as it's written: no index
                // at the start, a fragment at each keyframe or two seconds.
                "-movflags",
                "frag_keyframe+empty_moov+default_base_moof",
                "-frag_duration",
                "2000000",
                "pipe:1",
            ]
            .map(String::from),
        );
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{AudioStream, VideoStream};

    fn probe(format: &str, video: Option<(&str, &str)>, audio: Option<(&str, i64)>) -> Probe {
        Probe {
            format_name: format.into(),
            duration: Some(5400.0),
            video: video.map(|(codec, pix_fmt)| VideoStream { index: 0, codec: codec.into(), pix_fmt: pix_fmt.into(), height: Some(1080), ..VideoStream::default() }),
            audio: audio.map(|(codec, channels)| AudioStream { index: 1, codec: codec.into(), channels: Some(channels), ..AudioStream::default() }),
            ..Probe::default()
        }
    }

    fn target() -> StreamTarget {
        StreamTarget { url: "http://provider.example/movie/user/pass/1.mkv".into(), user_agent: Some("UA".into()), headers: vec![], is_live: false }
    }

    fn joined(plan: &CastPlan, offset: f64) -> String {
        plan.args(&target(), &Input::of(&target()).unwrap(), offset).join(" ")
    }

    const MP4: &str = "mov,mp4,m4a,3gp,3g2,mj2";
    const MKV: &str = "matroska,webm";

    #[test]
    fn web_files_go_as_they_are() {
        let film = probe(MP4, Some(("h264", "yuv420p")), Some(("aac", 2)));
        assert_eq!(route(Some(&film), Some("mp4"), true), Ok(Route::Direct { content_type: "video/mp4" }));
        let webm = probe(MKV, Some(("vp9", "yuv420p")), Some(("opus", 2)));
        assert_eq!(route(Some(&webm), Some("webm"), false), Ok(Route::Direct { content_type: "video/webm" }), "no ffmpeg needed");
        let music = probe(MP4, None, Some(("mp3", 2)));
        assert_eq!(route(Some(&music), None, true), Ok(Route::Direct { content_type: "video/mp4" }));
        // Not probed: the extension decides.
        assert_eq!(route(None, Some("m4v"), false), Ok(Route::Direct { content_type: "video/mp4" }));
    }

    #[test]
    fn what_tvs_may_not_decode_is_remuxed() {
        for (format, video, audio) in [
            (MP4, ("hevc", "yuv420p"), ("aac", 2)),
            (MP4, ("h264", "yuv420p10le"), ("aac", 2)),
            (MP4, ("h264", "yuv420p"), ("ac3", 6)),
            (MKV, ("h264", "yuv420p"), ("aac", 2)),
            (MKV, ("vp9", "yuv420p"), ("aac", 2)),
            ("mpegts", ("mpeg2video", "yuv420p"), ("mp2", 2)),
        ] {
            let found = probe(format, Some(video), Some(audio));
            assert!(matches!(route(Some(&found), None, true), Ok(Route::Mp4(_))), "{format} {video:?} {audio:?}");
            assert_eq!(route(Some(&found), None, false), Err(NEEDS_FFMPEG));
        }
        assert_eq!(route(None, Some("mkv"), false), Err(NEEDS_FFMPEG));
        // HLS films are read by ffmpeg, whatever they hold.
        let hls = probe("hls", Some(("h264", "yuv420p")), Some(("aac", 2)));
        assert!(matches!(route(Some(&hls), None, true), Ok(Route::Mp4(plan)) if plan.hls_input));
    }

    #[test]
    fn copies_h264_and_aac_into_fragmented_mp4() {
        let plan = CastPlan::new(Some(&probe(MKV, Some(("h264", "yuv420p")), Some(("aac", 2)))), false);
        let args = joined(&plan, 1234.5);
        assert!(args.contains("-user_agent UA -reconnect 1"), "{args}");
        assert!(args.contains("-ss 1234.500 -fflags +genpts -i http://provider.example/movie/user/pass/1.mkv -map 0:0 -map 0:1 -c:v copy -c:a copy -bsf:a aac_adtstoasc -sn"), "{args}");
        assert!(args.ends_with("-f mp4 -movflags frag_keyframe+empty_moov+default_base_moof -frag_duration 2000000 pipe:1"), "{args}");
        assert!(plan.notes().is_empty());
        assert!(!joined(&plan, 0.0).contains("-ss"));
    }

    #[test]
    fn encodes_other_video_and_converts_other_audio() {
        let mut film = probe(MKV, Some(("hevc", "yuv420p10le")), Some(("eac3", 6)));
        film.video.as_mut().unwrap().height = Some(2160);
        let plan = CastPlan::new(Some(&film), false);
        let args = joined(&plan, 0.0);
        assert!(args.contains("-vf scale=-2:1080 -c:v libx264 -preset veryfast -crf 21 -pix_fmt yuv420p -profile:v high"), "{args}");
        assert!(args.contains("-af aformat=channel_layouts=5.1|stereo|mono -c:a aac -b:a 384k"), "{args}");
        assert_eq!(plan.notes(), ["Video encoded to H.264 at 1080p", "Audio converted to AAC 5.1"]);
        let mut broadcast = probe("mpegts", Some(("mpeg2video", "yuv420p")), Some(("mp2", 2)));
        broadcast.video.as_mut().unwrap().interlaced = true;
        let args = joined(&CastPlan::new(Some(&broadcast), false), 0.0);
        assert!(args.contains("-vf yadif -c:v libx264") && args.contains("-c:a aac -b:a 192k"), "{args}");
    }

    #[test]
    fn hls_and_unprobed_inputs() {
        let args = joined(&CastPlan::new(None, true), 0.0);
        assert!(args.contains("-allowed_extensions ALL -allowed_segment_extensions ALL -extension_picky 0 -fflags +genpts"), "{args}");
        assert!(args.contains("-map 0:V:0? -map 0:a:0? -c:v copy -af aformat"), "{args}");
        let silent = CastPlan::new(Some(&probe(MKV, Some(("h264", "")), None)), false);
        assert!(!joined(&silent, 0.0).contains("-c:a"), "no audio to convert");
    }
}
