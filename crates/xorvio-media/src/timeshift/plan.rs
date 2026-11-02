//! ffmpeg's arguments for a live buffer: what it reads, what it copies or
//! converts, and the sliding-window HLS of fragmented MP4 it writes.

use std::path::Path;

use xorvio_model::StreamTarget;

use super::sniff::{Audio, Codecs, Track, Video};
use crate::gateway::PlaybackSupport;
use crate::process::header_args;

/// Seconds per segment (ffmpeg cuts at the first keyframe after this).
pub(crate) const SEGMENT_SECONDS: f64 = 2.0;
pub(crate) const PLAYLIST: &str = "index.m3u8";
pub(crate) const INIT: &str = "init.mp4";

/// Where ffmpeg's input comes from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Input {
    /// Bytes the gateway reads from the provider and writes to ffmpeg's stdin.
    Pipe {
        /// "mpegts" or "flv"; None leaves it to ffmpeg.
        format: Option<&'static str>,
        /// The bytes start where the video can be decoded from, with its
        /// parameter sets, so a short probe finds everything.
        from_keyframe: bool,
    },
    /// An HLS playlist ffmpeg reads itself, with the provider's headers.
    Playlist { url: String },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Plan {
    input: Input,
    video: Option<(String, VideoPlan)>,
    audio: Option<(String, AudioPlan)>,
    list_size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum VideoPlan {
    Copy,
    /// HEVC copied, tagged `hvc1` as WebKit's HLS player requires.
    CopyHevc,
    /// To H.264, deinterlacing the frames that are interlaced.
    Encode,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AudioPlan {
    Copy,
    /// AAC copied: MPEG-TS carries it with ADTS headers, which MP4 doesn't take.
    CopyAac,
    /// To AAC in a standard layout (5.1, stereo or mono), as the remux does.
    Aac { surround: bool },
}

impl Plan {
    /// Copies H.264, HEVC (when the web view decodes it), AAC and Dolby
    /// audio the web view decodes; converts the rest. `window_seconds` is
    /// how much the playlist keeps.
    pub fn new(input: Input, codecs: Codecs, support: PlaybackSupport, window_seconds: f64) -> Plan {
        let map = |pid: Option<u16>, kind: &str| match pid {
            // ffmpeg numbers MPEG-TS streams by PID, so this is exactly the one sniffed.
            Some(pid) => format!("0:i:{pid:#x}"),
            None => format!("0:{kind}:0?"),
        };
        let video = codecs.video.map(|Track { codec, pid }| {
            let plan = match codec {
                Video::H264 => VideoPlan::Copy,
                Video::Hevc if support.hevc => VideoPlan::CopyHevc,
                _ => VideoPlan::Encode,
            };
            (map(pid, "v"), plan)
        });
        let audio = codecs.audio.map(|Track { codec, pid }| {
            let plan = match codec {
                Audio::Aac => AudioPlan::CopyAac,
                Audio::Ac3 if support.ac3 => AudioPlan::Copy,
                Audio::Eac3 if support.eac3 => AudioPlan::Copy,
                // MP2 is stereo or mono.
                Audio::Mpeg => AudioPlan::Aac { surround: false },
                _ => AudioPlan::Aac { surround: true },
            };
            (map(pid, "a"), plan)
        });
        let window = if window_seconds.is_finite() { window_seconds } else { 1800.0 };
        Plan {
            input,
            video,
            audio,
            list_size: (window / SEGMENT_SECONDS).ceil().max(3.0) as u32,
        }
    }

    pub fn reads_stdin(&self) -> bool {
        matches!(self.input, Input::Pipe { .. })
    }

    /// What was done to the stream, for the player's stream information.
    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if matches!(self.video, Some((_, VideoPlan::Encode))) {
            notes.push("Video encoded to H.264".to_string());
        }
        if let Some((_, AudioPlan::Aac { surround })) = self.audio {
            notes.push(format!("Audio converted to AAC {}", if surround { "5.1" } else { "stereo" }));
        }
        notes
    }

    /// A line for the log (never the address).
    pub fn describe(&self) -> String {
        let input = match &self.input {
            Input::Pipe { format, from_keyframe } => {
                format!("pipe ({}{})", format.unwrap_or("detected"), if *from_keyframe { ", from a keyframe" } else { "" })
            }
            Input::Playlist { .. } => "HLS playlist".to_string(),
        };
        format!("{input}, video {:?}, audio {:?}, {} segments", self.video, self.audio, self.list_size)
    }

    /// ffmpeg's arguments, writing `index.m3u8`, `init.mp4` and `seg<n>.m4s` into `dir`.
    pub fn args(&self, target: &StreamTarget, dir: &Path) -> Vec<String> {
        let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-nostats", "-loglevel", "error"].map(String::from).to_vec();
        match &self.input {
            Input::Pipe { format, from_keyframe } => {
                // MPEG-TS and FLV streams can add streams at any time, so
                // ffmpeg probes for the whole analyze duration (five seconds
                // by default). Starting at a keyframe, two seconds hold the
                // parameter sets and the first audio; that's less than the
                // first segment takes anyway. Otherwise allow for a long GOP.
                let analyze = if *from_keyframe || *format == Some("flv") { "2000000" } else { "10000000" };
                args.extend(["-analyzeduration", analyze, "-probesize", "32M"].map(String::from));
                args.extend(["-fflags", "+genpts+discardcorrupt"].map(String::from));
                if let Some(format) = format {
                    args.extend(["-f".to_string(), format.to_string()]);
                    if *format == "mpegts" {
                        // A reconnect can bring a new PMT; keep the same streams.
                        args.extend(["-merge_pmt_versions", "1"].map(String::from));
                    }
                }
                args.extend(["-i", "pipe:0"].map(String::from));
            }
            Input::Playlist { url } => {
                args.extend(header_args(target));
                args.extend(
                    [
                        "-rw_timeout",
                        "15000000",
                        // One connection at a time, as the provider allows.
                        "-http_multiple",
                        "0",
                        "-seg_max_retry",
                        "3",
                        // Providers name segments anything (.php, .jpg, none).
                        "-allowed_extensions",
                        "ALL",
                        "-allowed_segment_extensions",
                        "ALL",
                        "-extension_picky",
                        "0",
                        "-fflags",
                        "+genpts+discardcorrupt",
                        "-f",
                        "hls",
                        "-i",
                    ]
                    .map(String::from),
                );
                args.push(url.clone());
            }
        }
        if let Some((map, _)) = &self.video {
            args.extend(["-map".to_string(), map.clone()]);
        }
        if let Some((map, _)) = &self.audio {
            args.extend(["-map".to_string(), map.clone()]);
        }
        match self.video {
            Some((_, VideoPlan::Copy)) => args.extend(["-c:v", "copy"].map(String::from)),
            Some((_, VideoPlan::CopyHevc)) => args.extend(["-c:v", "copy", "-tag:v", "hvc1"].map(String::from)),
            Some((_, VideoPlan::Encode)) => {
                let keyframes = format!("expr:gte(t,n_forced*{SEGMENT_SECONDS})");
                args.extend(
                    [
                        "-vf",
                        "yadif=deint=interlaced",
                        "-c:v",
                        "libx264",
                        "-preset",
                        "veryfast",
                        "-crf",
                        "21",
                        "-pix_fmt",
                        "yuv420p",
                        // A keyframe where every segment should start.
                        "-force_key_frames",
                        keyframes.as_str(),
                    ]
                    .map(String::from),
                );
            }
            None => {}
        }
        match self.audio {
            Some((_, AudioPlan::Copy)) => args.extend(["-c:a", "copy"].map(String::from)),
            Some((_, AudioPlan::CopyAac)) => args.extend(["-c:a", "copy", "-bsf:a", "aac_adtstoasc"].map(String::from)),
            Some((_, AudioPlan::Aac { surround })) => args.extend(
                [
                    "-af",
                    "aformat=channel_layouts=5.1|stereo|mono",
                    "-c:a",
                    "aac",
                    "-b:a",
                    if surround { "384k" } else { "192k" },
                ]
                .map(String::from),
            ),
            None => {}
        }
        let path = |name: &str| ffmpeg_path(&dir.join(name));
        args.extend(["-sn", "-dn", "-f", "hls", "-hls_time"].map(String::from));
        args.push(SEGMENT_SECONDS.to_string());
        args.extend([
            "-hls_list_size".to_string(),
            self.list_size.to_string(),
            // Segments leave the disk as they leave the window; files appear
            // whole (written under another name, then renamed); and wall-clock
            // times let the player show when each moment went out.
            "-hls_flags".to_string(),
            "delete_segments+independent_segments+temp_file+program_date_time".to_string(),
            "-hls_segment_type".to_string(),
            "fmp4".to_string(),
            "-hls_fmp4_init_filename".to_string(),
            INIT.to_string(),
            "-hls_segment_filename".to_string(),
            path("seg%d.m4s"),
            path(PLAYLIST),
        ]);
        args
    }
}

/// A path as ffmpeg is told it. Its HLS writer puts init.mp4 beside the
/// playlist by looking for the last "/" in the playlist's path, so Windows's
/// "\" go as "/" (which Windows takes too).
fn ffmpeg_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    if cfg!(windows) { path.replace('\\', "/") } else { path.into_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> StreamTarget {
        StreamTarget {
            url: "http://provider.example/live/user/pass/1.ts".into(),
            user_agent: Some("UA".into()),
            headers: vec![("Referer".into(), "http://r".into())],
            is_live: true,
        }
    }

    fn codecs(video: Option<(Video, Option<u16>)>, audio: Option<(Audio, Option<u16>)>) -> Codecs {
        Codecs {
            video: video.map(|(codec, pid)| Track { codec, pid }),
            audio: audio.map(|(codec, pid)| Track { codec, pid }),
        }
    }

    fn pipe() -> Input {
        Input::Pipe { format: Some("mpegts"), from_keyframe: true }
    }

    fn joined(plan: &Plan) -> String {
        plan.args(&target(), Path::new("/tmp/ts/abc")).join(" ")
    }

    const NO_DOLBY: PlaybackSupport = PlaybackSupport { hevc: true, ac3: false, eac3: false, vp9: false, av1: false };
    const DOLBY: PlaybackSupport = PlaybackSupport { hevc: true, ac3: true, eac3: true, vp9: false, av1: false };

    #[test]
    fn copies_h264_and_aac_from_the_pipe() {
        let plan = Plan::new(pipe(), codecs(Some((Video::H264, Some(0x100))), Some((Audio::Aac, Some(0x101)))), NO_DOLBY, 1800.0);
        let args = joined(&plan);
        assert!(args.starts_with("-hide_banner -nostdin -nostats -loglevel error -analyzeduration 2000000 -probesize 32M -fflags +genpts+discardcorrupt -f mpegts -merge_pmt_versions 1 -i pipe:0"), "{args}");
        assert!(args.contains("-map 0:i:0x100 -map 0:i:0x101 -c:v copy -c:a copy -bsf:a aac_adtstoasc -sn -dn"), "{args}");
        // The folder's files go with "/" on every platform, so init.mp4 lands beside the playlist.
        assert!(args.ends_with(
            "-f hls -hls_time 2 -hls_list_size 900 -hls_flags delete_segments+independent_segments+temp_file+program_date_time -hls_segment_type fmp4 -hls_fmp4_init_filename init.mp4 -hls_segment_filename /tmp/ts/abc/seg%d.m4s /tmp/ts/abc/index.m3u8"
        ), "{args}");
        #[cfg(windows)]
        assert_eq!(ffmpeg_path(Path::new(r"C:\Temp\xorvio-timeshift\abc\index.m3u8")), "C:/Temp/xorvio-timeshift/abc/index.m3u8");
        assert!(!args.contains("provider.example") && !args.contains("-user_agent"), "the gateway reads the provider, not ffmpeg");
        assert!(plan.reads_stdin() && plan.notes().is_empty());
    }

    #[test]
    fn copies_hevc_tagged_hvc1_and_dolby_audio_when_decoded() {
        let sniffed = codecs(Some((Video::Hevc, Some(0x200))), Some((Audio::Ac3, Some(0x202))));
        let args = joined(&Plan::new(pipe(), sniffed, DOLBY, 3600.0));
        assert!(args.contains("-map 0:i:0x200 -map 0:i:0x202 -c:v copy -tag:v hvc1 -c:a copy -sn"), "{args}");
        assert!(args.contains("-hls_list_size 1800"));
        let args = joined(&Plan::new(pipe(), sniffed, NO_DOLBY, 3600.0));
        assert!(args.contains("-c:a aac -b:a 384k"), "AC-3 the web view can't play becomes AAC: {args}");
        let no_hevc = PlaybackSupport { hevc: false, ..DOLBY };
        assert!(joined(&Plan::new(pipe(), sniffed, no_hevc, 60.0)).contains("-c:v libx264"));
    }

    #[test]
    fn encodes_mpeg2_and_converts_mp2() {
        let plan = Plan::new(pipe(), codecs(Some((Video::Mpeg2, Some(0x100))), Some((Audio::Mpeg, Some(0x101)))), DOLBY, 900.0);
        let args = joined(&plan);
        assert!(args.contains(
            "-vf yadif=deint=interlaced -c:v libx264 -preset veryfast -crf 21 -pix_fmt yuv420p -force_key_frames expr:gte(t,n_forced*2) -af aformat=channel_layouts=5.1|stereo|mono -c:a aac -b:a 192k -sn"
        ), "{args}");
        assert_eq!(plan.notes(), ["Video encoded to H.264", "Audio converted to AAC stereo"]);
        let latm = joined(&Plan::new(pipe(), codecs(Some((Video::H264, Some(1))), Some((Audio::AacLatm, Some(2)))), DOLBY, 900.0));
        assert!(latm.contains("-c:v copy -af aformat=channel_layouts=5.1|stereo|mono -c:a aac -b:a 384k"), "{latm}");
    }

    #[test]
    fn unknown_streams_are_mapped_if_there_and_converted() {
        let input = Input::Pipe { format: None, from_keyframe: false };
        let args = joined(&Plan::new(input, Codecs::unknown(), DOLBY, 1800.0));
        assert!(args.contains("-analyzeduration 10000000"), "no keyframe to start from: a long probe");
        assert!(!args.contains("-f mpegts"));
        assert!(args.contains("-map 0:v:0? -map 0:a:0? -vf yadif"), "{args}");
        let radio = joined(&Plan::new(pipe(), codecs(None, Some((Audio::Aac, Some(0x101)))), DOLBY, 1800.0));
        assert!(radio.contains("-i pipe:0 -map 0:i:0x101 -c:a copy"), "{radio}");
        let flv = joined(&Plan::new(Input::Pipe { format: Some("flv"), from_keyframe: false }, codecs(Some((Video::H264, None)), None), DOLBY, 1800.0));
        assert!(flv.contains("-analyzeduration 2000000") && flv.contains("-f flv -i pipe:0 -map 0:v:0?"), "{flv}");
    }

    #[test]
    fn hls_sources_are_read_by_ffmpeg_with_the_headers() {
        let input = Input::Playlist { url: "http://provider.example/live/user/pass/1.m3u8".into() };
        let plan = Plan::new(input, codecs(Some((Video::H264, None)), Some((Audio::Aac, None))), DOLBY, 7200.0);
        let args = joined(&plan);
        assert!(args.contains("-user_agent UA -headers Referer: http://r\r\n -rw_timeout 15000000 -http_multiple 0"), "{args}");
        assert!(args.contains("-extension_picky 0 -fflags +genpts+discardcorrupt -f hls -i http://provider.example/live/user/pass/1.m3u8 -map 0:v:0? -map 0:a:0?"), "{args}");
        assert!(args.contains("-hls_list_size 3600"));
        assert!(!plan.reads_stdin());
        assert!(!plan.describe().contains("provider.example"), "logs never hold the address");
    }

    #[test]
    fn short_or_odd_windows_still_make_a_playlist() {
        let sniffed = codecs(Some((Video::H264, Some(1))), None);
        assert!(joined(&Plan::new(pipe(), sniffed, DOLBY, 1.0)).contains("-hls_list_size 3 "));
        assert!(joined(&Plan::new(pipe(), sniffed, DOLBY, f64::NAN)).contains("-hls_list_size 900 "));
    }
}
