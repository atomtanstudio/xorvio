//! The Stremio add-on protocol's responses, read leniently (see [`crate::lenient`]).
//! Protocol reference: <https://github.com/Stremio/stremio-addon-sdk/tree/master/docs/api>

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::lenient::{array, boolean, date, float, int, string, string_list, strings, web_url};

/// What an add-on says it offers. Kept in `addons.json` in this normalized
/// form (not the raw manifest), so nothing unexpected an add-on put in its
/// manifest ends up on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Manifest {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub logo: Option<String>,
    pub types: Vec<String>,
    pub catalogs: Vec<CatalogDef>,
    pub resources: Vec<Resource>,
    pub id_prefixes: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogDef {
    pub media_type: String,
    pub id: String,
    pub name: Option<String>,
    /// Extra parameters it accepts ("search", "genre", "skip").
    pub extra_names: Vec<String>,
    /// Extra parameters it can't do without.
    pub required_extras: Vec<String>,
    /// The genres it can be narrowed to.
    pub genres: Vec<String>,
}

/// A resource is either a bare name ("stream") or an object with its own types
/// and id prefixes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Resource {
    pub name: String,
    pub types: Option<Vec<String>>,
    pub id_prefixes: Option<Vec<String>>,
}

impl Manifest {
    pub fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            id: string(value, "id")?,
            name: string(value, "name")?,
            version: string(value, "version"),
            description: string(value, "description"),
            logo: web_url(value, "logo"),
            types: strings(value, "types"),
            catalogs: array(value, "catalogs")
                .iter()
                .filter_map(CatalogDef::from_json)
                .collect(),
            resources: array(value, "resources")
                .iter()
                .filter_map(Resource::from_json)
                .collect(),
            id_prefixes: string_list(value, "idPrefixes"),
        })
    }

    /// Whether it answers `resource` requests for this type and id.
    pub fn supports(&self, resource: &str, media_type: &str, id: &str) -> bool {
        let Some(entry) = self.resources.iter().find(|r| r.name == resource) else {
            return false;
        };
        let types = entry.types.as_ref().unwrap_or(&self.types);
        if !types.iter().any(|t| t == media_type) {
            return false;
        }
        match entry.id_prefixes.as_ref().or(self.id_prefixes.as_ref()) {
            Some(prefixes) if !prefixes.is_empty() => {
                prefixes.iter().any(|p| id.starts_with(p.as_str()))
            }
            _ => true,
        }
    }

    /// Resource names, with "catalog" implied by listed catalogs.
    pub fn resource_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.resources.iter().map(|r| r.name.clone()).collect();
        if !self.catalogs.is_empty() && !names.iter().any(|n| n == "catalog") {
            names.insert(0, "catalog".into());
        }
        names.dedup();
        names
    }
}

impl CatalogDef {
    fn from_json(value: &Value) -> Option<Self> {
        let extras: Vec<Extra> = array(value, "extra")
            .iter()
            .filter_map(Extra::from_json)
            .collect();
        let mut extra_names: Vec<String> = extras.iter().map(|e| e.name.clone()).collect();
        extra_names.extend(string_list(value, "extraSupported").unwrap_or_default());
        let mut required_extras: Vec<String> = extras
            .iter()
            .filter(|e| e.is_required)
            .map(|e| e.name.clone())
            .collect();
        required_extras.extend(string_list(value, "extraRequired").unwrap_or_default());
        let genres = match extras.iter().find(|e| e.name == "genre") {
            Some(genre) => genre.options.clone(),
            None => string_list(value, "genres").unwrap_or_default(),
        };
        Some(Self {
            media_type: string(value, "type")?,
            id: string(value, "id")?,
            name: string(value, "name"),
            extra_names,
            required_extras,
            genres,
        })
    }

    pub fn supports_search(&self) -> bool {
        self.extra_names.iter().any(|n| n == "search")
    }

    /// Can be listed without any extra parameter.
    pub fn is_browsable(&self) -> bool {
        self.required_extras.is_empty()
    }
}

struct Extra {
    name: String,
    is_required: bool,
    options: Vec<String>,
}

impl Extra {
    fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            name: string(value, "name")?,
            is_required: boolean(value, "isRequired").unwrap_or(false),
            options: string_list(value, "options").unwrap_or_default(),
        })
    }
}

impl Resource {
    fn from_json(value: &Value) -> Option<Self> {
        if let Some(name) = value.as_str() {
            return Some(Self {
                name: name.to_string(),
                types: None,
                id_prefixes: None,
            });
        }
        Some(Self {
            name: string(value, "name")?,
            types: string_list(value, "types"),
            id_prefixes: string_list(value, "idPrefixes"),
        })
    }
}

/// A film or series, as a catalog lists it or in full.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Meta {
    pub id: String,
    pub media_type: String,
    pub name: String,
    pub poster: Option<String>,
    pub background: Option<String>,
    pub logo: Option<String>,
    pub description: Option<String>,
    pub release_info: Option<String>,
    pub year: Option<String>,
    pub released: Option<String>,
    pub imdb_rating: Option<f64>,
    pub genres: Vec<String>,
    pub cast: Vec<String>,
    pub director: Vec<String>,
    pub writer: Vec<String>,
    pub runtime: Option<String>,
    pub country: Option<String>,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<String>,
    /// YouTube ids.
    pub trailers: Vec<String>,
    pub videos: Vec<Video>,
}

static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(19|20)\d\d").unwrap());
static HOURS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\d+)\s*h").unwrap());
static MINUTES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\d+)\s*m").unwrap());
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());

impl Meta {
    pub fn from_json(value: &Value) -> Option<Self> {
        // Newer add-ons put people and genres in `links` instead of plain lists.
        let linked = |category: &str| -> Vec<String> {
            array(value, "links")
                .iter()
                .filter(|link| {
                    string(link, "category").is_some_and(|c| c.eq_ignore_ascii_case(category))
                })
                .filter_map(|link| string(link, "name"))
                .collect()
        };
        let or_linked = |list: Vec<String>, category: &str| {
            if list.is_empty() {
                linked(category)
            } else {
                list
            }
        };

        let mut genres = strings(value, "genres");
        if genres.is_empty() {
            genres = strings(value, "genre");
        }
        let mut trailers: Vec<String> = array(value, "trailers")
            .iter()
            .filter(|t| string(t, "type").is_none_or(|kind| kind.eq_ignore_ascii_case("trailer")))
            .filter_map(|t| string(t, "source"))
            .collect();
        for stream in array(value, "trailerStreams") {
            if let Some(id) = string(stream, "ytId").filter(|id| !trailers.contains(id)) {
                trailers.push(id);
            }
        }
        Some(Self {
            id: string(value, "id")?,
            media_type: string(value, "type").unwrap_or_default(),
            name: string(value, "name")?,
            poster: web_url(value, "poster"),
            background: web_url(value, "background"),
            logo: web_url(value, "logo"),
            description: string(value, "description"),
            release_info: string(value, "releaseInfo"),
            year: string(value, "year"),
            released: string(value, "released"),
            imdb_rating: float(value, "imdbRating"),
            genres: or_linked(genres, "Genres"),
            cast: or_linked(strings(value, "cast"), "Cast"),
            director: or_linked(strings(value, "director"), "Directors"),
            writer: or_linked(strings(value, "writer"), "Writers"),
            runtime: string(value, "runtime"),
            country: string(value, "country"),
            imdb_id: string(value, "imdb_id"),
            tmdb_id: string(value, "moviedb_id"),
            trailers,
            videos: array(value, "videos")
                .iter()
                .filter_map(Video::from_json)
                .collect(),
        })
    }

    /// The first year it came out, from whichever field has one.
    pub fn first_year(&self) -> Option<i64> {
        [&self.year, &self.release_info, &self.released]
            .into_iter()
            .flatten()
            .find_map(|text| YEAR.find(text).and_then(|m| m.as_str().parse().ok()))
    }

    /// "120 min", "2h 15min", "1h" or "95" in minutes.
    pub fn runtime_minutes(&self) -> Option<i64> {
        let text = self.runtime.as_deref()?;
        let capture = |re: &Regex| re.captures(text).and_then(|c| c[1].parse::<i64>().ok());
        let minutes = match capture(&HOURS) {
            Some(hours) => hours * 60 + capture(&MINUTES).unwrap_or(0),
            None => NUMBER.find(text)?.as_str().parse().ok()?,
        };
        (minutes > 0).then_some(minutes)
    }
}

/// An episode (or other video) of a title.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Video {
    pub id: String,
    pub title: Option<String>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    /// Unix seconds.
    pub released: Option<i64>,
    pub thumbnail: Option<String>,
    pub overview: Option<String>,
    /// Streams listed with the video itself, as some add-ons do for files they hold.
    pub streams: Vec<Stream>,
}

impl Video {
    fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            id: string(value, "id")?,
            title: string(value, "name").or_else(|| string(value, "title")),
            season: int(value, "season"),
            episode: int(value, "episode").or_else(|| int(value, "number")),
            released: string(value, "released")
                .or_else(|| string(value, "firstAired"))
                .and_then(|text| date(&text)),
            thumbnail: web_url(value, "thumbnail"),
            overview: string(value, "overview").or_else(|| string(value, "description")),
            streams: array(value, "streams")
                .iter()
                .filter_map(Stream::from_json)
                .collect(),
        })
    }
}

/// One way to play a title.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Stream {
    pub url: Option<String>,
    pub info_hash: Option<String>,
    pub name: Option<String>,
    /// The protocol's newer field; older add-ons write `title` instead.
    pub description: Option<String>,
    pub filename: Option<String>,
    pub video_size: Option<i64>,
    /// `behaviorHints.proxyHeaders.request`: headers the stream's host expects.
    pub request_headers: Vec<(String, String)>,
}

impl Stream {
    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        if !value.is_object() {
            return None;
        }
        let hints = value.get("behaviorHints").cloned().unwrap_or(Value::Null);
        let request_headers = hints
            .get("proxyHeaders")
            .and_then(|p| p.get("request"))
            .and_then(Value::as_object)
            .map(|headers| {
                headers
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            url: string(value, "url"),
            info_hash: string(value, "infoHash"),
            name: string(value, "name"),
            description: string(value, "description").or_else(|| string(value, "title")),
            filename: string(&hints, "filename"),
            video_size: int(&hints, "videoSize"),
            request_headers,
        })
    }
}

/// A subtitle file an add-on offers for a title.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Subtitle {
    pub url: String,
    /// Usually ISO 639-2 ("eng"), sometimes 639-1 ("en").
    pub language: String,
}

impl Subtitle {
    fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            url: web_url(value, "url")?,
            language: string(value, "lang").unwrap_or_else(|| "und".into()),
        })
    }
}

/// `{ "metas": [...] }`; entries that can't be read are skipped.
pub(crate) fn metas(response: &Value) -> Vec<Meta> {
    array(response, "metas")
        .iter()
        .filter_map(Meta::from_json)
        .collect()
}

pub(crate) fn meta(response: &Value) -> Option<Meta> {
    response.get("meta").and_then(Meta::from_json)
}

pub(crate) fn streams(response: &Value) -> Vec<Stream> {
    array(response, "streams")
        .iter()
        .filter_map(Stream::from_json)
        .collect()
}

pub(crate) fn subtitles(response: &Value) -> Vec<Subtitle> {
    array(response, "subtitles")
        .iter()
        .filter_map(Subtitle::from_json)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn manifest_reads_resources_both_ways() {
        let manifest = Manifest::from_json(&json!({
            "id": "org.example", "name": "Example", "types": ["movie", "series"],
            "resources": ["catalog", { "name": "stream", "types": ["movie"], "idPrefixes": ["tt"] }, 42],
            "catalogs": [
                { "type": "movie", "id": "top", "name": "Top", "extra": [{ "name": "genre", "options": ["Drama"] }, { "name": "skip" }] },
                { "type": "movie", "id": "find", "extra": [{ "name": "search", "isRequired": true }] },
                { "type": "series", "id": "legacy", "extraSupported": ["search"], "genres": ["Crime"] },
                { "id": "broken" }
            ]
        }))
        .unwrap();
        assert_eq!(manifest.resources.len(), 2);
        assert_eq!(manifest.catalogs.len(), 3);
        assert!(manifest.supports("stream", "movie", "tt0078788"));
        assert!(!manifest.supports("stream", "movie", "kitsu:1"));
        assert!(!manifest.supports("stream", "series", "tt0141842:1:1"));
        assert!(!manifest.supports("meta", "movie", "tt0078788"));
        assert!(manifest.supports("catalog", "series", "anything"));
        let top = &manifest.catalogs[0];
        assert_eq!(top.genres, vec!["Drama"]);
        assert!(top.is_browsable() && !top.supports_search());
        assert!(!manifest.catalogs[1].is_browsable() && manifest.catalogs[1].supports_search());
        assert!(manifest.catalogs[2].supports_search());
        assert_eq!(manifest.catalogs[2].genres, vec!["Crime"]);
    }

    #[test]
    fn meta_reads_odd_fields() {
        let meta = Meta::from_json(&json!({
            "id": "tt0141842", "type": "series", "name": "The Sopranos",
            "releaseInfo": "1999–2007", "imdbRating": "9.2", "runtime": "1h 5min",
            "genre": "Crime, Drama", "poster": "/relative.jpg",
            "links": [{ "name": "James Gandolfini", "category": "Cast" }, { "name": "David Chase", "category": "Writers" }],
            "trailers": [{ "source": "abc123", "type": "Trailer" }, { "source": "clip1", "type": "Clip" }],
            "trailerStreams": [{ "ytId": "abc123" }, { "ytId": "def456" }],
            "videos": [
                { "id": "tt0141842:1:1", "name": "Pilot", "season": "1", "number": 1, "released": "1999-01-10T00:00:00.000Z" },
                { "title": "no id" }
            ]
        }))
        .unwrap();
        assert_eq!(meta.first_year(), Some(1999));
        assert_eq!(meta.imdb_rating, Some(9.2));
        assert_eq!(meta.runtime_minutes(), Some(65));
        assert_eq!(meta.genres, vec!["Crime", "Drama"]);
        assert_eq!(meta.poster, None);
        assert_eq!(meta.cast, vec!["James Gandolfini"]);
        assert_eq!(meta.writer, vec!["David Chase"]);
        assert_eq!(meta.trailers, vec!["abc123", "def456"]);
        assert_eq!(meta.videos.len(), 1);
        assert_eq!(meta.videos[0].season, Some(1));
        assert_eq!(meta.videos[0].episode, Some(1));
        assert_eq!(meta.videos[0].title.as_deref(), Some("Pilot"));
    }

    #[test]
    fn runtime_forms() {
        let with = |runtime: &str| Meta {
            runtime: Some(runtime.into()),
            ..Meta::from_json(&json!({"id": "x", "name": "x"})).unwrap()
        };
        assert_eq!(with("120 min").runtime_minutes(), Some(120));
        assert_eq!(with("2h").runtime_minutes(), Some(120));
        assert_eq!(with("95").runtime_minutes(), Some(95));
        assert_eq!(with("N/A").runtime_minutes(), None);
    }

    #[test]
    fn stream_reads_hints() {
        let stream = Stream::from_json(&json!({
            "url": "https://cdn.example/v.mkv", "title": "Old style",
            "behaviorHints": { "filename": "v.mkv", "videoSize": 1024, "proxyHeaders": { "request": { "Referer": "https://example" } } }
        }))
        .unwrap();
        assert_eq!(stream.description.as_deref(), Some("Old style"));
        assert_eq!(stream.video_size, Some(1024));
        assert_eq!(
            stream.request_headers,
            vec![("Referer".to_string(), "https://example".to_string())]
        );
    }
}
