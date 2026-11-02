//! M3U playlists: parsing, and turning entries into channels, films and series.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use xorvio_model::{Category, CategoryKind, Channel, Episode, Movie, Series, StreamLocator};

use super::ids;
use crate::error::SourceError;
use crate::json::web_url;
use crate::names;
use crate::snapshot::{CatalogSnapshot, StreamOptions};

/// One `#EXTINF` entry exactly as the playlist describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct M3uEntry {
    pub title: String,
    /// Attribute names are lowercased.
    pub attributes: HashMap<String, String>,
    pub url: String,
    pub group: Option<String>,
    /// From `#EXTVLCOPT:http-user-agent=…` or a `user-agent` attribute.
    pub user_agent: Option<String>,
    /// From `#EXTVLCOPT:http-referrer=…`.
    pub referrer: Option<String>,
}

impl M3uEntry {
    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(String::as_str)
    }

    pub fn tvg_id(&self) -> Option<&str> {
        self.attribute("tvg-id").filter(|id| !id.trim().is_empty())
    }

    pub fn logo(&self) -> Option<String> {
        self.attribute("tvg-logo").and_then(web_url)
    }

    pub fn number(&self) -> Option<i64> {
        self.attribute("tvg-chno").or_else(|| self.attribute("channel-number")).and_then(|n| n.trim().parse().ok())
    }

    /// The replay window in days, when the entry advertises replay at all.
    pub fn catchup_days(&self) -> i64 {
        let advertised = ["catchup", "catchup-source", "tvg-rec"].iter().any(|name| self.attributes.contains_key(*name));
        if !advertised {
            return 0;
        }
        self.attribute("catchup-days")
            .or_else(|| self.attribute("tvg-rec"))
            .and_then(|days| days.trim().parse::<i64>().ok())
            .map_or(0, |days| days.clamp(0, 365))
    }

    /// The catch-up address template, or the standard one a `catchup` mode implies.
    pub fn catchup_template(&self) -> Option<String> {
        if let Some(source) = self.attribute("catchup-source").filter(|source| !source.is_empty()) {
            return Some(source.to_string());
        }
        match self.attribute("catchup").map(str::to_lowercase).as_deref() {
            Some("shift" | "append" | "default") => Some("?utc={utc}&lutc={lutc}".into()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct M3uPlaylist {
    pub entries: Vec<M3uEntry>,
    /// The guide named in the header (`url-tvg`).
    pub epg_url: Option<String>,
}

/// Playlists are untrusted: stream links may only use streaming protocols,
/// never `file://` or other schemes that would reach into this computer.
const STREAM_SCHEMES: &[&str] = &["http", "https", "rtmp", "rtmps", "rtsp", "rtsps", "rtp", "udp", "mms", "mmsh", "srt"];

fn stream_url(line: &str) -> Option<String> {
    let parsed = url::Url::parse(line).ok()?;
    STREAM_SCHEMES.contains(&parsed.scheme()).then(|| line.to_string())
}

/// Text from playlist bytes: UTF-8, or Latin-1 when it isn't.
fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&byte| byte as char).collect(),
    }
}

pub fn parse_bytes(bytes: &[u8]) -> Result<M3uPlaylist, SourceError> {
    parse(&decode(bytes))
}

#[derive(Default)]
struct Pending {
    title: Option<String>,
    attributes: HashMap<String, String>,
    group: Option<String>,
    user_agent: Option<String>,
    referrer: Option<String>,
}

pub fn parse(text: &str) -> Result<M3uPlaylist, SourceError> {
    let mut entries = Vec::new();
    let mut epg_url = None;
    let mut pending = Pending::default();
    let mut saw_header = false;

    for raw_line in text.trim_start_matches('\u{feff}').lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTM3U") {
            saw_header = true;
            let attributes = parse_attributes(rest);
            if let Some(guide) = ["url-tvg", "x-tvg-url", "tvg-url"].iter().find_map(|name| attributes.get(*name)) {
                // Some playlists list several guides separated by commas; the first is used.
                epg_url = guide.split(',').next().and_then(web_url);
            }
        } else if let Some(body) = line.strip_prefix("#EXTINF:") {
            let (attributes, title) = split_info(body);
            pending.group = attributes.get("group-title").cloned();
            if let Some(agent) = attributes.get("user-agent").or_else(|| attributes.get("http-user-agent")) {
                pending.user_agent = Some(agent.clone());
            }
            if let Some(referrer) = attributes.get("referrer").or_else(|| attributes.get("http-referrer")) {
                pending.referrer = Some(referrer.clone());
            }
            pending.attributes = attributes;
            pending.title = Some(title);
        } else if let Some(group) = line.strip_prefix("#EXTGRP:") {
            pending.group = Some(group.trim().to_string());
        } else if let Some(option) = line.strip_prefix("#EXTVLCOPT:") {
            // VLC options can come before or after #EXTINF; they apply to the next link.
            if let Some((name, value)) = option.split_once('=') {
                let value = value.trim().trim_matches('"').to_string();
                match name.trim().to_lowercase().as_str() {
                    "http-user-agent" => pending.user_agent = Some(value),
                    "http-referrer" | "http-referer" => pending.referrer = Some(value),
                    _ => {}
                }
            }
        } else if line.starts_with('#') {
            continue;
        } else if let (Some(_), Some(url)) = (&pending.title, stream_url(line)) {
            let done = std::mem::take(&mut pending);
            entries.push(M3uEntry {
                title: done.title.unwrap_or_default(),
                attributes: done.attributes,
                url,
                group: done.group.filter(|group| !group.is_empty()),
                user_agent: done.user_agent.filter(|agent| !agent.is_empty()),
                referrer: done.referrer.filter(|referrer| !referrer.is_empty()),
            });
        }
    }

    if entries.is_empty() {
        let reason = if saw_header { "it contains no playable entries" } else { "it is not an M3U playlist" };
        return Err(SourceError::InvalidPlaylist(reason.into()));
    }
    Ok(M3uPlaylist { entries, epg_url })
}

/// Splits `-1 tvg-id="x" group-title="A, B",Display Name` into attributes and
/// title. The title follows the first comma outside quotes.
fn split_info(body: &str) -> (HashMap<String, String>, String) {
    let mut in_quotes = false;
    let split = body.char_indices().find_map(|(index, c)| {
        if c == '"' {
            in_quotes = !in_quotes;
        }
        (c == ',' && !in_quotes).then_some(index)
    });
    let (attributes, title) = match split {
        Some(split) => (parse_attributes(&body[..split]), body[split + 1..].trim()),
        None => (parse_attributes(body), body.trim()),
    };
    let title = if title.is_empty() {
        attributes.get("tvg-name").cloned().unwrap_or_else(|| "Untitled".into())
    } else {
        title.to_string()
    };
    (attributes, title)
}

/// `key="value" key2=value2` pairs; names are lowercased.
fn parse_attributes(text: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let mut rest = text;
    while let Some(equals) = rest.find('=') {
        let key = rest[..equals].split(' ').next_back().unwrap_or_default().trim().to_lowercase();
        let after = &rest[equals + 1..];
        let (value, remainder) = if let Some(quoted) = after.strip_prefix('"') {
            match quoted.find('"') {
                Some(close) => (&quoted[..close], &quoted[close + 1..]),
                None => (quoted, ""),
            }
        } else {
            let end = after.find(' ').unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        if !key.is_empty() {
            result.insert(key, value.to_string());
        }
        rest = remainder;
    }
    result
}

enum Classification {
    Live,
    Movie,
    Episode { series: String, season: i64, number: i64 },
}

static ON_DEMAND_GROUP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(vod|movies?|films?)\b").expect("built-in pattern"));

const FINITE_EXTENSIONS: &[&str] = &["mp4", "mkv", "avi", "mov", "m4v", "wmv", "webm"];

fn classify(entry: &M3uEntry) -> Classification {
    let (path, extension) = match url::Url::parse(&entry.url) {
        Ok(parsed) => {
            let path = parsed.path().to_lowercase();
            let last = path.rsplit('/').next().unwrap_or_default();
            let extension = last.rsplit_once('.').map(|(_, ext)| ext.to_string()).unwrap_or_default();
            (path, extension)
        }
        Err(_) => (String::new(), String::new()),
    };
    let looks_series = path.contains("/series/");
    let looks_movie = path.contains("/movie/") || path.contains("/movies/");

    if looks_series || looks_movie || FINITE_EXTENSIONS.contains(&extension.as_str()) {
        if let Some((series, season, number)) = names::episode_info(&entry.title) {
            return Classification::Episode { series, season, number };
        }
        if looks_series {
            let series = entry.group.clone().unwrap_or_else(|| entry.title.clone());
            return Classification::Episode { series, season: 1, number: 1 };
        }
        return Classification::Movie;
    }
    // Streamed films in a clearly labeled on-demand group, such as "Films" with "Title (2012)".
    if entry.group.as_deref().is_some_and(|group| ON_DEMAND_GROUP.is_match(group))
        && names::split_year(&entry.title).1.is_some()
    {
        return Classification::Movie;
    }
    Classification::Live
}

/// Turns playlist entries into catalog items. Ids come from the tidied name
/// plus the order of repeats, so a refresh (or a playlist that moves to a new
/// host) keeps favorites, progress and the watchlist.
pub(crate) fn build(playlist: &M3uPlaylist, source_id: &str) -> CatalogSnapshot {
    let mut snapshot = CatalogSnapshot { discovered_epg_url: playlist.epg_url.clone(), ..Default::default() };
    let mut category_ids: HashMap<(CategoryKind, String), String> = HashMap::new();
    let mut used_keys: HashMap<String, usize> = HashMap::new();
    let mut series_index: HashMap<String, usize> = HashMap::new();

    let mut category = |snapshot: &mut CatalogSnapshot, name: Option<&str>, kind: CategoryKind| -> String {
        let name = name.unwrap_or("Uncategorized").to_string();
        if let Some(id) = category_ids.get(&(kind, name.clone())) {
            return id.clone();
        }
        let id = ids::category(source_id, kind, &name);
        category_ids.insert((kind, name.clone()), id.clone());
        let sort_index = snapshot.categories.len() as i64;
        snapshot.categories.push(Category { id: id.clone(), source_id: source_id.into(), kind, name, sort_index });
        id
    };
    let mut unique_key = |base: String| -> String {
        let count = used_keys.entry(base.clone()).or_insert(0);
        *count += 1;
        if *count == 1 { base } else { format!("{base}#{count}") }
    };

    for entry in &playlist.entries {
        if entry.user_agent.is_some() || entry.referrer.is_some() {
            snapshot.stream_options.push(StreamOptions {
                url: entry.url.clone(),
                user_agent: entry.user_agent.clone(),
                referrer: entry.referrer.clone(),
            });
        }
        let stream = StreamLocator::Url { url: entry.url.clone() };
        match classify(entry) {
            Classification::Live => {
                let key = unique_key(format!("c:{}", names::match_key(&entry.title)));
                let category_id = category(&mut snapshot, entry.group.as_deref(), CategoryKind::Live);
                let sort_index = snapshot.channels.len() as i64;
                snapshot.channels.push(Channel {
                    id: ids::channel(source_id, &key),
                    source_id: source_id.into(),
                    name: entry.title.clone(),
                    number: entry.number(),
                    logo_url: entry.logo(),
                    category_id: Some(category_id),
                    epg_id: entry.tvg_id().map(str::to_string),
                    stream,
                    catchup_days: entry.catchup_days(),
                    catchup_source: entry.catchup_template(),
                    sort_index,
                    catchup_offset_minutes: 0,
                    versions: Vec::new(),
                });
            }
            Classification::Movie => {
                let (title, year) = names::split_year(&entry.title);
                let key = unique_key(format!("m:{}", names::match_key(&entry.title)));
                let category_id = category(&mut snapshot, entry.group.as_deref(), CategoryKind::Movie);
                snapshot.movies.push(Movie {
                    id: ids::movie(source_id, &key),
                    source_id: source_id.into(),
                    title,
                    year,
                    poster_url: entry.logo(),
                    backdrop_url: None,
                    category_id: Some(category_id),
                    rating: None,
                    added_at: None,
                    plot: None,
                    genre: None,
                    tmdb_id: entry.attribute("tmdb-id").or_else(|| entry.attribute("tmdb")).map(str::to_string),
                    imdb_id: None,
                    duration_seconds: None,
                    stream,
                });
            }
            Classification::Episode { series, season, number } => {
                let series_key = format!("s:{}", names::match_key(&series));
                let series_id = ids::series(source_id, &series_key);
                if !series_index.contains_key(&series_key) {
                    let category_id = category(&mut snapshot, entry.group.as_deref(), CategoryKind::Series);
                    series_index.insert(series_key.clone(), snapshot.series.len());
                    snapshot.series.push(Series {
                        id: series_id.clone(),
                        source_id: source_id.into(),
                        title: series,
                        year: None,
                        poster_url: entry.logo(),
                        backdrop_url: None,
                        category_id: Some(category_id),
                        rating: None,
                        added_at: None,
                        plot: None,
                        genre: None,
                        tmdb_id: None,
                        imdb_id: None,
                        remote_id: None,
                    });
                }
                let episode_key = unique_key(format!("e:{series_key}:{season}:{number}"));
                snapshot.episodes.push(Episode {
                    id: ids::episode(&series_id, &episode_key),
                    series_id,
                    source_id: source_id.into(),
                    season,
                    number,
                    title: entry.title.clone(),
                    plot: None,
                    duration_seconds: None,
                    still_url: entry.logo(),
                    stream,
                    air_date: None,
                });
            }
        }
    }
    snapshot.derive_capabilities();
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"#EXTM3U url-tvg="https://guide.example/epg.xml.gz,https://backup.example/epg.xml"
#EXTINF:-1 tvg-id="one.uk" tvg-name="One" tvg-logo="https://img.example/one.png" group-title="News, UK" catchup="default" catchup-days="3",UK | Channel One FHD
https://live.example/one.m3u8
#EXTINF:-1 tvg-id="" group-title="Sports",Sports Two
#EXTVLCOPT:http-user-agent=Test
https://live.example/two.ts
#EXTINF:-1 group-title="Movies",The Glass Valley (2018)
https://vod.example/movie/user/pass/55.mp4
#EXTINF:-1 group-title="Series",Tidewater S01E02
https://vod.example/series/user/pass/77.mkv
#EXTINF:-1 group-title="Series",Tidewater S01 E01
https://vod.example/series/user/pass/76.mkv
"#;

    #[test]
    fn parses_entries_attributes_and_guide() {
        let playlist = parse(SAMPLE).unwrap();
        assert_eq!(playlist.entries.len(), 5);
        assert_eq!(playlist.epg_url.as_deref(), Some("https://guide.example/epg.xml.gz"));
        let first = &playlist.entries[0];
        assert_eq!(first.title, "UK | Channel One FHD");
        // Commas inside quoted attributes must not split the title.
        assert_eq!(first.group.as_deref(), Some("News, UK"));
        assert_eq!(first.tvg_id(), Some("one.uk"));
        assert_eq!(first.catchup_days(), 3);
        assert_eq!(first.logo().as_deref(), Some("https://img.example/one.png"));
        assert_eq!(playlist.entries[1].tvg_id(), None);
        assert_eq!(playlist.entries[1].user_agent.as_deref(), Some("Test"));
        assert_eq!(playlist.entries[0].user_agent, None, "player options belong to one link");
    }

    #[test]
    fn classifies_live_movies_and_series() {
        let snapshot = build(&parse(SAMPLE).unwrap(), "s");
        assert_eq!(snapshot.channels.len(), 2);
        assert_eq!(snapshot.movies.len(), 1);
        assert_eq!(snapshot.movies[0].title, "The Glass Valley");
        assert_eq!(snapshot.movies[0].year, Some(2018));
        assert_eq!(snapshot.series.len(), 1);
        assert_eq!(snapshot.episodes.len(), 2);
        assert!(snapshot.capabilities.replay && snapshot.capabilities.guide);
        assert_eq!(snapshot.channels[0].catchup_source.as_deref(), Some("?utc={utc}&lutc={lutc}"));
        assert_eq!(snapshot.stream_options.len(), 1);
        assert_eq!(snapshot.stream_options[0].url, "https://live.example/two.ts");
        assert!(snapshot.categories.iter().any(|c| c.kind == CategoryKind::Series && c.name == "Series"));
    }

    #[test]
    fn stable_ids_survive_refresh_and_reordering() {
        let a = build(&parse(SAMPLE).unwrap(), "s");
        let reordered = "#EXTM3U\n#EXTINF:-1 group-title=\"Sports\",Sports Two\nhttps://new-host.example/two.m3u8\n#EXTINF:-1 group-title=\"News\",UK | Channel One FHD\nhttps://new-host.example/one.m3u8\n";
        let b = build(&parse(reordered).unwrap(), "s");
        let mut ids_a: Vec<_> = a.channels.iter().map(|c| c.id.clone()).collect();
        let mut ids_b: Vec<_> = b.channels.iter().map(|c| c.id.clone()).collect();
        ids_a.sort();
        ids_b.sort();
        assert_eq!(ids_a, ids_b);
    }

    #[test]
    fn duplicate_names_get_distinct_ids() {
        let snapshot = build(&parse("#EXTM3U\n#EXTINF:-1,News\nhttp://a/1\n#EXTINF:-1,News\nhttp://a/2\n").unwrap(), "s");
        assert_ne!(snapshot.channels[0].id, snapshot.channels[1].id);
    }

    #[test]
    fn rejects_non_playlists_and_unsafe_links() {
        assert!(matches!(parse("<html>Not found</html>"), Err(SourceError::InvalidPlaylist(_))));
        let local = "#EXTM3U\n#EXTINF:-1,Secret\nfile:///etc/passwd\n";
        assert!(parse(local).is_err(), "file links never become entries");
        let logo = "#EXTM3U\n#EXTINF:-1 tvg-logo=\"file:///x.png\",A\nhttp://a/1\n";
        assert_eq!(parse(logo).unwrap().entries[0].logo(), None);
    }

    #[test]
    fn tolerates_odd_formatting() {
        // A byte-order mark, Windows line ends, unquoted attributes, no title and Latin-1 text.
        let mut bytes = b"\xef\xbb\xbf#EXTM3U\r\n#EXTINF:-1 tvg-chno=7 tvg-name=\"Named\",\r\nhttp://a/1\r\n".to_vec();
        let playlist = parse_bytes(&bytes).unwrap();
        assert_eq!(playlist.entries[0].title, "Named");
        assert_eq!(playlist.entries[0].number(), Some(7));
        bytes = b"#EXTM3U\n#EXTINF:-1,Caf\xe9\nhttp://a/1\n".to_vec();
        assert_eq!(parse_bytes(&bytes).unwrap().entries[0].title, "Café");
        // An unterminated quote and an #EXTINF with nothing after it don't panic.
        let odd = "#EXTM3U\n#EXTINF:-1 group-title=\"Open,Title\nhttp://a/1\n#EXTINF:\nhttp://a/2\n";
        assert_eq!(parse(odd).unwrap().entries.len(), 2);
    }

    #[test]
    fn films_in_on_demand_groups() {
        let text = "#EXTM3U\n#EXTINF:-1 group-title=\"VOD | Films\",Paper Boats (2024)\nhttp://a/live/1.m3u8\n#EXTINF:-1 group-title=\"News\",Paper Boats (2024)\nhttp://a/live/2.m3u8\n";
        let snapshot = build(&parse(text).unwrap(), "s");
        assert_eq!(snapshot.movies.len(), 1);
        assert_eq!(snapshot.channels.len(), 1);
    }

    #[test]
    fn options_before_extinf_apply_too() {
        let text = "#EXTM3U\n#EXTVLCOPT:http-referrer=https://ref.example/\n#EXTINF:-1,A\nhttp://a/1\n";
        let entry = &parse(text).unwrap().entries[0];
        assert_eq!(entry.referrer.as_deref(), Some("https://ref.example/"));
    }
}
