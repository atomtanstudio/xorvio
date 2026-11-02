//! The rich metadata Xtream panels attach to films and series. Panels usually
//! fill these fields from TMDB themselves; field names and types vary between
//! panels, so every reader is tolerant.

use serde_json::Value;
use xorvio_model::{SeasonInfo, VideoFormat};

use crate::details::DetailsRecord;
use crate::json::{self, Object};

/// A parsed `get_vod_info` reply.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct MovieInfo {
    pub details: DetailsRecord,
    pub plot: Option<String>,
    pub genre: Option<String>,
    pub duration_seconds: Option<i64>,
}

pub(crate) fn movie_info(root: &Value) -> Option<MovieInfo> {
    let info = root.as_object().and_then(|root| json::object(root, "info"))?;
    Some(MovieInfo {
        details: from_movie_info(info),
        plot: text(info.get("plot")).or_else(|| text(info.get("description"))),
        genre: text(info.get("genre")),
        duration_seconds: json::int(info.get("duration_secs")).filter(|seconds| *seconds > 0),
    })
}

/// The first of several field names a panel might use.
fn either<'a>(object: &'a Object, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| object.get(*key).filter(|value| !value.is_null()))
}

/// Details carried by a `get_vod_streams` or `get_series` list entry.
pub(crate) fn from_list_entry(raw: &Object) -> DetailsRecord {
    DetailsRecord {
        backdrop_urls: image_urls(raw.get("backdrop_path")),
        cast: names(either(raw, &["cast", "actors"])),
        director: text(raw.get("director")),
        release_date: text(either(raw, &["releaseDate", "release_date", "releasedate"])),
        trailer_youtube_id: youtube_id(either(raw, &["youtube_trailer", "trailer"])),
        age_rating: age_rating(either(raw, &["mpaa_rating", "age", "certification"])),
        ..Default::default()
    }
}

/// The `info` object of `get_vod_info`.
pub(crate) fn from_movie_info(info: &Object) -> DetailsRecord {
    let mut details = from_list_entry(info);
    details.country = text(info.get("country"));
    details.original_title = text(either(info, &["o_name", "original_name"]));
    details.tagline = text(info.get("tagline"));
    details.video = video_format(info.get("video"));
    details.audio_channels = json::object(info, "audio").and_then(|audio| json::int(audio.get("channels")));
    details.genres = genre_list(info.get("genre"));
    details
}

/// The `info` object and `seasons` list of `get_series_info`.
pub(crate) fn from_series_info(root: &Value) -> DetailsRecord {
    let Some(root) = root.as_object() else { return DetailsRecord::default() };
    let empty = Object::new();
    let info = json::object(root, "info").unwrap_or(&empty);
    let mut details = from_list_entry(info);
    details.country = text(info.get("country"));
    details.genres = genre_list(info.get("genre"));
    let mut seasons: Vec<SeasonInfo> = json::objects(root.get("seasons"))
        .into_iter()
        .filter_map(|raw| {
            Some(SeasonInfo {
                number: json::int(raw.get("season_number"))?,
                name: text(raw.get("name")),
                overview: text(raw.get("overview")),
                air_date: text(raw.get("air_date")),
                cover_url: image_urls(either(raw, &["cover_big", "cover"])).into_iter().next(),
                episode_count: json::int(raw.get("episode_count")),
            })
        })
        .collect();
    seasons.sort_by_key(|season| season.number);
    details.seasons = seasons;
    details
}

/// Text that means something: not empty, "0", "null" or "N/A".
pub(crate) fn text(value: Option<&Value>) -> Option<String> {
    let text = json::string(value)?;
    let trimmed = text.trim();
    let lower = trimmed.to_lowercase();
    (!trimmed.is_empty() && trimmed != "0" && lower != "null" && lower != "n/a").then(|| trimmed.to_string())
}

/// Comma-separated names ("A, B, C") or a list, as a short list.
fn names(value: Option<&Value>) -> Vec<String> {
    let list: Vec<String> = match value {
        Some(Value::Array(items)) => items.iter().filter_map(|item| json::string(Some(item))).collect(),
        other => match text(other) {
            Some(text) => text.split(',').map(str::to_string).collect(),
            None => return Vec::new(),
        },
    };
    list.into_iter().map(|name| name.trim().to_string()).filter(|name| !name.is_empty()).take(12).collect()
}

/// "Drama, Thriller" or "Drama / Thriller" as separate genres.
fn genre_list(value: Option<&Value>) -> Vec<String> {
    text(value)
        .map(|text| {
            text.split([',', '/'])
                .map(|genre| genre.trim().to_string())
                .filter(|genre| !genre.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Image links from a list or a single string. TMDB-style paths ("/abc.jpg")
/// are completed with the image host the panel took them from.
pub(crate) fn image_urls(value: Option<&Value>) -> Vec<String> {
    let strings: Vec<String> = match value {
        Some(Value::Array(items)) => items.iter().filter_map(|item| json::string(Some(item))).collect(),
        other => json::string(other).into_iter().collect(),
    };
    strings
        .into_iter()
        .filter_map(|raw| {
            let trimmed = raw.trim();
            if trimmed.starts_with('/') && trimmed.len() > 1 {
                return Some(format!("https://image.tmdb.org/t/p/w1280{trimmed}"));
            }
            json::web_url(trimmed)
        })
        .collect()
}

/// A YouTube video id from an id or any YouTube link.
pub(crate) fn youtube_id(value: Option<&Value>) -> Option<String> {
    let raw = text(value)?;
    if let Ok(parsed) = url::Url::parse(&raw)
        && let Some(host) = parsed.host_str()
    {
        let segments: Vec<&str> = parsed.path_segments().map(|s| s.filter(|s| !s.is_empty()).collect()).unwrap_or_default();
        if host.contains("youtu.be") {
            return segments.first().map(|id| id.to_string());
        }
        if host.contains("youtube.com") {
            if let Some((_, id)) = parsed.query_pairs().find(|(name, _)| name == "v") {
                return Some(id.into_owned());
            }
            return segments.last().map(|id| id.to_string());
        }
    }
    let valid = (6..=20).contains(&raw.chars().count())
        && raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    valid.then_some(raw)
}

/// Some panels prefix the country ("US: PG-13") or append prose.
fn age_rating(value: Option<&Value>) -> Option<String> {
    let raw = text(value)?;
    let cleaned = raw.rsplit(':').next().unwrap_or(&raw).trim().to_string();
    (!cleaned.is_empty() && cleaned.chars().count() <= 8).then_some(cleaned)
}

/// `video` is an ffprobe stream description.
fn video_format(value: Option<&Value>) -> Option<VideoFormat> {
    let video = value?.as_object()?;
    let width = json::int(video.get("width")).filter(|width| *width > 0)?;
    let height = json::int(video.get("height")).filter(|height| *height > 0)?;
    let transfer = json::string(video.get("color_transfer")).unwrap_or_default().to_lowercase();
    Some(VideoFormat {
        width,
        height,
        codec: json::string(video.get("codec_name")),
        is_hdr: transfer.contains("2084") || transfer.contains("b67"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_vod_info_in_any_shape() {
        let root = json!({
            "info": {
                "plot": " A surveyor maps a canyon. ",
                "genre": "Science Fiction / Drama",
                "duration_secs": "5520",
                "cast": "Mara Ellison, Joon Park, ",
                "director": "N/A",
                "backdrop_path": ["/abc.jpg", "https://img.example/b.jpg", "file:///x"],
                "youtube_trailer": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                "mpaa_rating": "US: PG-13",
                "video": {"width": 3840, "height": "2160", "codec_name": "hevc", "color_transfer": "smpte2084"},
                "audio": {"channels": 6},
                "releasedate": "2018-04-12"
            }
        });
        let info = movie_info(&root).unwrap();
        assert_eq!(info.plot.as_deref(), Some("A surveyor maps a canyon."));
        assert_eq!(info.duration_seconds, Some(5520));
        let details = info.details;
        assert_eq!(details.cast, ["Mara Ellison", "Joon Park"]);
        assert_eq!(details.director, None);
        assert_eq!(details.backdrop_urls, ["https://image.tmdb.org/t/p/w1280/abc.jpg", "https://img.example/b.jpg"]);
        assert_eq!(details.trailer_youtube_id.as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(details.age_rating.as_deref(), Some("PG-13"));
        assert!(details.video.as_ref().is_some_and(|video| video.is_hdr && video.height == 2160));
        assert_eq!(details.audio_channels, Some(6));
        assert_eq!(details.genres, ["Science Fiction", "Drama"]);
        assert_eq!(details.release_year(), Some(2018));

        // Panels send `[]` for a missing info object, or nothing at all.
        assert_eq!(movie_info(&json!({"info": []})), None);
        assert_eq!(movie_info(&json!([])), None);
        assert_eq!(movie_info(&json!(null)), None);
    }

    #[test]
    fn youtube_ids_from_any_form() {
        for (raw, expected) in [
            ("dQw4w9WgXcQ", Some("dQw4w9WgXcQ")),
            ("https://youtu.be/dQw4w9WgXcQ", Some("dQw4w9WgXcQ")),
            ("https://www.youtube.com/embed/dQw4w9WgXcQ", Some("dQw4w9WgXcQ")),
            ("not a trailer!", None),
            ("0", None),
        ] {
            assert_eq!(youtube_id(Some(&json!(raw))).as_deref(), expected, "{raw}");
        }
    }

    #[test]
    fn series_info_seasons() {
        let root = json!({
            "info": {"cast": ["A", 7, null], "country": "Ireland", "youtube_trailer": "dQw4w9WgXcQ"},
            "seasons": [{"season_number": "2", "name": "Season 2"}, {"season_number": 1, "cover": "/c.jpg"}, {"name": "No number"}]
        });
        let details = from_series_info(&root);
        assert_eq!(details.seasons.iter().map(|s| s.number).collect::<Vec<_>>(), [1, 2]);
        assert!(details.seasons[0].cover_url.as_deref().is_some_and(|url| url.ends_with("/c.jpg")));
        assert_eq!(details.cast, ["A", "7"]);
        assert_eq!(details.country.as_deref(), Some("Ireland"));
        assert_eq!(details.trailer_youtube_id.as_deref(), Some("dQw4w9WgXcQ"));
        assert!(from_series_info(&json!({"info": [], "seasons": {}})).is_empty());
    }
}
