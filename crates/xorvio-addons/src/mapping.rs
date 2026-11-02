//! Turns add-on titles into Xorvio's films, series, episodes and details. A
//! title keeps the id add-ons know it by in its Xorvio id
//! (`addon|<type>|<meta id>`) and in its [`StreamLocator::Addon`], so copies
//! can be asked for later.

use std::sync::LazyLock;

use regex::Regex;
use xorvio_model::{CastMember, Episode, MediaDetails, Movie, RatingSource, Series, StreamLocator, Timestamp, TitleRating};

use crate::protocol::Meta;

/// The `source_id` of every add-on title.
pub const SOURCE_ID: &str = "addon";

/// `addon|<type>|<meta id>`
pub fn item_id(media_type: &str, meta_id: &str) -> String {
    format!("{SOURCE_ID}|{media_type}|{meta_id}")
}

/// The type and meta id inside an add-on item id; the meta id itself may hold `|`.
pub fn parse_item_id(item_id: &str) -> Option<(String, String)> {
    let mut parts = item_id.splitn(3, '|');
    (parts.next()? == SOURCE_ID).then_some(())?;
    let media_type = parts.next()?;
    let id = parts.next()?;
    (!media_type.is_empty() && !id.is_empty()).then(|| (media_type.to_string(), id.to_string()))
}

fn locator(media_type: &str, id: &str) -> StreamLocator {
    StreamLocator::Addon {
        media_type: media_type.to_string(),
        media_id: id.to_string(),
    }
}

fn genre(meta: &Meta) -> Option<String> {
    (!meta.genres.is_empty()).then(|| meta.genres.join(", "))
}

fn imdb_id(meta: &Meta) -> Option<String> {
    meta.imdb_id
        .clone()
        .or_else(|| meta.id.starts_with("tt").then(|| meta.id.clone()))
}

/// A film, or a title of another kind ("other": a file someone holds), which
/// plays like a film and is named by its release.
pub(crate) fn movie(meta: &Meta, media_type: &str) -> Movie {
    let (title, year) = if media_type == "movie" {
        (meta.name.clone(), meta.first_year())
    } else {
        let (title, year) = release_title(&meta.name);
        (title, year.or_else(|| meta.first_year()))
    };
    Movie {
        id: item_id(media_type, &meta.id),
        source_id: SOURCE_ID.into(),
        title,
        year,
        poster_url: meta.poster.clone(),
        backdrop_url: meta.background.clone(),
        category_id: None,
        rating: meta.imdb_rating.filter(|r| *r > 0.0),
        added_at: None,
        plot: meta.description.clone(),
        genre: genre(meta),
        tmdb_id: meta.tmdb_id.clone(),
        imdb_id: imdb_id(meta),
        duration_seconds: meta.runtime_minutes().map(|m| m * 60),
        stream: locator(media_type, &meta.id),
    }
}

pub(crate) fn series(meta: &Meta) -> Series {
    Series {
        id: item_id("series", &meta.id),
        source_id: SOURCE_ID.into(),
        title: meta.name.clone(),
        year: meta.first_year(),
        poster_url: meta.poster.clone(),
        backdrop_url: meta.background.clone(),
        category_id: None,
        rating: meta.imdb_rating.filter(|r| *r > 0.0),
        added_at: None,
        plot: meta.description.clone(),
        genre: genre(meta),
        tmdb_id: meta.tmdb_id.clone(),
        imdb_id: imdb_id(meta),
        remote_id: Some(meta.id.clone()),
    }
}

/// A series' released episodes, specials included. Episodes listed ahead of
/// airing are left out: there's nothing to play yet.
pub(crate) fn episodes(meta: &Meta, series_id: &str, now: Timestamp) -> Vec<Episode> {
    let mut episodes: Vec<Episode> = meta
        .videos
        .iter()
        .filter_map(|video| {
            let season = video.season?;
            let number = video.episode.filter(|n| *n > 0)?;
            if video.released.is_some_and(|released| released > now) {
                return None;
            }
            Some(Episode {
                id: format!("{series_id}|ep|{}", video.id),
                series_id: series_id.to_string(),
                source_id: SOURCE_ID.into(),
                season,
                number,
                title: video
                    .title
                    .clone()
                    .unwrap_or_else(|| format!("Episode {number}")),
                plot: video.overview.clone(),
                duration_seconds: meta.runtime_minutes().map(|m| m * 60),
                still_url: video.thumbnail.clone(),
                stream: locator("series", &video.id),
                air_date: video.released,
            })
        })
        .collect();
    episodes.sort_by_key(|e| (e.season, e.number));
    episodes.dedup_by(|a, b| a.id == b.id);
    episodes
}

pub(crate) fn details(meta: &Meta) -> MediaDetails {
    MediaDetails {
        backdrop_urls: meta.background.iter().cloned().collect(),
        logo_url: meta.logo.clone(),
        people: meta
            .cast
            .iter()
            .map(|name| CastMember {
                name: name.clone(),
                ..CastMember::default()
            })
            .collect(),
        director: meta.director.first().cloned(),
        creators: if meta.media_type == "series" {
            meta.writer.clone()
        } else {
            Vec::new()
        },
        release_date: meta
            .released
            .as_ref()
            .and_then(|d| d.get(..10))
            .map(String::from),
        country: meta.country.clone(),
        trailer_youtube_id: meta.trailers.first().cloned(),
        genres: meta.genres.clone(),
        runtime_minutes: meta.runtime_minutes(),
        imdb_id: imdb_id(meta),
        overview: meta.description.clone(),
        // Add-ons that list titles from IMDb carry its score.
        ratings: meta
            .imdb_rating
            .filter(|score| *score > 0.0 && *score <= 10.0)
            .map(|score| TitleRating { source: RatingSource::Imdb, score, votes: None })
            .into_iter()
            .collect(),
        ..MediaDetails::default()
    }
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

static VIDEO_EXTENSION: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\.(mkv|mp4|avi|m4v)$"));
static SITE_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^\s*\[?\s*www\.[A-Za-z0-9-]+\.[A-Za-z]{2,}\s*\]?\s*[-–_]*\s*"));
static SEPARATORS: LazyLock<Regex> = LazyLock::new(|| re(r"[._]"));
static MARKER: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(S\d{1,2}(E\d{1,3})?|Season \d+|(19|20)\d{2}|2160p|1080p|720p|480p|4K|UHD|BluRay|WEB-?DL|WEBRip|HDTV|REMUX)\b|[\[(]",
    )
});
static PACK_WORDS: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)(\s+(TV|Complete|Series|Collection|Pack|Boxset|Box Set))+$"));
static LEADING_YEAR: LazyLock<Regex> = LazyLock::new(|| re(r"^((19|20)\d\d)\b"));
static EPISODE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^S(\d{1,2})E(\d{1,3})"));
static SEASONS: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^S(\d{1,2})\s*-\s*S?(\d{1,2})\b"));
static SEASON: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^(?:S(\d{1,2})\b|Season (\d+))"));

/// A readable title from a release name: "The.Sopranos.S06.1080p.BluRay-GRP"
/// is "The Sopranos · Season 6", "Heat.1995.2160p.UHD" is "Heat" from 1995.
pub(crate) fn release_title(release: &str) -> (String, Option<i64>) {
    let text = VIDEO_EXTENSION.replace(release, "");
    let text = SITE_PREFIX.replace(&text, "");
    let text = SEPARATORS.replace_all(&text, " ").to_string();
    let Some(marker) = MARKER.find(&text).filter(|m| m.start() > 0) else {
        return (text.trim().to_string(), None);
    };
    let head = text[..marker.start()].trim_matches([' ', '-']);
    let title = PACK_WORDS.replace(head, "").trim().to_string();
    let rest = &text[marker.start()..];
    let number = |text: &str| text.parse::<i64>().unwrap_or(0);
    let year = LEADING_YEAR.captures(rest).map(|c| number(&c[1]));
    if title.is_empty() {
        return (release.to_string(), year);
    }
    if let Some(c) = EPISODE.captures(rest) {
        return (
            format!("{title} · S{} E{}", number(&c[1]), number(&c[2])),
            year,
        );
    }
    if let Some(c) = SEASONS.captures(rest) {
        return (
            format!("{title} · Seasons {}–{}", number(&c[1]), number(&c[2])),
            year,
        );
    }
    if let Some(c) = SEASON.captures(rest) {
        let season = c.get(1).or(c.get(2)).map_or(0, |m| number(m.as_str()));
        return (format!("{title} · Season {season}"), year);
    }
    (title, year)
}

/// Lower-case letters and digits only, so "Spider-Man" and "Spider Man" meet.
pub(crate) fn match_key(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The words that identify a title, without short ones and articles.
pub(crate) fn words(text: &str) -> Vec<String> {
    const SKIP: [&str; 7] = ["the", "and", "for", "les", "der", "die", "das"];
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 3 && !SKIP.contains(&w.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta(value: serde_json::Value) -> Meta {
        Meta::from_json(&value).unwrap()
    }

    #[test]
    fn item_ids_round_trip() {
        assert_eq!(item_id("movie", "tt1"), "addon|movie|tt1");
        assert_eq!(
            parse_item_id("addon|series|a|b"),
            Some(("series".into(), "a|b".into()))
        );
        assert_eq!(parse_item_id("demo|mv|1"), None);
    }

    #[test]
    fn maps_a_film() {
        let film = movie(
            &meta(
                json!({ "id": "tt0078788", "type": "movie", "name": "Apocalypse Now", "year": "1979", "imdbRating": 8.4, "runtime": "147 min", "genres": ["War"] }),
            ),
            "movie",
        );
        assert_eq!(film.id, "addon|movie|tt0078788");
        assert_eq!(film.source_id, "addon");
        assert_eq!(film.year, Some(1979));
        assert_eq!(film.duration_seconds, Some(147 * 60));
        assert_eq!(film.imdb_id.as_deref(), Some("tt0078788"));
        assert_eq!(
            film.stream,
            StreamLocator::Addon {
                media_type: "movie".into(),
                media_id: "tt0078788".into()
            }
        );
    }

    #[test]
    fn an_imdb_score_is_a_rating() {
        let film = meta(json!({ "id": "tt0078788", "type": "movie", "name": "Apocalypse Now", "imdbRating": "8.4" }));
        assert_eq!(
            details(&film).ratings,
            [TitleRating { source: RatingSource::Imdb, score: 8.4, votes: None }]
        );
        let unrated = meta(json!({ "id": "tt1", "type": "movie", "name": "New", "imdbRating": "0" }));
        assert!(details(&unrated).ratings.is_empty());
    }

    #[test]
    fn maps_episodes_in_order_without_unaired_ones() {
        let show = meta(
            json!({ "id": "tt1", "type": "series", "name": "Show", "videos": [
                { "id": "tt1:1:2", "season": 1, "episode": 2, "released": "2001-01-08" },
                { "id": "tt1:1:1", "season": 1, "episode": 1, "released": "2001-01-01" },
                { "id": "tt1:0:0", "season": 0, "episode": 0 },
                { "id": "tt1:2:1", "season": 2, "episode": 1, "released": "2999-01-01" }
            ]}),
        );
        let list = episodes(&show, "addon|series|tt1", 1_700_000_000);
        let ids: Vec<&str> = list.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            ["addon|series|tt1|ep|tt1:1:1", "addon|series|tt1|ep|tt1:1:2"]
        );
        assert_eq!(list[0].title, "Episode 1");
        assert_eq!(
            list[0].stream,
            StreamLocator::Addon {
                media_type: "series".into(),
                media_id: "tt1:1:1".into()
            }
        );
    }

    #[test]
    fn reads_release_names() {
        assert_eq!(
            release_title("The.Sopranos.S06.1080p.BluRay-GRP"),
            ("The Sopranos · Season 6".into(), None)
        );
        assert_eq!(
            release_title("Heat.1995.2160p.UHD.mkv"),
            ("Heat".into(), Some(1995))
        );
        assert_eq!(
            release_title("www.Site.org - Show.S01E02.720p"),
            ("Show · S1 E2".into(), None)
        );
        assert_eq!(
            release_title("Show Complete Series S01-S05"),
            ("Show · Seasons 1–5".into(), None)
        );
        assert_eq!(release_title("Plain name"), ("Plain name".into(), None));
    }

    #[test]
    fn keys_and_words() {
        assert_eq!(match_key("Spider-Man: No Way Home"), "spidermannowayhome");
        assert_eq!(words("The Lord of the Rings"), vec!["lord", "rings"]);
    }
}
