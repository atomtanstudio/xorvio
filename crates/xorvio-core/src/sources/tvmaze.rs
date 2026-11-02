//! Free series information from TVmaze (CC BY-SA; credited as "TVmaze").
//! Only the series title is sent. Images stay on TVmaze's servers and are
//! shown from there.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use xorvio_model::CastMember;

use crate::details::DetailsRecord;
use crate::error::SourceError;
use crate::http::Http;
use crate::json;
use crate::names;

pub(crate) const DEFAULT_BASE: &str = "https://api.tvmaze.com";

/// Details for the best match, or `None` when TVmaze's match isn't
/// convincingly the same show.
pub(crate) async fn series_details(
    http: &Http,
    base: &str,
    title: &str,
    year: Option<i64>,
) -> Result<Option<DetailsRecord>, SourceError> {
    let Ok(mut link) = url::Url::parse(&format!("{}/singlesearch/shows", base.trim_end_matches('/'))) else {
        return Ok(None);
    };
    link.query_pairs_mut().append_pair("q", title).append_pair("embed[]", "cast").append_pair("embed[]", "images");
    let show = match http.json(link.as_str(), None).await {
        Ok(show) => show,
        // TVmaze answers 404 when nothing matches.
        Err(SourceError::Http(404)) => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(parse_show(&show, title, year))
}

pub(crate) fn parse_show(show: &Value, title: &str, year: Option<i64>) -> Option<DetailsRecord> {
    let show = show.as_object()?;
    // Accept only a confident match: the same tidy name, and a premiere year within one of ours.
    let name = json::string(show.get("name")).unwrap_or_default();
    if names::match_key(&name) != names::match_key(title) || names::match_key(title).is_empty() {
        return None;
    }
    let premiered = json::string(show.get("premiered"));
    let show_year = premiered.as_deref().and_then(|date| date.get(..4)?.parse::<i64>().ok());
    if let (Some(year), Some(show_year)) = (year, show_year)
        && (show_year - year).abs() > 1
    {
        return None;
    }

    let embedded = json::object(show, "_embedded");
    let images = json::objects(embedded.and_then(|e| e.get("images")));
    let backgrounds: Vec<String> = images
        .iter()
        .filter(|image| json::string(image.get("type")).as_deref() == Some("background"))
        .filter_map(|image| {
            let original = json::object(json::object(image, "resolutions")?, "original")?;
            json::url(original.get("url"))
        })
        .take(5)
        .collect();
    let cast = json::objects(embedded.and_then(|e| e.get("cast")));
    let people: Vec<CastMember> = cast
        .iter()
        .take(20)
        .filter_map(|entry| {
            let person = json::object(entry, "person")?;
            Some(CastMember {
                name: json::string(person.get("name"))?,
                role: json::object(entry, "character").and_then(|character| json::string(character.get("name"))),
                photo_url: json::object(person, "image").and_then(|image| json::url(image.get("medium"))),
                tmdb_id: None,
            })
        })
        .collect();
    let network = json::object(show, "network")
        .or_else(|| json::object(show, "webChannel"))
        .and_then(|network| json::string(network.get("name")));
    let genres = match show.get("genres") {
        Some(Value::Array(items)) => items.iter().filter_map(|genre| json::string(Some(genre))).collect(),
        _ => Vec::new(),
    };
    Some(DetailsRecord {
        backdrop_urls: backgrounds,
        cast: people.iter().take(10).map(|person| person.name.clone()).collect(),
        people,
        genres,
        runtime_minutes: json::int(show.get("averageRuntime")).or_else(|| json::int(show.get("runtime"))),
        network,
        release_date: premiered,
        imdb_id: json::object(show, "externals").and_then(|externals| json::string(externals.get("imdb"))),
        overview: json::string(show.get("summary")).map(|summary| plain_text(&summary)).filter(|s| !s.is_empty()),
        attribution: Some("TVmaze".into()),
        ..Default::default()
    })
}

static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new("<[^>]+>").expect("built-in pattern"));

/// TVmaze summaries are short HTML fragments.
fn plain_text(html: &str) -> String {
    TAGS.replace_all(html, "")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn show() -> Value {
        json!({
            "name": "Tidewater",
            "premiered": "2025-03-02",
            "genres": ["Drama", 3],
            "averageRuntime": 44,
            "network": null,
            "webChannel": {"name": "Harbor One"},
            "externals": {"imdb": "tt0000001"},
            "summary": "<p>A coastal town&#39;s families &amp; their harbor.</p>",
            "_embedded": {
                "images": [
                    {"type": "poster", "resolutions": {"original": {"url": "https://img.example/p.jpg"}}},
                    {"type": "background", "resolutions": {"original": {"url": "https://img.example/b.jpg"}}},
                    {"type": "background", "resolutions": []}
                ],
                "cast": [
                    {"person": {"name": "Mara Ellison", "image": {"medium": "https://img.example/m.jpg"}}, "character": {"name": "Ada"}},
                    {"person": {"name": "Joon Park", "image": null}, "character": null},
                    {"person": null}
                ]
            }
        })
    }

    #[test]
    fn reads_a_confident_match() {
        let details = parse_show(&show(), "Tidewater", Some(2026)).unwrap();
        assert_eq!(details.backdrop_urls, ["https://img.example/b.jpg"]);
        assert_eq!(details.people.len(), 2);
        assert_eq!(details.people[0].role.as_deref(), Some("Ada"));
        assert_eq!(details.people[1].photo_url, None);
        assert_eq!(details.network.as_deref(), Some("Harbor One"));
        assert_eq!(details.overview.as_deref(), Some("A coastal town's families & their harbor."));
        assert_eq!(details.genres, ["Drama", "3"]);
        assert_eq!(details.attribution.as_deref(), Some("TVmaze"));
    }

    #[test]
    fn rejects_doubtful_matches() {
        assert!(parse_show(&show(), "Tidewater Bay", None).is_none(), "a different name");
        assert!(parse_show(&show(), "Tidewater", Some(2019)).is_none(), "years too far apart");
        assert!(parse_show(&json!([]), "Tidewater", None).is_none());
    }
}
