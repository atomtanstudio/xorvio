//! Rewriting HLS playlists so every address in them (variants, renditions,
//! segments, keys, init sections) comes back through the gateway, where the
//! provider's headers are added and CORS is handled.

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

/// What a playlist address points at, which decides how the gateway serves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildKind {
    /// Another playlist, rewritten in turn.
    Playlist,
    /// Bytes passed through: a segment, a key, an init section.
    Media,
}

static URI_ATTRIBUTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"URI="([^"]*)""#).expect("valid pattern"));

/// Tags whose `URI` attribute names another playlist.
const PLAYLIST_TAGS: [&str; 3] = [
    "#EXT-X-MEDIA:",
    "#EXT-X-I-FRAME-STREAM-INF:",
    "#EXT-X-RENDITION-REPORT:",
];

/// The playlist with every address resolved against `base` (the playlist's
/// own address after redirects) and replaced by what `register` returns for
/// it. Addresses that aren't http(s) (FairPlay `skd://`, `data:`) stay as they are.
pub(crate) fn rewrite(
    text: &str,
    base: &Url,
    mut register: impl FnMut(Url, ChildKind) -> String,
) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 2);
    let mut next_is_variant = false;
    let mut resolve = |reference: &str, kind: ChildKind| -> Option<String> {
        let url = base.join(reference.trim()).ok()?;
        matches!(url.scheme(), "http" | "https").then(|| register(url, kind))
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push_str(line);
        } else if trimmed.starts_with('#') {
            if trimmed.starts_with("#EXT-X-STREAM-INF") {
                next_is_variant = true;
            }
            let kind = if PLAYLIST_TAGS.iter().any(|tag| trimmed.starts_with(tag)) {
                ChildKind::Playlist
            } else {
                ChildKind::Media
            };
            let rewritten =
                URI_ATTRIBUTE.replace_all(line, |caps: &regex::Captures| {
                    match resolve(&caps[1], kind) {
                        Some(address) => format!(r#"URI="{address}""#),
                        None => caps[0].to_string(),
                    }
                });
            out.push_str(&rewritten);
        } else {
            let kind = if next_is_variant {
                ChildKind::Playlist
            } else {
                ChildKind::Media
            };
            next_is_variant = false;
            match resolve(trimmed, kind) {
                Some(address) => out.push_str(&address),
                None => out.push_str(line),
            }
        }
        out.push('\n');
    }
    out
}

/// Whether a response body is a playlist rather than media.
pub(crate) fn looks_like_playlist(body: &[u8]) -> bool {
    let start = body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body);
    let start = start.trim_ascii_start();
    start.starts_with(b"#EXTM3U")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewritten(text: &str) -> (String, Vec<(String, ChildKind)>) {
        let base = Url::parse("https://cdn.example/live/chan/index.m3u8?token=abc").unwrap();
        let mut seen = Vec::new();
        let out = rewrite(text, &base, |url, kind| {
            seen.push((url.to_string(), kind));
            format!("http://gw/{}", seen.len())
        });
        (out, seen)
    }

    #[test]
    fn rewrites_master_playlists() {
        let (out, seen) = rewritten(
            "#EXTM3U\r\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"en\",URI=\"audio/en.m3u8\"\r\n#EXT-X-STREAM-INF:BANDWIDTH=1000\r\nhi/index.m3u8\r\n#EXT-X-STREAM-INF:BANDWIDTH=500\r\nhttps://other.example/lo.m3u8?sig=1\r\n#EXT-X-I-FRAME-STREAM-INF:URI=\"iframes.m3u8\"\r\n",
        );
        assert_eq!(
            seen,
            [
                (
                    "https://cdn.example/live/chan/audio/en.m3u8".to_string(),
                    ChildKind::Playlist
                ),
                (
                    "https://cdn.example/live/chan/hi/index.m3u8".to_string(),
                    ChildKind::Playlist
                ),
                (
                    "https://other.example/lo.m3u8?sig=1".to_string(),
                    ChildKind::Playlist
                ),
                (
                    "https://cdn.example/live/chan/iframes.m3u8".to_string(),
                    ChildKind::Playlist
                ),
            ]
        );
        assert!(
            out.contains("URI=\"http://gw/1\"")
                && out.contains("\nhttp://gw/2\n")
                && out.contains("\nhttp://gw/3\n")
        );
        assert!(!out.contains("cdn.example") && !out.contains("other.example"));
    }

    #[test]
    fn rewrites_media_playlists() {
        let (out, seen) = rewritten(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-KEY:METHOD=AES-128,URI=\"/keys/1.key\",IV=0x1\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4.0,\nseg1.ts\n#EXTINF:4.0,\n../other/seg2.ts\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://key\"\n#EXT-X-ENDLIST\n",
        );
        let urls: Vec<&str> = seen.iter().map(|(u, _)| u.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://cdn.example/keys/1.key",
                "https://cdn.example/live/chan/init.mp4",
                "https://cdn.example/live/chan/seg1.ts",
                "https://cdn.example/live/other/seg2.ts",
            ]
        );
        assert!(seen.iter().all(|(_, kind)| *kind == ChildKind::Media));
        assert!(out.contains("METHOD=AES-128,URI=\"http://gw/1\",IV=0x1"));
        assert!(out.contains("URI=\"skd://key\""));
        assert!(out.ends_with("#EXT-X-ENDLIST\n"));
    }

    #[test]
    fn spots_playlists() {
        assert!(looks_like_playlist(b"\xEF\xBB\xBF #EXTM3U\n"));
        assert!(!looks_like_playlist(&[0x47, 0x40, 0x00]));
    }
}
