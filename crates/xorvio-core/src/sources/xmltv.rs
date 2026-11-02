//! Streaming XMLTV parser. Programmes are handed over in batches, so a large
//! guide never has to be held in memory, and only programmes inside the
//! window are kept.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use flate2::read::MultiGzDecoder;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, BytesText, Event};
use xorvio_model::{Program, ProgramFlags, Timestamp};

use crate::error::SourceError;
use crate::names;

/// A guide channel's id and one of its display names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelName {
    pub epg_id: String,
    pub display_name: String,
}

/// No real guide comes near this once decompressed; a "zip bomb" would.
const MAX_DECOMPRESSED_BYTES: u64 = 4 << 30;

pub(crate) struct XmltvParser<'a> {
    source_id: String,
    window: (Timestamp, Timestamp),
    /// Seconds added to every programme time.
    offset: i64,
    batch_size: usize,
    on_programs: &'a mut dyn FnMut(Vec<Program>) -> anyhow::Result<()>,
    on_channels: &'a mut dyn FnMut(Vec<ChannelName>) -> anyhow::Result<()>,
    pub program_count: usize,
}

/// What's known about the programme being read.
#[derive(Default)]
struct Current {
    channel: String,
    start: Timestamp,
    end: Timestamp,
    title: Option<String>,
    subtitle: Option<String>,
    summary: Option<String>,
    category: Option<String>,
    image_url: Option<String>,
    image_is_landscape: bool,
    episode: Option<String>,
    episode_is_onscreen: bool,
    flags: ProgramFlags,
    age_rating: Option<String>,
    star_rating: Option<f64>,
    year: Option<i64>,
}

impl<'a> XmltvParser<'a> {
    pub fn new(
        source_id: &str,
        window: (Timestamp, Timestamp),
        on_programs: &'a mut dyn FnMut(Vec<Program>) -> anyhow::Result<()>,
        on_channels: &'a mut dyn FnMut(Vec<ChannelName>) -> anyhow::Result<()>,
    ) -> Self {
        Self { source_id: source_id.into(), window, offset: 0, batch_size: 2000, on_programs, on_channels, program_count: 0 }
    }

    /// Moves every programme by `minutes`, for a guide written for the wrong
    /// time zone. The window applies to the moved times.
    pub fn shifted(mut self, minutes: i64) -> Self {
        self.offset = minutes * 60;
        self
    }

    /// Parses an XMLTV file, plain or gzip-compressed.
    pub fn parse_file(&mut self, path: &Path) -> anyhow::Result<()> {
        let file = std::fs::File::open(path).map_err(|_| SourceError::InvalidGuide("the file could not be opened".into()))?;
        let mut reader = BufReader::with_capacity(1 << 16, file);
        let is_gzip = reader.fill_buf().map(|start| start.starts_with(&[0x1f, 0x8b])).unwrap_or(false);
        if is_gzip {
            let decoder = Capped { inner: MultiGzDecoder::new(reader), remaining: MAX_DECOMPRESSED_BYTES };
            self.parse(BufReader::with_capacity(1 << 16, decoder))
        } else {
            self.parse(reader)
        }
    }

    pub fn parse(&mut self, input: impl BufRead) -> anyhow::Result<()> {
        let mut reader = Reader::from_reader(input);
        let config = reader.config_mut();
        config.check_end_names = false;
        config.trim_text(false);

        let mut buffer = Vec::new();
        let mut batch: Vec<Program> = Vec::new();
        let mut channels: Vec<ChannelName> = Vec::new();
        let mut text = String::new();
        let mut channel_id: Option<String> = None;
        let mut current: Option<Current> = None;
        // Context for elements whose meaning depends on their attributes or parent.
        let mut image_type: Option<String> = None;
        let mut episode_system: Option<String> = None;
        let mut value_parent: Option<&'static str> = None;
        let mut failure: Option<String> = None;
        let mut saw_root = false;

        loop {
            let event = match reader.read_event_into(&mut buffer) {
                Ok(event) => event,
                Err(error) => {
                    failure = Some(error.to_string());
                    break;
                }
            };
            match event {
                Event::Eof => break,
                Event::Start(element) => {
                    text.clear();
                    saw_root |= element.local_name().as_ref() == b"tv";
                    self.start(&reader, &element, &mut channel_id, &mut current, &mut image_type, &mut episode_system, &mut value_parent);
                }
                Event::Empty(element) => {
                    text.clear();
                    saw_root |= element.local_name().as_ref() == b"tv";
                    self.start(&reader, &element, &mut channel_id, &mut current, &mut image_type, &mut episode_system, &mut value_parent);
                    let name = element.local_name();
                    self.end(name.as_ref(), "", &mut channels, &mut channel_id, &mut current, &mut batch, &mut image_type, &mut episode_system, &mut value_parent)?;
                }
                Event::Text(content) => text.push_str(&unescaped(&content)),
                Event::CData(content) => text.push_str(&String::from_utf8_lossy(&content)),
                Event::End(element) => {
                    let value = text.trim().to_string();
                    text.clear();
                    let name = element.local_name();
                    self.end(name.as_ref(), &value, &mut channels, &mut channel_id, &mut current, &mut batch, &mut image_type, &mut episode_system, &mut value_parent)?;
                }
                _ => {}
            }
            buffer.clear();
        }
        if !batch.is_empty() {
            (self.on_programs)(std::mem::take(&mut batch))?;
        }
        let saw_channels = !channels.is_empty();
        if saw_channels {
            (self.on_channels)(channels)?;
        }
        // An error page or some other document instead of a guide.
        if !saw_root {
            let reason = failure.map_or_else(|| "it is not an XMLTV guide".to_string(), |reason| format!("it is not valid XMLTV ({reason})"));
            return Err(SourceError::InvalidGuide(reason).into());
        }
        // A guide cut off part-way still gives what it had; one with nothing usable is an error.
        if let Some(reason) = failure
            && self.program_count == 0
            && !saw_channels
        {
            return Err(SourceError::InvalidGuide(format!("it is not valid XMLTV ({reason})")).into());
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn start<R>(
        &self,
        reader: &Reader<R>,
        element: &BytesStart<'_>,
        channel_id: &mut Option<String>,
        current: &mut Option<Current>,
        image_type: &mut Option<String>,
        episode_system: &mut Option<String>,
        value_parent: &mut Option<&'static str>,
    ) {
        let attribute = |name: &[u8]| -> Option<String> {
            element.attributes().flatten().find(|attr| attr.key.local_name().as_ref() == name).map(|attr| {
                attr.decode_and_unescape_value(reader.decoder())
                    .map(|value| value.into_owned())
                    .unwrap_or_else(|_| String::from_utf8_lossy(&attr.value).into_owned())
            })
        };
        let in_programme = current.is_some();
        match element.local_name().as_ref() {
            b"channel" if !in_programme => *channel_id = attribute(b"id"),
            b"programme" => {
                *current = None;
                let (Some(channel), Some(start)) = (attribute(b"channel"), attribute(b"start").and_then(|s| parse_date(&s)))
                else {
                    return;
                };
                let end = attribute(b"stop").and_then(|s| parse_date(&s)).unwrap_or(start + 3600) + self.offset;
                let start = start + self.offset;
                if end > self.window.0 && start < self.window.1 && end > start {
                    *current = Some(Current { channel, start, end, ..Default::default() });
                }
            }
            // Classic programme artwork; newer <image> elements take precedence.
            b"icon" if in_programme => {
                if let Some(program) = current.as_mut()
                    && program.image_url.is_none()
                {
                    program.image_url = attribute(b"src").and_then(|src| http_url(&src));
                }
            }
            b"image" if in_programme => {
                *image_type = attribute(b"type").or_else(|| {
                    Some(if attribute(b"orient").as_deref() == Some("L") { "backdrop" } else { "poster" }.into())
                });
            }
            b"episode-num" if in_programme => *episode_system = attribute(b"system"),
            b"rating" if in_programme => *value_parent = Some("rating"),
            b"star-rating" if in_programme => *value_parent = Some("star-rating"),
            b"new" | b"premiere" | b"live" | b"previously-shown" if in_programme => {
                if let Some(program) = current.as_mut() {
                    match element.local_name().as_ref() {
                        b"new" => program.flags.is_new = true,
                        b"premiere" => program.flags.is_premiere = true,
                        b"live" => program.flags.is_live = true,
                        _ => program.flags.is_repeat = true,
                    }
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn end(
        &mut self,
        name: &[u8],
        value: &str,
        channels: &mut Vec<ChannelName>,
        channel_id: &mut Option<String>,
        current: &mut Option<Current>,
        batch: &mut Vec<Program>,
        image_type: &mut Option<String>,
        episode_system: &mut Option<String>,
        value_parent: &mut Option<&'static str>,
    ) -> anyhow::Result<()> {
        if name == b"display-name" {
            if let Some(id) = channel_id.as_ref().filter(|_| current.is_none() && !value.is_empty()) {
                channels.push(ChannelName { epg_id: id.clone(), display_name: value.to_string() });
            }
            return Ok(());
        }
        if name == b"channel" {
            *channel_id = None;
            return Ok(());
        }
        if name == b"rating" || name == b"star-rating" {
            *value_parent = None;
            return Ok(());
        }
        if name == b"programme" {
            if let Some(program) = current.take() {
                self.finish(program, batch)?;
            }
            return Ok(());
        }
        let Some(program) = current.as_mut() else { return Ok(()) };
        let non_empty = || (!value.is_empty()).then(|| value.to_string());
        match name {
            b"title" if program.title.is_none() => program.title = non_empty(),
            b"sub-title" if program.subtitle.is_none() => program.subtitle = non_empty(),
            // The first description goes with the first title; multi-language
            // guides list one per language.
            b"desc" if program.summary.is_none() => program.summary = non_empty(),
            b"category" if program.category.is_none() => program.category = non_empty(),
            b"image" => {
                // Landscape art (stills, backdrops) suits cards best; posters are a fallback.
                if let Some(link) = http_url(value) {
                    let landscape = matches!(image_type.as_deref(), Some("still" | "backdrop"));
                    if program.image_url.is_none() || (landscape && !program.image_is_landscape) {
                        program.image_url = Some(link);
                        program.image_is_landscape = landscape;
                    }
                }
                *image_type = None;
            }
            b"episode-num" => {
                match episode_system.as_deref() {
                    Some("onscreen") if !value.is_empty() => {
                        program.episode = Some(value.to_string());
                        program.episode_is_onscreen = true;
                    }
                    Some("xmltv_ns") if !program.episode_is_onscreen => {
                        if let Some(code) = episode_code(value) {
                            program.episode = Some(code);
                        }
                    }
                    _ => {}
                }
                *episode_system = None;
            }
            b"value" if *value_parent == Some("rating") => {
                if program.age_rating.is_none() && !value.is_empty() && value.chars().count() <= 8 {
                    program.age_rating = Some(value.to_string());
                }
            }
            b"value" if *value_parent == Some("star-rating") => {
                if program.star_rating.is_none() {
                    program.star_rating = star_rating(value);
                }
            }
            b"date" => program.year = value.get(..4).and_then(|year| year.parse().ok()),
            _ => {}
        }
        Ok(())
    }

    fn finish(&mut self, current: Current, batch: &mut Vec<Program>) -> anyhow::Result<()> {
        let (title, marked_live) = names::live_marker(current.title.as_deref().unwrap_or("Untitled"));
        let mut flags = current.flags;
        flags.is_live |= marked_live;
        batch.push(Program {
            id: Program::make_id(&self.source_id, &current.channel, current.start),
            source_id: self.source_id.clone(),
            epg_id: current.channel,
            start: current.start,
            end: current.end,
            title,
            subtitle: current.subtitle,
            summary: current.summary,
            category: current.category,
            image_url: current.image_url,
            episode: current.episode,
            flags,
            age_rating: current.age_rating,
            star_rating: current.star_rating,
            year: current.year,
            genre: None,
        });
        self.program_count += 1;
        if batch.len() >= self.batch_size {
            (self.on_programs)(std::mem::take(batch))?;
        }
        Ok(())
    }
}

/// Element text with entities resolved. Guides written by hand use HTML
/// entities XML doesn't define; those are kept as written rather than lost.
fn unescaped(content: &BytesText<'_>) -> String {
    content
        .unescape_with(|entity| match entity {
            _ if quick_xml::escape::resolve_predefined_entity(entity).is_some() => {
                quick_xml::escape::resolve_predefined_entity(entity)
            }
            "nbsp" => Some("\u{a0}"),
            "ndash" => Some("–"),
            "mdash" => Some("—"),
            "hellip" => Some("…"),
            _ => None,
        })
        .map(|text| text.into_owned())
        .unwrap_or_else(|_| String::from_utf8_lossy(content).into_owned())
}

/// Stops a decompressing reader once it has produced more than it should.
struct Capped<R> {
    inner: R,
    remaining: u64,
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        if count as u64 > self.remaining {
            return Err(std::io::Error::other("the guide is larger than 4 GB once decompressed"));
        }
        self.remaining -= count as u64;
        Ok(count)
    }
}

fn http_url(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let parsed = url::Url::parse(trimmed).ok()?;
    parsed.scheme().starts_with("http").then(|| trimmed.to_string())
}

/// xmltv_ns numbering is zero-based "season.episode.part" with optional
/// totals ("1/5"): "1.4.0/1" is season 2, episode 5.
pub(crate) fn episode_code(xmltv_ns: &str) -> Option<String> {
    let mut parts = xmltv_ns.split('.').map(|part| {
        // Numbers no real guide uses are dropped (they'd also overflow when made 1-based).
        part.split('/').next().and_then(|n| n.trim().parse::<i64>().ok()).filter(|n| (0..100_000).contains(n))
    });
    let season = parts.next().flatten();
    let episode = parts.next().flatten();
    match (season, episode) {
        (Some(s), Some(e)) => Some(format!("S{} E{}", s + 1, e + 1)),
        (None, Some(e)) => Some(format!("E{}", e + 1)),
        (Some(s), None) => Some(format!("S{}", s + 1)),
        (None, None) => None,
    }
}

/// "7.5/10" or "3/5" on a 10-point scale.
pub(crate) fn star_rating(text: &str) -> Option<f64> {
    let mut parts = text.split('/').map(|part| part.trim().parse::<f64>().ok());
    let value = parts.next().flatten()?;
    let scale = match parts.next() {
        Some(scale) => scale.unwrap_or(10.0),
        None => 10.0,
    };
    if !value.is_finite() || !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    Some((value / scale * 10.0).clamp(0.0, 10.0))
}

/// XMLTV times: "20260925143000 +0100" (the offset and the seconds are optional).
pub fn parse_date(text: &str) -> Option<Timestamp> {
    let bytes = text.as_bytes();
    let number = |from: usize, length: usize| -> Option<u32> {
        let digits = bytes.get(from..from + length)?;
        digits.iter().try_fold(0u32, |value, &c| c.is_ascii_digit().then(|| value * 10 + u32::from(c - b'0')))
    };
    let year = number(0, 4)?;
    let (month, day, hour, minute) = (number(4, 2)?, number(6, 2)?, number(8, 2)?, number(10, 2)?);
    let second = number(12, 2).unwrap_or(0);
    let mut offset: i64 = 0;
    if let Some(sign) = bytes.iter().position(|&c| c == b'+' || c == b'-').filter(|&index| index >= 12)
        && let (Some(hours), Some(minutes)) = (number(sign + 1, 2), number(sign + 3, 2))
    {
        offset = i64::from(hours * 3600 + minutes * 60) * if bytes[sign] == b'-' { -1 } else { 1 };
    }
    let date = chrono::NaiveDate::from_ymd_opt(year as i32, month, day)?;
    let time = date.and_hms_opt(hour, minute, second.min(59))?;
    Some(time.and_utc().timestamp() - offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn run(xml: &[u8], window: (Timestamp, Timestamp)) -> (Vec<Program>, Vec<ChannelName>, anyhow::Result<()>) {
        let mut programs = Vec::new();
        let mut names = Vec::new();
        let mut on_programs = |batch: Vec<Program>| {
            programs.extend(batch);
            Ok(())
        };
        let mut on_channels = |batch: Vec<ChannelName>| {
            names.extend(batch);
            Ok(())
        };
        let result = {
            let mut parser = XmltvParser::new("s", window, &mut on_programs, &mut on_channels);
            parser.batch_size = 2;
            parser.parse(xml)
        };
        (programs, names, result)
    }

    const EVERYTHING: (Timestamp, Timestamp) = (i64::MIN / 2, i64::MAX / 2);

    #[test]
    fn parses_dates_with_offsets() {
        let utc = parse_date("20260925143000 +0000");
        assert!(utc.is_some());
        assert_eq!(utc, parse_date("20260925153000 +0100"));
        assert_eq!(utc, parse_date("20260925093000 -0500"));
        assert_eq!(utc, parse_date("202609251430"));
        assert_eq!(utc, parse_date("202609251430 +0000"));
        assert_eq!(parse_date("garbage"), None);
        assert_eq!(parse_date("20261325143000"), None, "month 13");
        assert_eq!(parse_date("2026"), None);
    }

    #[test]
    fn streams_programmes_inside_the_window_and_channel_names() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE tv SYSTEM "xmltv.dtd">
<tv>
  <channel id="one.uk"><display-name>Channel One</display-name><display-name>C1</display-name><icon src="https://img.example/c1.png"/></channel>
  <programme channel="one.uk" start="20260925100000 +0000" stop="20260925110000 +0000">
    <title lang="en">Live: Morning &amp; More</title><title lang="fr">Matin</title>
    <sub-title>Part 1</sub-title>
    <desc lang="en">First</desc><desc lang="fr">Premier</desc>
    <category>News</category><category>Talk</category>
    <icon src="https://img.example/icon.jpg"/>
    <image type="poster">https://img.example/poster.jpg</image>
    <image type="backdrop">https://img.example/backdrop.jpg</image>
    <episode-num system="xmltv_ns">1.4.0/1</episode-num>
    <rating system="MPAA"><value>PG-13</value></rating>
    <star-rating><value>3/5</value></star-rating>
    <date>2019-05-01</date>
    <new/><previously-shown start="20200101000000"/>
  </programme>
  <programme channel="one.uk" start="20260925110000 +0000">
    <title>No stop</title>
    <episode-num system="onscreen">S2 E6</episode-num>
    <episode-num system="xmltv_ns">0.0.</episode-num>
  </programme>
  <programme channel="one.uk" start="20270101100000 +0000" stop="20270101110000 +0000"><title>Far future</title></programme>
  <programme channel="one.uk" start="nonsense"><title>Broken</title></programme>
  <programme channel="one.uk" start="20260925120000 +0000" stop="20260925110000 +0000"><title>Backwards</title></programme>
</tv>"#;
        let window = (parse_date("20260924000000 +0000").unwrap(), parse_date("20260930000000 +0000").unwrap());
        let (programs, names, result) = run(xml, window);
        result.unwrap();
        assert_eq!(programs.len(), 2);
        let first = &programs[0];
        assert_eq!(first.title, "Morning & More");
        assert!(first.flags.is_live && first.flags.is_new && first.flags.is_repeat && !first.flags.is_premiere);
        assert_eq!(first.subtitle.as_deref(), Some("Part 1"));
        assert_eq!(first.summary.as_deref(), Some("First"));
        assert_eq!(first.category.as_deref(), Some("News"));
        assert_eq!(first.image_url.as_deref(), Some("https://img.example/backdrop.jpg"));
        assert_eq!(first.episode.as_deref(), Some("S2 E5"));
        assert_eq!(first.age_rating.as_deref(), Some("PG-13"));
        assert_eq!(first.star_rating, Some(6.0));
        assert_eq!(first.year, Some(2019));
        assert_eq!(first.id, format!("s|one.uk|{}", first.start));
        let second = &programs[1];
        assert_eq!(second.end - second.start, 3600, "a missing stop means an hour");
        assert_eq!(second.episode.as_deref(), Some("S2 E6"), "on-screen numbering wins");
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], ChannelName { epg_id: "one.uk".into(), display_name: "Channel One".into() });
    }

    #[test]
    fn an_offset_moves_every_programme() {
        let xml = br#"<tv>
  <programme channel="a" start="20260925100000 +0000" stop="20260925110000 +0000"><title>Ten</title></programme>
  <programme channel="a" start="20260925230000 +0000" stop="20260926000000 +0000"><title>Eleven</title></programme>
</tv>"#;
        let ten = parse_date("20260925100000 +0000").unwrap();
        let mut programs = Vec::new();
        let mut on_programs = |batch: Vec<Program>| {
            programs.extend(batch);
            Ok(())
        };
        let mut on_channels = |_: Vec<ChannelName>| Ok(());
        // The window applies to the moved times: "Eleven" moves past its end.
        let window = (ten, parse_date("20260926000000 +0000").unwrap());
        XmltvParser::new("s", window, &mut on_programs, &mut on_channels).shifted(90).parse(&xml[..]).unwrap();
        assert_eq!(programs.len(), 1);
        assert_eq!((programs[0].start, programs[0].end), (ten + 5400, ten + 9000));
        assert_eq!(programs[0].id, format!("s|a|{}", ten + 5400), "the id carries the moved start");
    }

    #[test]
    fn reads_gzip_compressed_guides_from_disk() {
        let xml = r#"<tv><programme channel="a" start="20260925100000 +0000" stop="20260925110000 +0000"><title>Zipped</title></programme></tv>"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("guide.xml.gz");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(xml.as_bytes()).unwrap();
        std::fs::write(&path, encoder.finish().unwrap()).unwrap();

        let mut titles = Vec::new();
        let mut on_programs = |batch: Vec<Program>| {
            titles.extend(batch.into_iter().map(|p| p.title));
            Ok(())
        };
        let mut on_channels = |_: Vec<ChannelName>| Ok(());
        XmltvParser::new("s", EVERYTHING, &mut on_programs, &mut on_channels).parse_file(&path).unwrap();
        assert_eq!(titles, ["Zipped"]);

        let plain = dir.path().join("guide.xml");
        std::fs::write(&plain, xml).unwrap();
        let mut count = 0;
        let mut on_programs = |batch: Vec<Program>| {
            count += batch.len();
            Ok(())
        };
        let mut on_channels = |_: Vec<ChannelName>| Ok(());
        XmltvParser::new("s", EVERYTHING, &mut on_programs, &mut on_channels).parse_file(&plain).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn latin1_guides_and_html_entities() {
        let xml = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><tv><programme channel=\"a\" start=\"20260925100000\"><title>Caf\xe9&nbsp;Society</title><desc>A &bogus; entity</desc></programme></tv>";
        let (programs, _, result) = run(xml, EVERYTHING);
        result.unwrap();
        assert_eq!(programs[0].title, "Café\u{a0}Society");
        assert!(programs[0].summary.as_deref().is_some_and(|s| s.contains("entity")));
    }

    #[test]
    fn broken_guides() {
        // An error page instead of a guide must not replace the last good one.
        let (_, _, result) = run(b"<html><body>Not found", EVERYTHING);
        assert!(result.is_err());
        let (programs, _, result) = run(b"not xml at all <<<>>>", EVERYTHING);
        assert!(programs.is_empty() && result.is_err());
        let (_, _, result) = run(b"<tv></tv>", EVERYTHING);
        assert!(result.is_ok(), "an empty guide is still a guide");
        // A guide cut off part-way keeps what it had.
        let cut = br#"<tv><programme channel="a" start="20260925100000"><title>Kept</title></programme><programme channel="a" start="2026"#;
        let (programs, _, result) = run(cut, EVERYTHING);
        assert!(result.is_ok());
        assert_eq!(programs.len(), 1);
    }

    #[test]
    fn absurd_numbers_dont_overflow() {
        assert_eq!(episode_code("9223372036854775807.9223372036854775807."), None);
        assert_eq!(episode_code("1.4.0/1"), Some("S2 E5".into()));
        assert_eq!(episode_code(".4."), Some("E5".into()));
        assert_eq!(episode_code("2.."), Some("S3".into()));
        assert_eq!(star_rating("7.5/10"), Some(7.5));
        assert_eq!(star_rating("3/0"), None);
        assert_eq!(star_rating("nan/10"), None);
        assert_eq!(star_rating("12"), Some(10.0));
    }
}
