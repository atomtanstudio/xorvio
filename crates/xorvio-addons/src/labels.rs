//! Reads an add-on's description of a stream into something Xorvio can show
//! and sort. The protocol has no fields for quality or size, so add-ons write
//! them into the name and description ("4k HDR", "💾 21.3 GB"); this finds them.

use std::sync::LazyLock;

use regex::Regex;
use xorvio_model::{AddonStream, StreamTarget};

use crate::protocol::Stream;

pub(crate) enum Reading {
    Copy(Box<AddonStream>),
    /// The add-on reporting a problem in place of streams ("Invalid API key").
    Notice(String),
    /// Something Xorvio can't play and shouldn't list (a YouTube id, an external page).
    Skip,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

static ERROR: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\berror\b"));

pub(crate) fn read(stream: &Stream, addon_id: &str, addon_name: &str) -> Reading {
    let name = stream.name.clone().unwrap_or_default();
    let description = stream.description.clone().unwrap_or_default();
    if ERROR.is_match(&name) {
        let notice = description.split_whitespace().collect::<Vec<_>>().join(" ");
        return Reading::Notice(if notice.is_empty() {
            format!("{addon_name} reported a problem.")
        } else {
            notice
        });
    }

    let target = match stream.url.as_deref().map(str::trim) {
        Some(url) if is_web(url) => Some(target(url, &stream.request_headers)),
        // A copy that needs a torrent engine: listed, but unavailable.
        Some(url) if url.to_ascii_lowercase().starts_with("magnet:") => None,
        None if stream.info_hash.is_some() => None,
        _ => return Reading::Skip,
    };

    let mut lines = name
        .lines()
        .chain(description.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let label = lines
        .next()
        .map(String::from)
        .unwrap_or_else(|| addon_name.to_string());
    let rest: Vec<&str> = lines.collect();

    let text = format!(
        "{name}\n{description}\n{}",
        stream.filename.as_deref().unwrap_or("")
    );
    // The name carries the add-on's own reading of the quality; a release name
    // can say "4K Remastered" of a 1080p copy.
    let named = resolution(&name);
    let resolution = if named > 0 {
        named
    } else {
        self::resolution(&text)
    };

    let mut tags = dynamic_range(&text);
    tags.extend(release_kind(&text).map(String::from));
    tags.extend(technical_tags(&text));
    tags.dedup();
    tags.truncate(6);

    Reading::Copy(Box::new(AddonStream {
        addon_id: addon_id.to_string(),
        addon_name: addon_name.to_string(),
        name: label,
        description: (!rest.is_empty()).then(|| rest.join("\n")),
        quality: (resolution > 0).then(|| quality_label(resolution).to_string()),
        size_bytes: size(&text).or(stream.video_size.filter(|size| *size > 0)),
        tags,
        target,
    }))
}

fn is_web(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// `behaviorHints.proxyHeaders.request` become the headers every request for
/// the stream carries; a User-Agent among them becomes the user agent.
fn target(url: &str, headers: &[(String, String)]) -> StreamTarget {
    let user_agent = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
        .map(|(_, value)| value.clone());
    StreamTarget {
        url: url.to_string(),
        user_agent,
        headers: headers
            .iter()
            .filter(|(name, _)| !name.eq_ignore_ascii_case("user-agent"))
            .cloned()
            .collect(),
        is_live: false,
    }
}

static RESOLUTIONS: LazyLock<Vec<(Regex, i64)>> = LazyLock::new(|| {
    vec![
        (re(r"(?i)\b(2160p|4k|uhd)\b"), 2160),
        (re(r"(?i)\b1440p\b"), 1440),
        (re(r"(?i)\b(1080p|1080i|fhd)\b"), 1080),
        (re(r"(?i)\b720p\b"), 720),
        (re(r"(?i)\b(576p|480p|360p|sd|dvdrip)\b"), 480),
        (re(r"(?i)\b(cam|hdcam|ts|telesync|scr|screener)\b"), 240),
    ]
});

/// Vertical lines of the best quality named, 0 when none is.
pub(crate) fn resolution(text: &str) -> i64 {
    RESOLUTIONS
        .iter()
        .find(|(pattern, _)| pattern.is_match(text))
        .map_or(0, |(_, value)| *value)
}

pub(crate) fn quality_label(resolution: i64) -> &'static str {
    match resolution {
        2000.. => "4K",
        1400.. => "1440p",
        1000.. => "1080p",
        700.. => "720p",
        300.. => "SD",
        _ => "Cam",
    }
}

static DOLBY_VISION: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\b(DV|dovi|dolby[ .]?vision)\b"));
static HDR10_PLUS: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)HDR10\+|HDR10plus"));
static HDR: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bHDR(10)?\b"));

fn dynamic_range(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    if DOLBY_VISION.is_match(text) {
        found.push("Dolby Vision".to_string());
    }
    if HDR10_PLUS.is_match(text) {
        found.push("HDR10+".to_string());
    } else if HDR.is_match(text) {
        found.push("HDR".to_string());
    }
    found
}

static RELEASE_KINDS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (re(r"(?i)\bremux\b"), "Remux"),
        (re(r"(?i)\b(blu-?ray|bdrip|brrip)\b"), "Blu-ray"),
        (re(r"(?i)\bweb-?dl\b"), "WEB-DL"),
        (re(r"(?i)\bweb-?rip\b"), "WEBRip"),
        (re(r"(?i)\bhdtv\b"), "HDTV"),
        (re(r"(?i)\bdvd(rip)?\b"), "DVD"),
    ]
});

fn release_kind(text: &str) -> Option<&'static str> {
    RELEASE_KINDS
        .iter()
        .find(|(pattern, _)| pattern.is_match(text))
        .map(|(_, kind)| *kind)
}

static TECHNICAL: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (re(r"(?i)\b(x265|h\.?265|hevc)\b"), "HEVC"),
        (re(r"(?i)\bav1\b"), "AV1"),
        (re(r"(?i)\b(x264|h\.?264|avc)\b"), "H.264"),
        (re(r"(?i)\batmos\b"), "Atmos"),
        (re(r"(?i)\btrue-?hd\b"), "TrueHD"),
        (re(r"(?i)\bdts-?hd\b|\bdts-?x\b"), "DTS-HD"),
        (re(r"(?i)\bdts\b"), "DTS"),
        (re(r"(?i)\b(ddp|dd\+|e-?ac-?3)"), "DD+"),
        (
            re(r"(?i)\b(ac-?3|dd)[ .]?5\.?1\b|\bac-?3\b"),
            "Dolby Digital",
        ),
        (re(r"\b7\.1\b"), "7.1"),
        (re(r"\b5\.1\b"), "5.1"),
        (re(r"(?i)\b10-?bit\b"), "10-bit"),
    ]
});

fn technical_tags(text: &str) -> Vec<String> {
    let mut found: Vec<&str> = Vec::new();
    for (pattern, tag) in TECHNICAL.iter() {
        if !pattern.is_match(text) {
            continue;
        }
        let covered = match *tag {
            "DTS" => found.contains(&"DTS-HD"),
            "Dolby Digital" => found.contains(&"DD+"),
            "5.1" => found.contains(&"7.1"),
            _ => false,
        };
        if !covered {
            found.push(tag);
        }
    }
    found.into_iter().take(5).map(String::from).collect()
}

static SIZE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)(\d+(?:[.,]\d+)?)\s*(TiB|GiB|MiB|KiB|TB|GB|MB|KB)\b"));

/// A size such as "💾 21.3 GB" in bytes. Add-ons count in 1024s. A size after
/// the 💾 marker wins over one elsewhere in the text.
pub(crate) fn size(text: &str) -> Option<i64> {
    let after_marker = text.split_once('💾').map(|(_, rest)| rest);
    let captures = after_marker
        .and_then(|rest| SIZE.captures(rest))
        .or_else(|| SIZE.captures(text))?;
    let value: f64 = captures[1].replace(',', ".").parse().ok()?;
    let power = match captures[2].to_ascii_uppercase().chars().next()? {
        'K' => 1,
        'M' => 2,
        'G' => 3,
        'T' => 4,
        _ => 0,
    };
    Some((value * 1024f64.powi(power)) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(name: &str, description: &str) -> Stream {
        Stream {
            url: Some("https://cdn.example/file.mkv".into()),
            name: Some(name.into()),
            description: Some(description.into()),
            ..Stream::default()
        }
    }

    fn copy(reading: Reading) -> AddonStream {
        match reading {
            Reading::Copy(copy) => *copy,
            _ => panic!("expected a copy"),
        }
    }

    #[test]
    fn reads_quality_size_and_tags() {
        let copy = copy(read(
            &stream(
                "[XX+] Example\n4k HDR | DV",
                "Movie.2023.2160p.WEB-DL.DDP5.1.Atmos.x265\n👤 34 💾 21.3 GB ⚙️ Site",
            ),
            "org.example",
            "Example",
        ));
        assert_eq!(copy.name, "[XX+] Example");
        assert_eq!(copy.quality.as_deref(), Some("4K"));
        assert_eq!(copy.size_bytes, Some((21.3 * 1024f64.powi(3)) as i64));
        assert_eq!(
            copy.tags,
            vec!["Dolby Vision", "HDR", "WEB-DL", "HEVC", "Atmos", "DD+"]
        );
        assert!(
            copy.description
                .unwrap()
                .starts_with("4k HDR | DV\nMovie.2023")
        );
        assert!(copy.target.is_some());
    }

    #[test]
    fn name_quality_beats_release_name() {
        let copy = copy(read(
            &stream("Example 1080p", "Film.4K.Remastered.1080p"),
            "a",
            "A",
        ));
        assert_eq!(copy.quality.as_deref(), Some("1080p"));
    }

    #[test]
    fn errors_become_notices() {
        match read(
            &stream("[Error] Example", "Invalid key.\nCheck settings"),
            "a",
            "A",
        ) {
            Reading::Notice(text) => assert_eq!(text, "Invalid key. Check settings"),
            _ => panic!("expected a notice"),
        }
    }

    #[test]
    fn torrents_have_no_target_and_youtube_is_skipped() {
        let torrent = Stream {
            info_hash: Some("abc".into()),
            name: Some("T 720p".into()),
            ..Stream::default()
        };
        let listed = copy(read(&torrent, "a", "A"));
        assert!(listed.target.is_none());
        assert_eq!(listed.quality.as_deref(), Some("720p"));
        let youtube = Stream {
            name: Some("Trailer".into()),
            ..Stream::default()
        };
        assert!(matches!(read(&youtube, "a", "A"), Reading::Skip));
    }

    #[test]
    fn proxy_headers_become_target_headers() {
        let mut with_headers = stream("A", "");
        with_headers.request_headers = vec![
            ("User-Agent".into(), "Player/1".into()),
            ("Referer".into(), "https://r".into()),
        ];
        let target = copy(read(&with_headers, "a", "A")).target.unwrap();
        assert_eq!(target.user_agent.as_deref(), Some("Player/1"));
        assert_eq!(
            target.headers,
            vec![("Referer".to_string(), "https://r".to_string())]
        );
    }

    #[test]
    fn sizes() {
        assert_eq!(size("Size: 700 MB"), Some(700 * 1024 * 1024));
        assert_eq!(size("1.5 GB then 💾 2 GiB"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(size("no size"), None);
    }
}
