//! Subtitle files from subtitle add-ons (OpenSubtitles and the like) and
//! beside the videos in media folders, made into WebVTT for the player.
//! They're mostly SRT, often in Windows-1252 rather than UTF-8; files beside
//! videos are sometimes SubStation Alpha (.ass, .ssa).

/// WebVTT from an SRT, WebVTT or SubStation Alpha file's bytes.
pub fn to_webvtt(bytes: &[u8]) -> String {
    let text = decode(bytes);
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    if text.trim_start().starts_with("WEBVTT") {
        return text;
    }
    if is_substation(&text) {
        return substation_to_webvtt(&text);
    }
    let mut out = String::with_capacity(text.len() + 16);
    out.push_str("WEBVTT\n\n");
    for line in text.lines() {
        // SRT writes "00:01:02,500"; WebVTT "00:01:02.500". Cue numbers are
        // valid WebVTT cue identifiers, so they stay.
        if line.contains("-->") {
            out.push_str(&line.replace(',', "."));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// UTF-8 (with or without a byte-order mark), UTF-16 with one, or else
/// Windows-1252, which most non-UTF-8 subtitles are.
fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| if le { u16::from_le_bytes(pair) } else { u16::from_be_bytes(pair) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        return utf16(rest, false);
    }
    bytes.iter().map(|&b| windows_1252(b)).collect()
}

fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}',
        '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9F => HIGH[(byte - 0x80) as usize],
        _ => byte as char,
    }
}

/// SubStation Alpha starts with a [Script Info] section and keeps its lines in [Events].
fn is_substation(text: &str) -> bool {
    text.trim_start().starts_with("[Script Info]") || text.lines().any(|line| line.trim().eq_ignore_ascii_case("[events]"))
}

/// A SubStation Alpha file's dialogue as WebVTT cues, in time order.
/// Styling and placement go, except italics and bold; line breaks stay.
/// Drawings (shapes some typesetters make) aren't words, so they're left out.
fn substation_to_webvtt(text: &str) -> String {
    let mut in_events = false;
    let mut format: Vec<String> = Vec::new();
    let mut cues: Vec<(f64, f64, String)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_events = line.eq_ignore_ascii_case("[events]");
            continue;
        }
        if !in_events {
            continue;
        }
        if let Some(fields) = line.strip_prefix("Format:") {
            format = fields.split(',').map(|field| field.trim().to_ascii_lowercase()).collect();
            continue;
        }
        let Some(fields) = line.strip_prefix("Dialogue:") else { continue };
        // The text is last and may hold commas, so the line splits into only as many fields as there are.
        let columns = if format.is_empty() { 10 } else { format.len() };
        let values: Vec<&str> = fields.splitn(columns, ',').collect();
        let field = |name: &str, fallback: usize| values.get(format.iter().position(|known| known == name).unwrap_or(fallback)).copied();
        let (Some(start), Some(end), Some(words)) =
            (field("start", 1).and_then(substation_time), field("end", 2).and_then(substation_time), field("text", 9))
        else {
            continue;
        };
        let words = substation_words(words);
        if end > start && !words.is_empty() {
            cues.push((start, end, words));
        }
    }
    cues.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = String::from("WEBVTT\n\n");
    for (start, end, words) in cues {
        out.push_str(&format!("{} --> {}\n{words}\n\n", webvtt_time(start), webvtt_time(end)));
    }
    out
}

/// "0:01:02.50" (hours, minutes, seconds with hundredths) in seconds.
fn substation_time(text: &str) -> Option<f64> {
    let mut parts = text.trim().split(':').map(|part| part.parse::<f64>().ok());
    let (hours, minutes, seconds) = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(hours * 3600.0 + minutes * 60.0 + seconds)
}

fn webvtt_time(seconds: f64) -> String {
    let millis = (seconds.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}.{:03}", millis / 3_600_000, millis / 60_000 % 60, millis / 1000 % 60, millis % 1000)
}

/// A line's words without its override tags (`{\pos(10,20)}`), with `\N`
/// as a line break, `\h` as a space, and italics and bold as WebVTT's.
fn substation_words(raw: &str) -> String {
    let escape = |text: &str| text.replace('&', "&amp;").replace('<', "&lt;");
    let mut out = String::new();
    let mut rest = raw;
    while let Some(open) = rest.find('{') {
        out.push_str(&escape(&rest[..open]));
        let Some(length) = rest[open..].find('}') else {
            rest = "";
            break;
        };
        let tags = &rest[open + 1..open + length];
        if tags.split('\\').any(|tag| tag.len() == 2 && tag.starts_with('p') && tag.as_bytes()[1].is_ascii_digit() && tag != "p0") {
            return String::new();
        }
        for tag in tags.split('\\') {
            match tag {
                "i1" => out.push_str("<i>"),
                "i0" => out.push_str("</i>"),
                "b1" => out.push_str("<b>"),
                "b0" => out.push_str("</b>"),
                _ => {}
            }
        }
        rest = &rest[open + length + 1..];
    }
    out.push_str(&escape(rest));
    let out = out.replace("\\N", "\n").replace("\\n", "\n").replace("\\h", " ");
    out.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substation_alpha_becomes_webvtt() {
        let ass = "[Script Info]\r\nTitle: Test\r\n\r\n[V4+ Styles]\r\nFormat: Name, Fontname\r\nStyle: Default,Arial\r\n\r\n[Events]\r\n\
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\r\n\
Dialogue: 0,0:00:05.00,0:00:06.50,Default,,0,0,0,,Second, with a comma\r\n\
Dialogue: 0,0:00:01.20,0:00:03.00,Default,,0,0,0,,{\\pos(10,20)}{\\i1}Hello{\\i0}\\NWorld & <you>\r\n\
Dialogue: 0,0:00:02.00,0:00:04.00,Default,,0,0,0,,{\\p1}m 0 0 l 100 0 100 100{\\p0}\r\n\
Comment: 0,0:00:02.00,0:00:04.00,Default,,0,0,0,,Not shown\r\n";
        assert_eq!(
            to_webvtt(ass.as_bytes()),
            "WEBVTT\n\n00:00:01.200 --> 00:00:03.000\n<i>Hello</i>\nWorld &amp; &lt;you>\n\n00:00:05.000 --> 00:00:06.500\nSecond, with a comma\n\n"
        );
    }

    #[test]
    fn srt_becomes_webvtt() {
        let srt = b"1\r\n00:00:01,000 --> 00:00:03,500\r\nCaf\xe9 \x93ol\xe9\x94\r\n\r\n2\r\n00:00:05,000 --> 00:00:06,000\r\nBye\r\n";
        let vtt = to_webvtt(srt);
        assert!(vtt.starts_with("WEBVTT\n\n1\n00:00:01.000 --> 00:00:03.500\nCafé “olé”\n"));
        assert!(vtt.contains("2\n00:00:05.000 --> 00:00:06.000\nBye\n"));
        assert_eq!(to_webvtt("WEBVTT\n\n00:01.000 --> 00:02.000\nHi\n".as_bytes()), "WEBVTT\n\n00:01.000 --> 00:02.000\nHi\n");
    }
}
