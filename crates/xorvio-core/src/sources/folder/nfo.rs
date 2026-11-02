//! What a media manager (Kodi, Jellyfin, Emby, tinyMediaManager) wrote about
//! a title in the .nfo file beside it: `movie.nfo` or `<file>.nfo` for a
//! film, `tvshow.nfo` for a series, `<file>.nfo` for an episode. Usually XML,
//! sometimes followed by (or only) a link to the title's IMDb or TMDB page.
//! Nothing is looked up online.

use std::sync::LazyLock;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, BytesText, Event};
use regex::Regex;
use serde::{Deserialize, Serialize};
use xorvio_model::{CastMember, Timestamp};

use crate::details::DetailsRecord;

/// Far bigger than any real one; a larger file isn't an NFO.
pub(crate) const MAX_BYTES: u64 = 1 << 20;
/// Actors kept from one file, enough for a title's page.
const MOST_CAST: usize = 25;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Nfo {
    pub title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i64>,
    pub plot: Option<String>,
    pub tagline: Option<String>,
    pub runtime_minutes: Option<i64>,
    pub genres: Vec<String>,
    pub age_rating: Option<String>,
    pub country: Option<String>,
    pub studio: Option<String>,
    pub director: Option<String>,
    /// "2016-11-11", as the file has it.
    pub premiered: Option<String>,
    /// On a 10-point scale.
    pub rating: Option<f64>,
    pub tmdb_id: Option<String>,
    pub imdb_id: Option<String>,
    pub cast: Vec<CastMember>,
}

static IMDB_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(tt\d{6,10})\b").expect("built-in pattern"));
static TMDB_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"themoviedb\.org/(?:movie|tv)/(\d+)").expect("built-in pattern"));

/// A rating in `<ratings>`, kept until its value arrives.
#[derive(Debug, Default)]
struct RatingSource {
    max: Option<f64>,
    default: bool,
}

impl Nfo {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// What a file says, or `None` when it says nothing usable. What was
    /// read before an XML error is kept: many files have a link after the XML.
    pub fn parse(bytes: &[u8]) -> Option<Nfo> {
        let mut nfo = Nfo::default();
        let mut reader = Reader::from_reader(bytes);
        reader.config_mut().check_end_names = false;
        let mut buffer = Vec::new();
        // Element names from the root down, lowercased.
        let mut path: Vec<String> = Vec::new();
        let mut text = String::new();
        let mut unique_id_type: Option<String> = None;
        let mut actor: Option<CastMember> = None;
        let mut rating_source: Option<RatingSource> = None;
        // The default rating in <ratings>, else the first one there.
        let mut chosen_rating: Option<(f64, bool)> = None;
        loop {
            let event = match reader.read_event_into(&mut buffer) {
                Ok(Event::Eof) | Err(_) => break,
                Ok(event) => event,
            };
            match event {
                Event::Start(element) => {
                    path.push(local_name(&element));
                    text.clear();
                    start(&reader, &element, &path, &mut unique_id_type, &mut actor, &mut rating_source);
                }
                Event::Text(content) => text.push_str(&unescaped(&content)),
                Event::CData(content) => text.push_str(&String::from_utf8_lossy(&content)),
                Event::End(_) => {
                    let value = text.trim().to_string();
                    text.clear();
                    end(&mut nfo, &path, value, &mut unique_id_type, &mut actor, &mut rating_source, &mut chosen_rating);
                    path.pop();
                }
                _ => {}
            }
            buffer.clear();
        }
        if nfo.rating.is_none() {
            nfo.rating = chosen_rating.map(|(value, _)| value);
        }
        let raw = String::from_utf8_lossy(bytes);
        if nfo.imdb_id.is_none() {
            nfo.imdb_id = IMDB_ID.captures(&raw).map(|found| found[1].to_string());
        }
        if nfo.tmdb_id.is_none() {
            nfo.tmdb_id = TMDB_LINK.captures(&raw).map(|found| found[1].to_string());
        }
        if nfo.year.is_none() {
            nfo.year = nfo.premiered.as_deref().and_then(|date| date.get(..4)?.parse().ok()).filter(|year| *year > 1880);
        }
        let mut seen = std::collections::HashSet::new();
        nfo.genres.retain(|genre| seen.insert(genre.to_lowercase()));
        (!nfo.is_empty()).then_some(nfo)
    }

    /// Genres as one line ("Crime, Drama"), as lists carry them.
    pub fn genre(&self) -> Option<String> {
        (!self.genres.is_empty()).then(|| self.genres.join(", "))
    }

    pub fn duration_seconds(&self) -> Option<i64> {
        self.runtime_minutes.map(|minutes| minutes * 60)
    }

    /// The day an episode first aired, as midday UTC so it reads as that day everywhere.
    pub fn air_date(&self) -> Option<Timestamp> {
        let day = chrono::NaiveDate::parse_from_str(self.premiered.as_deref()?.get(..10)?, "%Y-%m-%d").ok()?;
        Some(day.and_hms_opt(12, 0, 0)?.and_utc().timestamp())
    }

    /// The details this file adds to a title's page. Actors go in as names;
    /// as people only when the file has their photos, so it doesn't hide
    /// TMDB's faces behind plain names.
    pub fn details(&self) -> DetailsRecord {
        let has_photos = self.cast.iter().any(|member| member.photo_url.is_some());
        DetailsRecord {
            overview: self.plot.clone(),
            tagline: self.tagline.clone(),
            original_title: self.original_title.clone().filter(|original| Some(original) != self.title.as_ref()),
            runtime_minutes: self.runtime_minutes,
            genres: self.genres.clone(),
            age_rating: self.age_rating.clone(),
            country: self.country.clone(),
            network: self.studio.clone(),
            director: self.director.clone(),
            release_date: self.premiered.clone(),
            imdb_id: self.imdb_id.clone(),
            cast: self.cast.iter().map(|member| member.name.clone()).collect(),
            people: if has_photos { self.cast.clone() } else { Vec::new() },
            ..Default::default()
        }
    }
}

fn local_name(element: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(element.local_name().as_ref()).to_lowercase()
}

fn attribute<R>(reader: &Reader<R>, element: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    element.attributes().flatten().find(|attr| attr.key.local_name().as_ref() == name).map(|attr| {
        attr.decode_and_unescape_value(reader.decoder())
            .map(|value| value.into_owned())
            .unwrap_or_else(|_| String::from_utf8_lossy(&attr.value).into_owned())
    })
}

fn start<R>(
    reader: &Reader<R>,
    element: &BytesStart<'_>,
    path: &[String],
    unique_id_type: &mut Option<String>,
    actor: &mut Option<CastMember>,
    rating_source: &mut Option<RatingSource>,
) {
    match path {
        [_, child] if child == "uniqueid" => *unique_id_type = attribute(reader, element, b"type").map(|kind| kind.to_lowercase()),
        [_, child] if child == "actor" => *actor = Some(CastMember::default()),
        [_, ratings, rating] if ratings == "ratings" && rating == "rating" => {
            *rating_source = Some(RatingSource {
                max: attribute(reader, element, b"max").and_then(|max| max.trim().parse().ok()),
                default: attribute(reader, element, b"default").is_some_and(|value| value.eq_ignore_ascii_case("true")),
            });
        }
        _ => {}
    }
}

/// Takes in an element's text as it closes. Only the root's own children
/// count (and each `<actor>`'s name, role and photo), so a `<set><name>` or
/// a stream's details never pass for the title's.
fn end(
    nfo: &mut Nfo,
    path: &[String],
    value: String,
    unique_id_type: &mut Option<String>,
    actor: &mut Option<CastMember>,
    rating_source: &mut Option<RatingSource>,
    chosen_rating: &mut Option<(f64, bool)>,
) {
    match path {
        [_, parent, child] if parent == "actor" => {
            if let Some(member) = actor.as_mut() {
                match child.as_str() {
                    "name" => member.name = value,
                    "role" => member.role = (!value.is_empty()).then_some(value),
                    "thumb" => member.photo_url = web_link(&value),
                    _ => {}
                }
            }
        }
        [_, ratings, rating, part] if ratings == "ratings" && rating == "rating" && part == "value" => {
            let Some(source) = rating_source.as_ref() else { return };
            let Ok(raw) = value.parse::<f64>() else { return };
            let scaled = match source.max {
                Some(max) if max > 0.0 => raw * 10.0 / max,
                _ => raw,
            };
            if (0.0..=10.0).contains(&scaled) && (chosen_rating.is_none() || (source.default && !chosen_rating.is_some_and(|(_, default)| default))) {
                *chosen_rating = Some((scaled, source.default));
            }
        }
        [_, ratings, rating] if ratings == "ratings" && rating == "rating" => *rating_source = None,
        [_, child] => {
            if child == "actor" {
                if let Some(member) = actor.take()
                    && !member.name.is_empty()
                    && nfo.cast.len() < MOST_CAST
                {
                    nfo.cast.push(member);
                }
                return;
            }
            let kind = unique_id_type.take();
            if value.is_empty() {
                return;
            }
            take(nfo, child, value, kind.as_deref());
        }
        _ => {}
    }
}

/// One of the root's own children. The first of each wins: a file holding
/// several episodes' details describes the first.
fn take(nfo: &mut Nfo, name: &str, value: String, unique_id_type: Option<&str>) {
    fn first<T>(slot: &mut Option<T>, value: Option<T>) {
        if slot.is_none() {
            *slot = value;
        }
    }
    match name {
        "title" => first(&mut nfo.title, Some(value)),
        "originaltitle" => first(&mut nfo.original_title, Some(value)),
        "year" => first(&mut nfo.year, value.parse().ok().filter(|year| *year > 1880)),
        "plot" => first(&mut nfo.plot, Some(value)),
        "outline" if nfo.plot.is_none() => nfo.plot = Some(value),
        "tagline" => first(&mut nfo.tagline, Some(value)),
        "runtime" => first(&mut nfo.runtime_minutes, value.parse().ok().filter(|minutes| *minutes > 0 && *minutes < 1000)),
        "genre" => nfo.genres.extend(value.split('/').map(str::trim).filter(|genre| !genre.is_empty()).map(str::to_string)),
        "mpaa" | "certification" => first(&mut nfo.age_rating, age_rating(&value)),
        "country" => first(&mut nfo.country, Some(value)),
        "studio" => first(&mut nfo.studio, Some(value)),
        "director" => first(&mut nfo.director, Some(value)),
        "premiered" | "aired" | "releasedate" => first(&mut nfo.premiered, Some(value)),
        "rating" => first(&mut nfo.rating, value.parse().ok().filter(|rating| (0.0..=10.0).contains(rating))),
        "tmdbid" => first(&mut nfo.tmdb_id, numeric(&value)),
        "imdbid" => first(&mut nfo.imdb_id, imdb(&value)),
        "id" => first(&mut nfo.imdb_id, imdb(&value)),
        "uniqueid" => match unique_id_type {
            Some("tmdb") => first(&mut nfo.tmdb_id, numeric(&value)),
            Some("imdb") => first(&mut nfo.imdb_id, imdb(&value)),
            // Kodi's own default when no type is given.
            None => first(&mut nfo.imdb_id, imdb(&value)),
            _ => {}
        },
        _ => {}
    }
}

fn numeric(value: &str) -> Option<String> {
    value.trim().parse::<u64>().ok().map(|number| number.to_string())
}

fn imdb(value: &str) -> Option<String> {
    let value = value.trim();
    (value.starts_with("tt") && value.len() > 2 && value[2..].chars().all(|c| c.is_ascii_digit())).then(|| value.to_string())
}

/// "Rated PG-13", "US:PG-13" and "PG-13" are all "PG-13".
fn age_rating(text: &str) -> Option<String> {
    let rating = text.rsplit(':').next().unwrap_or(text).trim();
    let rating = rating.strip_prefix("Rated ").or_else(|| rating.strip_prefix("rated ")).unwrap_or(rating).trim();
    (!rating.is_empty()).then(|| rating.to_string())
}

/// A photo at a web address; a path on someone else's computer is no use here.
fn web_link(text: &str) -> Option<String> {
    url::Url::parse(text.trim()).ok().filter(|link| matches!(link.scheme(), "http" | "https")).map(|link| link.to_string())
}

/// Element text with entities resolved; HTML ones XML doesn't define are kept as written.
fn unescaped(content: &BytesText<'_>) -> String {
    content
        .unescape_with(|entity| match entity {
            _ if quick_xml::escape::resolve_predefined_entity(entity).is_some() => quick_xml::escape::resolve_predefined_entity(entity),
            "nbsp" => Some("\u{a0}"),
            "ndash" => Some("–"),
            "mdash" => Some("—"),
            "hellip" => Some("…"),
            _ => None,
        })
        .map(|text| text.into_owned())
        .unwrap_or_else(|_| String::from_utf8_lossy(content).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfo_files_from_media_managers() {
        let kodi = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?>
<movie>
    <title>Heat</title>
    <originaltitle>Heat</originaltitle>
    <year>1995</year>
    <plot><![CDATA[A group of professional bank robbers start to feel the heat.]]></plot>
    <runtime>170</runtime>
    <genre>Crime</genre>
    <genre>Drama / Thriller</genre>
    <mpaa>Rated R</mpaa>
    <ratings>
        <rating name="themoviedb" max="10"><value>7.9</value></rating>
        <rating name="imdb" max="10" default="true"><value>8.3</value><votes>700000</votes></rating>
    </ratings>
    <uniqueid type="imdb">tt0113277</uniqueid>
    <uniqueid type="tmdb" default="true">949</uniqueid>
    <actor><name>Al Pacino</name><role>Lt. Vincent Hanna</role><thumb>https://image.tmdb.org/t/p/original/a.jpg</thumb></actor>
    <actor><name>Robert De Niro</name><role>Neil McCauley</role><thumb>/local/path.jpg</thumb></actor>
    <set><name>Not a title</name></set>
    <fileinfo><streamdetails><video><codec>hevc</codec></video></streamdetails></fileinfo>
</movie>
https://www.themoviedb.org/movie/949
"#;
        let nfo = Nfo::parse(kodi.as_bytes()).unwrap();
        assert_eq!(nfo.title.as_deref(), Some("Heat"));
        assert_eq!(nfo.year, Some(1995));
        assert_eq!(nfo.plot.as_deref(), Some("A group of professional bank robbers start to feel the heat."));
        assert_eq!(nfo.runtime_minutes, Some(170));
        assert_eq!(nfo.genres, ["Crime", "Drama", "Thriller"]);
        assert_eq!(nfo.age_rating.as_deref(), Some("R"));
        assert_eq!(nfo.rating, Some(8.3), "the default rating");
        assert_eq!(nfo.tmdb_id.as_deref(), Some("949"));
        assert_eq!(nfo.imdb_id.as_deref(), Some("tt0113277"));
        assert_eq!(nfo.cast.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Al Pacino", "Robert De Niro"]);
        assert_eq!(nfo.cast[0].photo_url.as_deref(), Some("https://image.tmdb.org/t/p/original/a.jpg"));
        assert_eq!(nfo.cast[1].photo_url, None, "a path on another computer isn't a photo");
        let details = nfo.details();
        assert_eq!(details.original_title, None, "the same as the title");
        assert_eq!(details.overview, nfo.plot);
        assert_eq!(details.cast, ["Al Pacino", "Robert De Niro"]);
        assert_eq!(details.people.len(), 2, "with photos, the actors are people");
    }

    #[test]
    fn older_and_shorter_files() {
        let old = "<movie><title>Arrival</title><rating>7.9</rating><id>tt2543164</id><premiered>2016-11-11</premiered>\
                   <tmdbid>329865</tmdbid><outline>Short.</outline><actor><name>Amy Adams</name></actor></movie>";
        let nfo = Nfo::parse(old.as_bytes()).unwrap();
        assert_eq!((nfo.rating, nfo.year), (Some(7.9), Some(2016)), "the year from the release date");
        assert_eq!(nfo.imdb_id.as_deref(), Some("tt2543164"));
        assert_eq!(nfo.tmdb_id.as_deref(), Some("329865"));
        assert_eq!(nfo.plot.as_deref(), Some("Short."), "an outline when there's no plot");
        assert!(nfo.details().people.is_empty(), "names alone leave TMDB's faces showing");

        let episode = "<episodedetails><title>Half Loop</title><aired>2022-02-18</aired><runtime>55</runtime></episodedetails>";
        let nfo = Nfo::parse(episode.as_bytes()).unwrap();
        assert_eq!(nfo.title.as_deref(), Some("Half Loop"));
        assert_eq!(nfo.air_date(), Some(chrono::NaiveDate::from_ymd_opt(2022, 2, 18).unwrap().and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp()));
        assert_eq!(nfo.duration_seconds(), Some(3300));

        let scaled = r#"<movie><ratings><rating name="x" max="100"><value>85</value></rating></ratings></movie>"#;
        assert_eq!(Nfo::parse(scaled.as_bytes()).unwrap().rating, Some(8.5));

        let entities = "<tvshow><title>Law &amp; Order</title><studio>NBC&nbsp;</studio><mpaa>US:TV-14</mpaa></tvshow>";
        let nfo = Nfo::parse(entities.as_bytes()).unwrap();
        assert_eq!(nfo.title.as_deref(), Some("Law & Order"));
        assert_eq!(nfo.age_rating.as_deref(), Some("TV-14"));
    }

    #[test]
    fn links_alone_and_files_that_say_nothing() {
        let link = Nfo::parse(b"https://www.imdb.com/title/tt1856101/\n").unwrap();
        assert_eq!(link.imdb_id.as_deref(), Some("tt1856101"));
        assert_eq!(link.title, None);
        let tmdb = Nfo::parse(b"https://www.themoviedb.org/tv/95396-severance").unwrap();
        assert_eq!(tmdb.tmdb_id.as_deref(), Some("95396"));
        assert_eq!(Nfo::parse(b"just some notes"), None);
        assert_eq!(Nfo::parse(b"<movie><set><name>Only a set</name></set></movie>"), None);
        assert_eq!(Nfo::parse(b"<movie><title>Cut off"), None, "an unclosed title says nothing");
    }
}
