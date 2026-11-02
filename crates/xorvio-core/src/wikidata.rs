//! Filmographies from Wikidata (CC0, and no key needed), for people's pages
//! when TMDB can't help: without a TMDB key, or when TMDB doesn't know
//! someone. Only the person's name is sent, never for someone from the demo
//! library. Wikidata's credits carry TMDB and IMDb ids where it has them,
//! which is how the titles are found in the catalog; the rest are matched
//! by name and year.
//!
//! What Wikidata says about a name (everyone called that, with their work)
//! is kept in the catalog for a month, a name it doesn't know for two
//! weeks, and requests go one at a time, a second apart, as Wikimedia asks
//! of clients.
//!
//! A person's photo is from Wikimedia Commons, whose licences mostly ask for
//! credit, so Commons is asked who took it and under what licence (only the
//! file's name is sent). A photo whose credit can't be given isn't shown.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use xorvio_model::{DEMO_SOURCE_ID, Movie, PersonDetails, Series};

use crate::details_api::ProviderTurns;
use crate::error::SourceError;
use crate::http::ApiCall;
use crate::json::{self, Object};
use crate::names;
use crate::sources::ids;
use crate::store::catalog::{MatchKeys, title_key};
use crate::store::lock;
use crate::{Core, Result, now};

/// Where Wikidata's query service is. Tests use a stand-in, and anything
/// that forgets to set one reaches nothing.
#[cfg(not(test))]
pub(crate) const DEFAULT_BASE: &str = "https://query.wikidata.org";
#[cfg(test)]
pub(crate) const DEFAULT_BASE: &str = "http://127.0.0.1:9";
/// Where Wikimedia Commons' API is, for photos' credits.
#[cfg(not(test))]
const COMMONS_BASE: &str = "https://commons.wikimedia.org";
#[cfg(test)]
const COMMONS_BASE: &str = "http://127.0.0.1:9";
/// The longest credit shown: a few names and a licence.
const LONGEST_CREDIT: usize = 120;

const KEEP_FOR: i64 = 30 * 86_400;
const RETRY_MISSES_AFTER: i64 = 14 * 86_400;
const RETRY_FAILURES_AFTER: Duration = Duration::from_secs(10 * 60);
const MOST_CREDITS: usize = 60;
/// Wikimedia asks every client to say what it is.
const USER_AGENT: &str = concat!("Xorvio/", env!("CARGO_PKG_VERSION"), " (open-source media player; people's filmographies)");

pub(crate) struct Wikidata {
    base: Mutex<String>,
    commons_base: Mutex<String>,
    turns: ProviderTurns,
    /// Names Wikidata couldn't be asked about lately, and when.
    failures: Mutex<HashMap<String, Instant>>,
    /// Photos' credits asked of Commons this session, by file name; `None`
    /// is a photo that can't be credited.
    credits: Mutex<HashMap<String, Option<PhotoCredit>>>,
}

impl Wikidata {
    pub fn new() -> Self {
        Self {
            base: Mutex::new(DEFAULT_BASE.into()),
            commons_base: Mutex::new(COMMONS_BASE.into()),
            turns: ProviderTurns::with(1, Duration::from_secs(1)),
            failures: Mutex::default(),
            credits: Mutex::default(),
        }
    }
}

/// Someone Wikidata knows, as kept.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Person {
    /// Wikidata's id ("Q43416").
    id: String,
    /// "Canadian actor (born 1964)".
    summary: Option<String>,
    photo_url: Option<String>,
    birthday: Option<String>,
    deathday: Option<String>,
    works: Vec<Work>,
}

/// Who took a Commons photo and under what licence, and its page there.
#[derive(Debug, Clone, PartialEq)]
struct PhotoCredit {
    /// "Gage Skidmore, CC BY-SA 3.0".
    text: String,
    page: String,
}

/// A film or series someone acted in or directed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Work {
    title: String,
    year: Option<i64>,
    /// `None` when Wikidata doesn't say which.
    series: Option<bool>,
    tmdb_id: Option<i64>,
    imdb_id: Option<String>,
}

impl Work {
    /// Whether a catalog title is this work: by TMDB or IMDb id, else (when
    /// the title has no TMDB id of its own to say otherwise) by name and a
    /// year within one.
    fn is(&self, tmdb_id: Option<i64>, imdb_id: Option<&str>, key: &str, year: Option<i64>) -> bool {
        if self.tmdb_id.is_some() && tmdb_id == self.tmdb_id {
            return true;
        }
        if self.imdb_id.is_some() && imdb_id == self.imdb_id.as_deref() {
            return true;
        }
        let year_fits = self.year.zip(year).is_none_or(|(ours, theirs)| (ours - theirs).abs() <= 1);
        (tmdb_id.is_none() || self.tmdb_id.is_none()) && !key.is_empty() && names::match_key(&self.title) == key && year_fits
    }
}

impl Core {
    #[cfg(test)]
    pub(crate) fn set_wikidata_base(&self, base: &str) {
        *lock(&self.wikidata.base) = base.to_string();
        *lock(&self.wikidata.commons_base) = base.to_string();
    }

    /// Someone's page from Wikidata: a line about them, their photo, and
    /// their films and series the sources carry, newest first. When several
    /// people share the name, the one credited in the title they were
    /// chosen from wins, then the one with the most work. `None` when
    /// Wikidata doesn't know them or can't be asked.
    pub(crate) async fn wikidata_person(&self, name: &str, item_id: Option<&str>) -> Result<Option<PersonDetails>> {
        if item_id.is_some_and(|id| ids::source_of(id) == DEMO_SOURCE_ID) {
            return Ok(None);
        }
        let name = name.trim();
        let name_key = names::match_key(name);
        if name_key.is_empty() {
            return Ok(None);
        }
        let Some(people) = self.wikidata_people(name, &name_key).await else { return Ok(None) };
        let known_for = item_id.map(|id| self.ids_of_item(id)).unwrap_or_default();
        let chosen = people
            .iter()
            .find(|person| {
                person.works.iter().any(|work| {
                    known_for.0.is_some_and(|id| work.tmdb_id == Some(id)) || known_for.1.as_ref().is_some_and(|id| work.imdb_id.as_ref() == Some(id))
                })
            })
            .or_else(|| people.iter().max_by_key(|person| person.works.len()));
        let Some(person) = chosen else { return Ok(None) };

        let mut works = person.works.clone();
        works.sort_by_key(|work| std::cmp::Reverse(work.year));
        let sources: Vec<String> = self.active_source_ids().into_iter().filter(|id| id != DEMO_SOURCE_ID).collect();
        let keys = |series: bool| -> MatchKeys {
            let mine: Vec<&Work> = works.iter().filter(|work| work.series.is_none_or(|is| is == series)).collect();
            let mut title_keys: Vec<String> = mine.iter().map(|work| names::match_key(&work.title)).filter(|key| !key.is_empty()).collect();
            title_keys.sort();
            title_keys.dedup();
            MatchKeys {
                imdb: mine.iter().filter_map(|work| work.imdb_id.clone()).collect(),
                tmdb: mine.iter().filter_map(|work| work.tmdb_id.map(|id| id.to_string())).collect(),
                title_keys,
            }
        };
        let movies = self.visible_movies(self.catalog.movies_matching(&sources, &keys(false))?)?;
        let series = self.visible_series(self.catalog.series_matching(&sources, &keys(true))?)?;
        let movies = credited(&works, false, movies, &sources);
        let series = credited(&works, true, series, &sources);
        let credit = match &person.photo_url {
            Some(photo) => self.photo_credit(photo).await,
            None => None,
        };
        Ok(Some(PersonDetails {
            tmdb_id: String::new(),
            name: name.to_string(),
            // Shown only with its credit.
            photo_url: credit.as_ref().and(person.photo_url.clone()),
            photo_credit: credit.as_ref().map(|credit| credit.text.clone()),
            photo_page: credit.map(|credit| credit.page),
            biography: None,
            department: None,
            birthday: person.birthday.clone(),
            deathday: person.deathday.clone(),
            place_of_birth: None,
            movies: self.present_movies(movies)?,
            series: self.present_series_list(series)?,
            summary: person.summary.clone(),
            attribution: Some("Wikidata".into()),
        }))
    }

    /// The TMDB and IMDb ids of a catalog title (or an add-on's, by its id).
    fn ids_of_item(&self, item_id: &str) -> (Option<i64>, Option<String>) {
        let ids = [item_id.to_string()];
        let parse = |id: Option<String>| id.and_then(|id| id.trim().parse::<i64>().ok()).filter(|id| *id > 0);
        if let Ok(Some(movie)) = self.catalog.movies_by_ids(&ids).map(|mut found| found.pop()) {
            return (parse(movie.tmdb_id), movie.imdb_id);
        }
        if let Ok(Some(series)) = self.catalog.series_by_ids(&ids).map(|mut found| found.pop()) {
            return (parse(series.tmdb_id), series.imdb_id);
        }
        let imdb = item_id.strip_prefix("addon|").and_then(|rest| rest.split('|').nth(1)).map(|id| id.split(':').next().unwrap_or(id).to_string());
        (None, imdb.filter(|id| id.starts_with("tt")))
    }

    /// Everyone Wikidata has by this name, as kept while that's fresh, else
    /// asked and kept; what was kept before when Wikidata can't be asked.
    /// `None` when there's nothing to go on.
    async fn wikidata_people(&self, name: &str, name_key: &str) -> Option<Vec<Person>> {
        let kept = match self.catalog.wikidata_people(name_key) {
            Ok(kept) => kept.and_then(|(text, at)| Some((serde_json::from_str::<Vec<Person>>(&text).ok()?, at))),
            Err(error) => {
                tracing::warn!("what Wikidata said could not be read: {error}");
                None
            }
        };
        let kept = match kept {
            Some((people, at)) if now() - at < if people.is_empty() { RETRY_MISSES_AFTER } else { KEEP_FOR } => return Some(people),
            kept => kept.map(|(people, _)| people),
        };
        if lock(&self.wikidata.failures).get(name_key).is_some_and(|at| at.elapsed() < RETRY_FAILURES_AFTER) {
            return kept;
        }
        let asked = {
            let _turn = self.wikidata.turns.wait().await;
            self.ask_wikidata(name).await
        };
        let people = match asked {
            Ok(people) => people,
            Err(error) => {
                tracing::debug!("Wikidata could not be asked about someone: {error}");
                lock(&self.wikidata.failures).insert(name_key.to_string(), Instant::now());
                return kept;
            }
        };
        let saved = serde_json::to_string(&people)
            .map_err(anyhow::Error::from)
            .and_then(|text| self.catalog.save_wikidata_people(name_key, &text, now()));
        if let Err(error) = saved {
            tracing::warn!("what Wikidata said could not be saved: {error}");
        }
        Some(people)
    }

    /// Who took a Commons photo and under what licence, asked once a
    /// session; `None` when Commons can't say (or can't be reached), so the
    /// photo isn't shown.
    async fn photo_credit(&self, photo: &str) -> Option<PhotoCredit> {
        let file = commons_file(photo)?;
        if let Some(known) = lock(&self.wikidata.credits).get(&file) {
            return known.clone();
        }
        let base = lock(&self.wikidata.commons_base).clone();
        let mut link = url::Url::parse(&format!("{}/w/api.php", base.trim_end_matches('/'))).ok()?;
        link.query_pairs_mut()
            .append_pair("action", "query")
            .append_pair("format", "json")
            .append_pair("formatversion", "2")
            .append_pair("prop", "imageinfo")
            .append_pair("iiprop", "extmetadata|url")
            .append_pair("titles", &format!("File:{file}"));
        let headers = [("User-Agent".to_string(), USER_AGENT.to_string())];
        let reply = {
            let _turn = self.wikidata.turns.wait().await;
            self.http.api_json(ApiCall::get(link.as_str(), &headers)).await
        };
        match reply {
            Ok(reply) => {
                let credit = parse_credit(&reply);
                lock(&self.wikidata.credits).insert(file, credit.clone());
                credit
            }
            Err(error) => {
                tracing::debug!("Commons could not be asked about a photo: {error}");
                None
            }
        }
    }

    async fn ask_wikidata(&self, name: &str) -> Result<Vec<Person>, SourceError> {
        let base = lock(&self.wikidata.base).clone();
        let mut link = url::Url::parse(&format!("{}/sparql", base.trim_end_matches('/')))
            .map_err(|_| SourceError::Unsupported("Wikidata's address isn't valid.".into()))?;
        link.query_pairs_mut().append_pair("query", &credits_query(name)).append_pair("format", "json");
        let headers = [
            ("Accept".to_string(), "application/sparql-results+json".to_string()),
            ("User-Agent".to_string(), USER_AGENT.to_string()),
        ];
        let reply = self.http.api_json(ApiCall::get(link.as_str(), &headers)).await?;
        Ok(parse_people(&reply))
    }
}

/// The SPARQL for everyone labelled `name` in English and what they acted
/// in or directed, with the ids that find each work.
fn credits_query(name: &str) -> String {
    let label: String = name.chars().filter(|c| !c.is_control()).collect::<String>().replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"SELECT ?person ?summary ?image ?born ?died ?work ?workLabel ?tmdbMovie ?tmdbTV ?imdb (MIN(?date) AS ?first) WHERE {{
  ?person wdt:P31 wd:Q5; rdfs:label "{label}"@en.
  {{ ?work wdt:P161 ?person. }} UNION {{ ?work wdt:P57 ?person. }}
  OPTIONAL {{ ?person schema:description ?summary. FILTER(LANG(?summary) = "en") }}
  OPTIONAL {{ ?person wdt:P18 ?image. }}
  OPTIONAL {{ ?person wdt:P569 ?born. }}
  OPTIONAL {{ ?person wdt:P570 ?died. }}
  OPTIONAL {{ ?work wdt:P4947 ?tmdbMovie. }}
  OPTIONAL {{ ?work wdt:P4983 ?tmdbTV. }}
  OPTIONAL {{ ?work wdt:P345 ?imdb. }}
  OPTIONAL {{ ?work wdt:P577 ?date. }}
  SERVICE wikibase:label {{ bd:serviceParam wikibase:language "en". }}
}}
GROUP BY ?person ?summary ?image ?born ?died ?work ?workLabel ?tmdbMovie ?tmdbTV ?imdb
LIMIT 1500"#
    )
}

/// The query's rows, one person each with their works once.
fn parse_people(reply: &Value) -> Vec<Person> {
    let bindings = reply.get("results").and_then(|results| results.get("bindings"));
    let rows: Vec<&Object> = json::objects(bindings);
    let value = |row: &Object, key: &str| -> Option<String> {
        row.get(key).and_then(|cell| cell.get("value")).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty()).map(str::to_string)
    };
    let mut people: Vec<Person> = Vec::new();
    let mut seen: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let Some(uri) = value(row, "person") else { continue };
        let id = uri.rsplit('/').next().unwrap_or(&uri).to_string();
        let at = match people.iter().position(|person| person.id == id) {
            Some(at) => at,
            None => {
                people.push(Person {
                    id: id.clone(),
                    summary: value(row, "summary"),
                    photo_url: value(row, "image").and_then(|image| photo(&image)),
                    birthday: value(row, "born").and_then(|date| day(&date)),
                    deathday: value(row, "died").and_then(|date| day(&date)),
                    works: Vec::new(),
                });
                people.len() - 1
            }
        };
        // Works without an English label come back as their bare id ("Q123456").
        let Some(title) = value(row, "workLabel").filter(|title| !is_entity_id(title)) else { continue };
        let tmdb_tv = value(row, "tmdbTV").and_then(|id| id.parse::<i64>().ok());
        let tmdb_movie = value(row, "tmdbMovie").and_then(|id| id.parse::<i64>().ok());
        let series = if tmdb_tv.is_some() { Some(true) } else { tmdb_movie.map(|_| false) };
        let key = match (tmdb_tv, tmdb_movie) {
            (Some(id), _) => format!("tv/{id}"),
            (_, Some(id)) => format!("movie/{id}"),
            _ => value(row, "work").unwrap_or_else(|| title.clone()),
        };
        let known = seen.entry(id).or_default();
        if known.contains(&key) {
            continue;
        }
        known.push(key);
        people[at].works.push(Work {
            title,
            year: value(row, "first").and_then(|date| date.get(..4)?.parse().ok()),
            series,
            tmdb_id: tmdb_tv.or(tmdb_movie),
            imdb_id: value(row, "imdb").filter(|id| id.starts_with("tt")),
        });
    }
    people
}

fn is_entity_id(text: &str) -> bool {
    text.len() > 1 && text.starts_with('Q') && text[1..].chars().all(|c| c.is_ascii_digit())
}

/// A Commons picture ("http://commons.wikimedia.org/wiki/Special:FilePath/…")
/// over https, at a size for a page.
fn photo(link: &str) -> Option<String> {
    let link = link.strip_prefix("http://").map_or_else(|| link.to_string(), |rest| format!("https://{rest}"));
    let parsed = url::Url::parse(&link).ok()?;
    (parsed.scheme() == "https" && parsed.host_str().is_some_and(|host| host.ends_with("wikimedia.org"))).then(|| format!("{link}?width=500"))
}

/// The file a Commons photo address names ("…/Special:FilePath/Keanu%20Reeves.jpg?width=500" → "Keanu Reeves.jpg").
fn commons_file(photo: &str) -> Option<String> {
    let parsed = url::Url::parse(photo).ok()?;
    let encoded = parsed.path().split_once("Special:FilePath/")?.1;
    let file = percent_encoding::percent_decode_str(encoded).decode_utf8().ok()?.replace('_', " ");
    (!file.trim().is_empty() && !file.contains('/')).then_some(file)
}

/// The credit Commons' file information gives: who made it and the licence,
/// or `None` when credit is asked for and Commons doesn't say whom.
fn parse_credit(reply: &Value) -> Option<PhotoCredit> {
    let page = reply["query"]["pages"].as_array()?.first()?;
    let info = page["imageinfo"].as_array()?.first()?;
    let meta = &info["extmetadata"];
    let field = |name: &str| meta[name]["value"].as_str().map(plain_text).filter(|text| !text.is_empty());
    let licence = field("LicenseShortName")?;
    let needs_credit = field("AttributionRequired").is_none_or(|required| required != "false");
    let author = field("Artist");
    if needs_credit && author.is_none() {
        return None;
    }
    let mut text = match author {
        Some(author) => format!("{author}, {licence}"),
        None => licence,
    };
    if text.chars().count() > LONGEST_CREDIT {
        text = text.chars().take(LONGEST_CREDIT - 1).collect::<String>().trim_end().to_string() + "…";
    }
    let page = info["descriptionurl"].as_str().filter(|link| link.starts_with("https://"))?.to_string();
    Some(PhotoCredit { text, page })
}

/// Commons' HTML fields as words: tags dropped, entities read, spaces
/// joined ("<a href=…>Gage Skidmore</a>" → "Gage Skidmore").
fn plain_text(html: &str) -> String {
    let mut words = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                words.push(' ');
            }
            c if !in_tag => words.push(c),
            _ => {}
        }
    }
    let words = words.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ");
    words.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "1964-09-02T00:00:00Z" → "1964-09-02".
fn day(date: &str) -> Option<String> {
    let day = date.get(..10)?;
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok().map(|_| day.to_string())
}

/// What a [`credited`] pick needs of a film or series.
trait Credited {
    fn source_id(&self) -> &str;
    fn ids(&self) -> (Option<i64>, Option<&str>);
    fn title(&self) -> &str;
    fn year(&self) -> Option<i64>;
}

impl Credited for Movie {
    fn source_id(&self) -> &str {
        &self.source_id
    }
    fn ids(&self) -> (Option<i64>, Option<&str>) {
        (self.tmdb_id.as_deref().and_then(|id| id.trim().parse().ok()), self.imdb_id.as_deref())
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn year(&self) -> Option<i64> {
        self.year
    }
}

impl Credited for Series {
    fn source_id(&self) -> &str {
        &self.source_id
    }
    fn ids(&self) -> (Option<i64>, Option<&str>) {
        (self.tmdb_id.as_deref().and_then(|id| id.trim().parse().ok()), self.imdb_id.as_deref())
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn year(&self) -> Option<i64> {
        self.year
    }
}

/// A catalog title for each work of this kind, in the works' order, from
/// the earliest source that has it, each work (and each title) once.
fn credited<T: Credited>(works: &[Work], series: bool, mut titles: Vec<T>, sources: &[String]) -> Vec<T> {
    titles.sort_by_key(|title| sources.iter().position(|source| source == title.source_id()).unwrap_or(usize::MAX));
    let keys: Vec<String> = titles.iter().map(|title| title_key(title.title())).collect();
    let mut used = vec![false; titles.len()];
    let mut chosen = Vec::new();
    for work in works.iter().filter(|work| work.series.is_none_or(|is| is == series)) {
        if chosen.len() == MOST_CREDITS {
            break;
        }
        let is_it = |index: usize| {
            let (tmdb, imdb) = titles[index].ids();
            work.is(tmdb, imdb, &keys[index], titles[index].year())
        };
        let Some(first) = (0..titles.len()).find(|&index| !used[index] && is_it(index)) else { continue };
        // Every copy of the work is spoken for.
        for (index, taken) in used.iter_mut().enumerate() {
            *taken |= is_it(index);
        }
        chosen.push(first);
    }
    let mut slots: Vec<Option<T>> = titles.into_iter().map(Some).collect();
    chosen.into_iter().filter_map(|index| slots[index].take()).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_reply_reads_into_people_and_their_work() {
        let cell = |value: &str| json!({"type": "literal", "value": value});
        let reply = json!({"results": {"bindings": [
            {"person": cell("http://www.wikidata.org/entity/Q43416"), "summary": cell("Canadian actor"),
             "image": cell("http://commons.wikimedia.org/wiki/Special:FilePath/Keanu.jpg"), "born": cell("1964-09-02T00:00:00Z"),
             "work": cell("http://www.wikidata.org/entity/Q83495"), "workLabel": cell("The Matrix"), "tmdbMovie": cell("603"),
             "imdb": cell("tt0133093"), "first": cell("1999-03-31T00:00:00Z")},
            {"person": cell("http://www.wikidata.org/entity/Q43416"), "work": cell("http://www.wikidata.org/entity/Q83495"),
             "workLabel": cell("The Matrix"), "tmdbMovie": cell("603")},
            {"person": cell("http://www.wikidata.org/entity/Q43416"), "work": cell("http://www.wikidata.org/entity/Q1"),
             "workLabel": cell("Q99999")},
            {"person": cell("http://www.wikidata.org/entity/Q43416"), "work": cell("http://www.wikidata.org/entity/Q2"),
             "workLabel": cell("Harbor Lights"), "tmdbTV": cell("1399")},
            {"person": cell("http://www.wikidata.org/entity/Q7"), "work": cell("http://www.wikidata.org/entity/Q3"),
             "workLabel": cell("Something Else")}
        ]}});
        let people = parse_people(&reply);
        assert_eq!(people.len(), 2);
        let keanu = &people[0];
        assert_eq!((keanu.id.as_str(), keanu.summary.as_deref(), keanu.birthday.as_deref()), ("Q43416", Some("Canadian actor"), Some("1964-09-02")));
        assert_eq!(keanu.photo_url.as_deref(), Some("https://commons.wikimedia.org/wiki/Special:FilePath/Keanu.jpg?width=500"));
        let works: Vec<(&str, Option<bool>, Option<i64>)> = keanu.works.iter().map(|work| (work.title.as_str(), work.series, work.year)).collect();
        assert_eq!(works, [("The Matrix", Some(false), Some(1999)), ("Harbor Lights", Some(true), None)], "once each, unlabelled ones left out");
    }

    #[test]
    fn photos_carry_their_credit_or_go() {
        let reply = |artist: Option<&str>, required: &str| {
            let mut meta = json!({"LicenseShortName": {"value": "CC BY-SA 3.0"}, "AttributionRequired": {"value": required}});
            if let Some(artist) = artist {
                meta["Artist"] = json!({"value": artist});
            }
            json!({"query": {"pages": [{"title": "File:Keanu.jpg", "imageinfo": [{"extmetadata": meta, "descriptionurl": "https://commons.wikimedia.org/wiki/File:Keanu.jpg"}]}]}})
        };
        let credit = parse_credit(&reply(Some("<a href=\"//commons.wikimedia.org/wiki/User:Gage\">Gage   Skidmore</a> &amp; friends"), "true")).unwrap();
        assert_eq!(credit, PhotoCredit { text: "Gage Skidmore & friends, CC BY-SA 3.0".into(), page: "https://commons.wikimedia.org/wiki/File:Keanu.jpg".into() });
        assert_eq!(parse_credit(&reply(None, "true")), None, "credit asked for, but nobody to credit");
        assert_eq!(parse_credit(&reply(None, "false")).map(|credit| credit.text), Some("CC BY-SA 3.0".into()));
        assert_eq!(parse_credit(&json!({"query": {"pages": [{"missing": true}]}})), None);

        assert_eq!(commons_file("https://commons.wikimedia.org/wiki/Special:FilePath/Keanu%20Reeves_2019.jpg?width=500").as_deref(), Some("Keanu Reeves 2019.jpg"));
        assert_eq!(commons_file("https://upload.wikimedia.org/wikipedia/commons/a/ab/Keanu.jpg"), None);
    }

    #[test]
    fn names_are_quoted_safely() {
        let query = credits_query("Robert \"Bob\" O'Neil\\\n");
        assert!(query.contains(r#"rdfs:label "Robert \"Bob\" O'Neil\\"@en"#), "{query}");
    }
}
