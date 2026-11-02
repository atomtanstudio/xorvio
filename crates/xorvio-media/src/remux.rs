//! Remuxing what the web view can't open (MKV, AVI, AC-3/DTS audio…) into
//! MPEG-TS with ffmpeg, streamed straight into the HTTP response for
//! mpegts.js to play. Video is copied when it's H.264 or HEVC the web view
//! decodes and encoded to H.264 otherwise (VP9 and AV1 too, which MPEG-TS
//! can't carry here); audio is copied when it's AAC, MP3 or Dolby audio the
//! web view decodes, and otherwise turned into AAC. Seeking starts a new
//! ffmpeg at an offset.

use std::path::Path;
use std::process::Stdio;

use bytes::Bytes;
use futures_util::Stream;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdout};
use tokio::sync::watch;
use xorvio_model::StreamTarget;

use crate::gateway::PlaybackSupport;
use crate::probe::{Probe, VideoStream};
use crate::process::{Input, drain_stderr, web_input_args, tool_command};

/// How ffmpeg treats each kind of stream, decided once from ffprobe.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RemuxPlan {
    /// Keep reading a local file as it grows (a recording still being made).
    follow: bool,
    video_map: String,
    video: VideoPlan,
    audio_map: Option<String>,
    audio: AudioPlan,
    /// Seconds the audio is moved; positive plays it later.
    audio_delay: f64,
    /// Text subtitle tracks, each written as WebVTT beside the video.
    subtitles: Vec<i64>,
    /// A picture subtitle track drawn into the video.
    burn: Option<Burn>,
}

/// Picture subtitles (PGS, VobSub, DVB) drawn into the video, the only way
/// the web view can show them, which means encoding the video.
#[derive(Debug, Clone, PartialEq)]
struct Burn {
    index: i64,
    /// The size the pictures are drawn at when their canvas isn't the
    /// video's (1080p subtitles on 4K video, which is made 1080p anyway).
    scale_to: Option<(i64, i64)>,
}

#[derive(Debug, Clone, PartialEq)]
enum VideoPlan {
    Copy,
    /// MPEG-2, VC-1, VP9, AV1 and other video that can't go through as it is.
    Encode {
        deinterlace: bool,
        /// Taller than 1080 lines (4K VP9 and AV1, mostly), so made 1080p,
        /// which software encoding keeps up with.
        downscale: bool,
    },
}

/// The tallest video encoded at its own size.
const MOST_ENCODED_LINES: i64 = 1080;

#[derive(Debug, Clone, PartialEq)]
enum AudioPlan {
    Copy,
    /// To AAC in a standard layout: 5.1, stereo or mono, whichever is
    /// closest. ffmpeg's AAC encoder writes other layouts (Dolby's
    /// side-surround 5.1) with a custom channel map that WebKit refuses,
    /// and can't write 7.1 into MPEG-TS at all. `surround` (or not knowing)
    /// gets the higher bitrate.
    Aac { surround: bool },
}

impl RemuxPlan {
    /// The plan for a probed stream, or a cautious one (copy video, AAC
    /// audio) when it couldn't be probed.
    pub fn new(probe: Option<&Probe>, support: PlaybackSupport) -> RemuxPlan {
        let Some(probe) = probe else {
            return RemuxPlan {
                follow: false,
                video_map: "0:V:0".into(),
                video: VideoPlan::Copy,
                audio_map: Some("0:a:0?".into()),
                audio: AudioPlan::Aac { surround: true },
                audio_delay: 0.0,
                subtitles: Vec::new(),
                burn: None,
            };
        };
        let (video_map, video) = match &probe.video {
            Some(v) if decodes_video(v, support) => (format!("0:{}", v.index), VideoPlan::Copy),
            Some(v) => (
                format!("0:{}", v.index),
                VideoPlan::Encode {
                    deinterlace: v.interlaced,
                    downscale: v.height.is_some_and(|height| height > MOST_ENCODED_LINES),
                },
            ),
            None => ("0:V:0?".into(), VideoPlan::Copy),
        };
        let (audio_map, audio) = match &probe.audio {
            Some(a) if decodes_audio(&a.codec, support) => {
                (Some(format!("0:{}", a.index)), AudioPlan::Copy)
            }
            Some(a) => (
                Some(format!("0:{}", a.index)),
                AudioPlan::Aac {
                    surround: a.channels.is_some_and(|c| c > 2),
                },
            ),
            None => (None, AudioPlan::Copy),
        };
        RemuxPlan {
            follow: false,
            video_map,
            video,
            audio_map,
            audio,
            audio_delay: 0.0,
            subtitles: probe.subtitles.iter().filter(|s| s.is_text()).map(|s| s.index).collect(),
            burn: None,
        }
    }

    /// The plan drawing a picture subtitle track into the video (`track`
    /// one of the probe's picture tracks), or drawing none.
    pub fn burning(self, track: Option<i64>, probe: Option<&Probe>) -> RemuxPlan {
        let subtitle = track.and_then(|index| probe?.subtitles.iter().find(|s| s.index == index && !s.is_text()));
        let Some(subtitle) = subtitle else {
            return RemuxPlan { burn: None, ..self };
        };
        let video = probe.and_then(|p| p.video.as_ref());
        let downscale = video.and_then(|v| v.height).is_some_and(|height| height > MOST_ENCODED_LINES);
        // The size the video comes out at, as `scale=-2:1080` makes it.
        let size = video.and_then(|v| Some((v.width?, v.height?))).filter(|(_, height)| *height > 0).map(|(width, height)| {
            if downscale {
                let width = (width * MOST_ENCODED_LINES + height / 2) / height;
                (width + width % 2, MOST_ENCODED_LINES)
            } else {
                (width, height)
            }
        });
        let scale_to = match (size, subtitle.width.zip(subtitle.height)) {
            (Some(size), Some(canvas)) if size != canvas => Some(size),
            _ => None,
        };
        RemuxPlan {
            video: VideoPlan::Encode {
                deinterlace: video.is_some_and(|v| v.interlaced),
                downscale,
            },
            burn: Some(Burn { index: subtitle.index, scale_to }),
            ..self
        }
    }

    /// The picture subtitle track drawn into the video, if any.
    pub fn burned_subtitle(&self) -> Option<i64> {
        self.burn.as_ref().map(|burn| burn.index)
    }

    pub fn with_audio_delay(self, seconds: f64) -> RemuxPlan {
        RemuxPlan {
            audio_delay: seconds,
            ..self
        }
    }

    pub fn is_following(&self) -> bool {
        self.follow
    }

    /// Whether a subtitle track is written beside the video.
    pub fn has_subtitles(&self, index: i64) -> bool {
        self.subtitles.contains(&index)
    }

    /// What was done to the stream, for the player's stream information.
    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if let VideoPlan::Encode { deinterlace, downscale } = self.video {
            let done = if deinterlace { "Video deinterlaced and encoded to H.264" } else { "Video encoded to H.264" };
            notes.push(if downscale { format!("{done} at 1080p") } else { done.to_string() });
        }
        if self.burn.is_some() {
            notes.push("Subtitles drawn into the picture".to_string());
        }
        if let AudioPlan::Aac { surround } = self.audio {
            notes.push(format!("Audio converted to AAC {}", if surround { "5.1" } else { "stereo" }));
        }
        notes
    }

    /// The same plan for a file that's still being written: ffmpeg waits at
    /// its end for more, and finishes once nothing new arrives for 10 seconds.
    pub fn following(self) -> RemuxPlan {
        RemuxPlan {
            follow: true,
            subtitles: Vec::new(),
            ..self
        }
    }

    /// ffmpeg's arguments, writing MPEG-TS to stdout from `offset` seconds in,
    /// and each text subtitle track to `<subtitle_dir>/<index>.vtt` as it goes
    /// (one read of the input serves both).
    pub fn args(&self, target: &StreamTarget, input: &Input, offset: f64, subtitle_dir: Option<&Path>) -> Vec<String> {
        let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-nostats", "-loglevel", "error"]
            .map(String::from)
            .to_vec();
        if input.is_web() {
            args.extend(web_input_args(target));
            args.extend(
                [
                    "-reconnect",
                    "1",
                    "-reconnect_delay_max",
                    "5",
                    "-rw_timeout",
                    "15000000",
                ]
                .map(String::from),
            );
        }
        if self.follow && !input.is_web() {
            args.extend(["-follow", "1", "-rw_timeout", "10000000"].map(String::from));
        }
        if offset > 0.0 {
            args.extend(["-ss".to_string(), format!("{offset:.3}")]);
        }
        args.extend([
            "-fflags".to_string(),
            "+genpts".to_string(),
            "-i".to_string(),
            input.ffmpeg_input(),
        ]);
        let filters: Vec<String> = match self.video {
            VideoPlan::Copy => Vec::new(),
            VideoPlan::Encode { deinterlace, downscale } => [
                deinterlace.then(|| "yadif".to_string()),
                downscale.then(|| format!("scale=-2:{MOST_ENCODED_LINES}")),
            ]
            .into_iter()
            .flatten()
            .collect(),
        };
        match &self.burn {
            // The subtitle pictures go over the video once it's deinterlaced
            // and sized, drawn at its size.
            Some(burn) => {
                let video = if filters.is_empty() { "null".to_string() } else { filters.join(",") };
                let pictures = burn.scale_to.map_or("null".to_string(), |(width, height)| format!("scale={width}:{height}"));
                let graph = format!("[{}]{video}[base];[0:{}]{pictures}[pictures];[base][pictures]overlay=eof_action=pass[video]", self.video_map, burn.index);
                args.extend(["-filter_complex".to_string(), graph, "-map".to_string(), "[video]".to_string()]);
            }
            None => args.extend(["-map".to_string(), self.video_map.clone()]),
        }
        if let Some(map) = &self.audio_map {
            args.extend(["-map".to_string(), map.clone()]);
        }
        match &self.video {
            VideoPlan::Copy => args.extend(["-c:v", "copy"].map(String::from)),
            VideoPlan::Encode { .. } => {
                if !filters.is_empty() && self.burn.is_none() {
                    args.extend(["-vf".to_string(), filters.join(",")]);
                }
                args.extend(
                    [
                        "-c:v", "libx264", "-preset", "veryfast", "-crf", "21", "-pix_fmt",
                        "yuv420p",
                    ]
                    .map(String::from),
                );
            }
        }
        match &self.audio {
            AudioPlan::Copy => args.extend(["-c:a", "copy"].map(String::from)),
            AudioPlan::Aac { surround } => args.extend(
                [
                    "-af",
                    "aformat=channel_layouts=5.1|stereo|mono",
                    "-c:a",
                    "aac",
                    "-b:a",
                    if *surround { "384k" } else { "192k" },
                ]
                .map(String::from),
            ),
        }
        if self.audio_delay != 0.0 && self.audio_map.is_some() {
            let d = self.audio_delay;
            args.extend(["-bsf:a".to_string(), format!("setts=pts=PTS{d:+.3}/TB:dts=DTS{d:+.3}/TB")]);
        }
        // Timestamps from zero with no mux delay, so playback starts at once
        // and the player's clock lines up with `start_offset_seconds`.
        args.extend(
            [
                "-sn",
                "-dn",
                "-muxdelay",
                "0",
                "-muxpreload",
                "0",
                "-f",
                "mpegts",
                "pipe:1",
            ]
            .map(String::from),
        );
        if let Some(dir) = subtitle_dir {
            for index in &self.subtitles {
                args.extend(["-map".to_string(), format!("0:{index}")]);
                args.extend(["-c:s", "webvtt", "-flush_packets", "1", "-f", "webvtt"].map(String::from));
                args.push(dir.join(format!("{index}.vtt")).to_string_lossy().into_owned());
            }
        }
        args
    }
}

/// Video the web view decodes as it is. 10-bit H.264 is a codec no
/// hardware decoder takes.
fn decodes_video(video: &VideoStream, support: PlaybackSupport) -> bool {
    match video.codec.as_str() {
        "hevc" => support.hevc,
        "h264" => !video.pix_fmt.contains("10"),
        // VP9 has no MPEG-TS form, and ffmpeg writes AV1 into MPEG-TS in a
        // form neither mpegts.js nor ffprobe reads.
        _ => false,
    }
}

/// Audio that goes into MPEG-TS and through Media Source as it is.
fn decodes_audio(codec: &str, support: PlaybackSupport) -> bool {
    match codec {
        "aac" | "mp3" => true,
        "ac3" => support.ac3,
        "eac3" => support.eac3,
        _ => false,
    }
}

/// Starts ffmpeg and returns its output as a byte stream. ffmpeg is killed
/// when the stream is dropped (the player disconnected) or when `stop`
/// moves past `generation` (the stream was released or seeked).
pub(crate) fn spawn(
    ffmpeg: &Path,
    args: Vec<String>,
    stop: watch::Receiver<u64>,
    generation: u64,
) -> anyhow::Result<impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static> {
    let mut child = tool_command(ffmpeg)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("ffmpeg has no output"))?;
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(drain_stderr(stderr, "remux"));
    }
    struct State {
        child: Child,
        stdout: ChildStdout,
        buffer: Vec<u8>,
    }
    let state = State {
        child,
        stdout,
        buffer: vec![0; 64 * 1024],
    };
    Ok(futures_util::stream::unfold(Some(state), move |state| {
        let mut stop = stop.clone();
        async move {
            let mut state = state?;
            let read = tokio::select! {
                read = state.stdout.read(&mut state.buffer) => read,
                _ = stop.wait_for(|current| *current != generation) => {
                    let _ = state.child.start_kill();
                    return None;
                }
            };
            match read {
                Ok(0) => None,
                Ok(n) => {
                    let chunk = Bytes::copy_from_slice(&state.buffer[..n]);
                    Some((Ok(chunk), Some(state)))
                }
                Err(error) => Some((Err(error), None)),
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{AudioStream, SubtitleStream};

    fn target() -> StreamTarget {
        StreamTarget {
            url: "https://a.example/film.mkv".into(),
            user_agent: Some("UA".into()),
            headers: vec![],
            is_live: false,
        }
    }

    fn probe(video: &str, audio: &str, channels: i64) -> Probe {
        Probe {
            format_name: "matroska,webm".into(),
            duration: Some(60.0),
            video: Some(VideoStream {
                index: 0,
                codec: video.into(),
                tag: String::new(),
                interlaced: true,
                ..VideoStream::default()
            }),
            audio: Some(AudioStream {
                index: 2,
                codec: audio.into(),
                channels: Some(channels),
                ..AudioStream::default()
            }),
            ..Probe::default()
        }
    }

    fn joined(plan: &RemuxPlan, offset: f64) -> String {
        let input = Input::of(&target()).unwrap();
        plan.args(&target(), &input, offset, None).join(" ")
    }

    #[test]
    fn copies_web_video_and_converts_other_audio() {
        let none = PlaybackSupport::default();
        let args = joined(&RemuxPlan::new(Some(&probe("hevc", "ac3", 6)), none), 90.0);
        assert!(args.contains("-ss 90.000 -fflags +genpts -i https://a.example/film.mkv"));
        assert!(args.contains(
            "-map 0:0 -map 0:2 -c:v copy -af aformat=channel_layouts=5.1|stereo|mono -c:a aac -b:a 384k -sn"
        ));
        assert!(args.contains("-user_agent UA"));
        assert!(args.ends_with("-sn -dn -muxdelay 0 -muxpreload 0 -f mpegts pipe:1"));
        assert!(!args.contains("hvc1") && !args.contains("-bsf"));
    }

    #[test]
    fn copies_aac_and_encodes_old_video() {
        let none = PlaybackSupport::default();
        let args = joined(&RemuxPlan::new(Some(&probe("mpeg2video", "aac", 2)), none), 0.0);
        assert!(!args.contains("-ss"));
        assert!(args.contains("-vf yadif -c:v libx264"));
        assert!(
            args.contains("-c:a copy -sn"),
            "ADTS AAC goes into TS as it is"
        );
        let many = joined(&RemuxPlan::new(Some(&probe("h264", "truehd", 8)), none), 0.0);
        assert!(many.contains("-c:v copy -af aformat=channel_layouts=5.1|stereo|mono -c:a aac -b:a 384k"));
        let mono = joined(&RemuxPlan::new(Some(&probe("h264", "flac", 1)), none), 0.0);
        assert!(mono.contains("-c:a aac -b:a 192k -sn"));
    }

    #[test]
    fn copies_what_the_web_view_reports_it_decodes() {
        let dolby = PlaybackSupport {
            hevc: false,
            ac3: true,
            eac3: true,
            vp9: true,
            av1: true,
        };
        let args = joined(&RemuxPlan::new(Some(&probe("hevc", "eac3", 6)), dolby), 0.0);
        assert!(args.contains("-c:v libx264"), "no HEVC decoder: encode");
        assert!(args.contains("-c:a copy"), "E-AC-3 goes through");
        let mut hi10p = probe("h264", "aac", 2);
        hi10p.video.as_mut().unwrap().pix_fmt = "yuv420p10le".into();
        let args = joined(&RemuxPlan::new(Some(&hi10p), dolby), 0.0);
        assert!(args.contains("-c:v libx264"), "10-bit H.264 is encoded");
    }

    #[test]
    fn vp9_and_av1_are_encoded_and_4k_made_1080p() {
        let everything = PlaybackSupport { hevc: true, ac3: true, eac3: true, vp9: true, av1: true };
        for codec in ["vp9", "av1"] {
            let mut film = probe(codec, "opus", 2);
            film.video.as_mut().unwrap().interlaced = false;
            film.video.as_mut().unwrap().height = Some(2160);
            let plan = RemuxPlan::new(Some(&film), everything);
            let args = joined(&plan, 0.0);
            assert!(args.contains("-vf scale=-2:1080 -c:v libx264"), "{codec}: {args}");
            assert_eq!(plan.notes()[0], "Video encoded to H.264 at 1080p");
            film.video.as_mut().unwrap().height = Some(1080);
            assert!(!joined(&RemuxPlan::new(Some(&film), everything), 0.0).contains("scale="), "{codec} at 1080p keeps its size");
        }
        // Interlaced 1080i is deinterlaced at its own size.
        let mut broadcast = probe("mpeg2video", "mp2", 2);
        broadcast.video.as_mut().unwrap().height = Some(1080);
        assert!(joined(&RemuxPlan::new(Some(&broadcast), everything), 0.0).contains("-vf yadif -c:v libx264"));
    }

    #[test]
    fn picture_subtitles_are_drawn_at_the_videos_size() {
        use crate::probe::SubtitleStream;
        let support = PlaybackSupport::default();
        let film = |height: i64, canvas: (i64, i64)| {
            let mut film = probe("h264", "aac", 2);
            let video = film.video.as_mut().unwrap();
            (video.interlaced, video.width, video.height) = (false, Some(height * 16 / 9), Some(height));
            film.subtitles = vec![
                SubtitleStream { index: 3, codec: "subrip".into(), ..SubtitleStream::default() },
                SubtitleStream { index: 4, codec: "hdmv_pgs_subtitle".into(), width: Some(canvas.0), height: Some(canvas.1), ..SubtitleStream::default() },
            ];
            film
        };
        let args = |film: &Probe, track: i64| joined(&RemuxPlan::new(Some(film), support).burning(Some(track), Some(film)), 0.0);

        // 1080p subtitles on 1080p video go straight over it, which is encoded.
        let full_hd = film(1080, (1920, 1080));
        let burned = args(&full_hd, 4);
        assert!(burned.contains("-filter_complex [0:0]null[base];[0:4]null[pictures];[base][pictures]overlay=eof_action=pass[video] -map [video]"), "{burned}");
        assert!(burned.contains("-c:v libx264") && !burned.contains("-vf"), "{burned}");
        // On 4K the video is made 1080p first, where the pictures fit.
        let burned = args(&film(2160, (1920, 1080)), 4);
        assert!(burned.contains("[0:0]scale=-2:1080[base];[0:4]null[pictures]"), "{burned}");
        // On 720p the pictures are scaled down to it.
        let burned = args(&film(720, (1920, 1080)), 4);
        assert!(burned.contains("[0:4]scale=1280:720[pictures]"), "{burned}");
        // A text track isn't drawn: it's shown as text.
        let plan = RemuxPlan::new(Some(&full_hd), support).burning(Some(3), Some(&full_hd));
        assert_eq!(plan.burned_subtitle(), None);
        assert!(joined(&plan, 0.0).contains("-map 0:0 -map 0:2 -c:v copy"));
        // Turning it off goes back to copying the video.
        let plan = RemuxPlan::new(Some(&full_hd), support).burning(Some(4), Some(&full_hd)).burning(None, Some(&full_hd));
        assert_eq!(plan.burned_subtitle(), None);
    }

    #[test]
    fn writes_text_subtitles_and_moves_audio() {
        let mut film = probe("h264", "aac", 2);
        film.subtitles = vec![
            SubtitleStream { index: 3, codec: "subrip".into(), ..SubtitleStream::default() },
            SubtitleStream { index: 4, codec: "hdmv_pgs_subtitle".into(), ..SubtitleStream::default() },
        ];
        let plan = RemuxPlan::new(Some(&film), PlaybackSupport::default()).with_audio_delay(-0.25);
        assert!(plan.has_subtitles(3) && !plan.has_subtitles(4), "picture subtitles can't become WebVTT");
        let input = Input::of(&target()).unwrap();
        let args = plan.args(&target(), &input, 0.0, Some(Path::new("/tmp/subs"))).join(" ");
        assert!(args.contains("-c:a copy -bsf:a setts=pts=PTS-0.250/TB:dts=DTS-0.250/TB -sn"));
        let vtt = Path::new("/tmp/subs").join("3.vtt");
        assert!(args.ends_with(&format!("pipe:1 -map 0:3 -c:s webvtt -flush_packets 1 -f webvtt {}", vtt.display())));
        assert!(plan.following().args(&target(), &input, 0.0, Some(Path::new("/tmp/subs"))).last().unwrap() == "pipe:1");
    }

    #[test]
    fn growing_files_are_followed() {
        let show = if cfg!(windows) { r"C:\rec\show.ts" } else { "/rec/show.ts" };
        let file = StreamTarget {
            url: show.into(),
            user_agent: None,
            headers: vec![],
            is_live: false,
        };
        let input = Input::of(&file).unwrap();
        let args = RemuxPlan::new(None, PlaybackSupport::default())
            .following()
            .args(&file, &input, 0.0, None)
            .join(" ");
        assert!(args.contains(&format!("-follow 1 -rw_timeout 10000000 -fflags +genpts -i {show}")));
    }

    #[test]
    fn unprobed_streams_get_a_cautious_plan() {
        let args = joined(&RemuxPlan::new(None, PlaybackSupport::default()), 0.0);
        assert!(args.contains(
            "-map 0:V:0 -map 0:a:0? -c:v copy -af aformat=channel_layouts=5.1|stereo|mono -c:a aac -b:a 384k"
        ));
    }
}
