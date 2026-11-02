//! How Xorvio's films, series and episodes are named to Trakt, and how
//! Trakt's titles are found in the catalog.
//!
//! Films and shows are matched by IMDb or TMDB id: the catalog's own columns,
//! or an add-on title's id itself (add-ons name titles by IMDb id). Episodes
//! go by their show plus season and number. Catalog titles without ids match
//! Trakt's by title and year, and are looked up on Trakt by them once (see
//! [`confident_match`]); the demo library is never matched, being made up.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};
use xorvio_model::{Episode, Movie, Series};

use crate::json;
use crate::store::catalog::{MatchKeys, title_key};
use crate::store::library::TraktTitle;

/// The `source_id` of add-on titles.
pub(crate) const ADDON_SOURCE: &str = "addon";

/// A film's or show's ids on Trakt, as many as are known.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub(crate) struct TraktIds {
    pub trakt: Option<i64>,
    pub imdb: Option<String>,
    pub tmdb: Option<i64>,
}

impl TraktIds {
    pub fn is_empty(&self) -> bool {
        self.trakt.is_none() && self.imdb.is_none() && self.tmdb.is_none()
    }

    /// A catalog title's ids, keeping only real ones: the demo library's
    /// made-up "demo-3" and anything else that isn't an IMDb or TMDB id are left out.
    pub fn from_catalog(imdb: Option<&str>, tmdb: Option<&str>) -> Self {
        Self { trakt: None, imdb: imdb.and_then(valid_imdb), tmdb: tmdb.and_then(valid_tmdb) }
    }

    /// The `ids` object Trakt sends with a film or show.
    pub fn from_json(ids: Option<&Value>) -> Self {
        let Some(ids) = ids.and_then(Value::as_object) else { return Self::default() };
        Self {
            trakt: json::int(ids.get("trakt")).filter(|id| *id > 0),
            imdb: json::string(ids.get("imdb")).as_deref().and_then(valid_imdb),
            tmdb: json::int(ids.get("tmdb")).filter(|id| *id > 0),
        }
    }

    /// The `ids` object to send.
    pub fn to_json(&self) -> Value {
        let mut ids = serde_json::Map::new();
        if let Some(trakt) = self.trakt {
            ids.insert("trakt".into(), json!(trakt));
        }
        if let Some(imdb) = &self.imdb {
            ids.insert("imdb".into(), json!(imdb));
        }
        if let Some(tmdb) = self.tmdb {
            ids.insert("tmdb".into(), json!(tmdb));
        }
        Value::Object(ids)
    }

    /// Whether these name the same title: any id in common.
    pub fn shares_id(&self, other: &Self) -> bool {
        fn same<T: PartialEq>(a: &Option<T>, b: &Option<T>) -> bool {
            a.is_some() && a == b
        }
        same(&self.trakt, &other.trakt) || same(&self.imdb, &other.imdb) || same(&self.tmdb, &other.tmdb)
    }
}

/// An IMDb id ("tt" and digits), lowercased "tt" and trimmed.
pub(crate) fn valid_imdb(text: &str) -> Option<String> {
    let text = text.trim();
    let digits = text.get(..2).filter(|prefix| prefix.eq_ignore_ascii_case("tt")).map(|_| &text[2..])?;
    (digits.len() >= 5 && digits.bytes().all(|b| b.is_ascii_digit())).then(|| format!("tt{digits}"))
}

/// A TMDB id: a positive whole number.
pub(crate) fn valid_tmdb(text: &str) -> Option<i64> {
    let text = text.trim();
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())).then(|| text.parse().ok()).flatten().filter(|id| *id > 0)
}

/// A film or show as Xorvio knows it, ready to be named to Trakt.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Title {
    pub show: bool,
    /// Empty when the source carries none; then the title is looked up by
    /// name and year.
    pub ids: TraktIds,
    /// The title as shown.
    pub name: String,
    pub year: Option<i64>,
}

impl Title {
    pub fn of_movie(movie: &Movie) -> Self {
        Self {
            show: false,
            ids: TraktIds::from_catalog(movie.imdb_id.as_deref(), movie.tmdb_id.as_deref()),
            name: crate::names::media_title(&movie.title),
            year: movie.year,
        }
    }

    pub fn of_series(series: &Series) -> Self {
        Self {
            show: true,
            ids: TraktIds::from_catalog(series.imdb_id.as_deref(), series.tmdb_id.as_deref()),
            name: crate::names::media_title(&series.title),
            year: series.year,
        }
    }

    pub fn key(&self) -> String {
        title_key(&self.name)
    }
}

/// What a film or episode is on Trakt.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Subject {
    Movie(Title),
    Episode { show: Title, season: i64, number: i64 },
}

impl TraktTitle {
    /// Whether this Trakt title is `title`: an id in common, or, for a title
    /// with no ids, the same title and year.
    pub fn is(&self, title: &Title) -> bool {
        if self.show != title.show {
            return false;
        }
        if !title.ids.is_empty() {
            return self.ids.shares_id(&title.ids);
        }
        title.year.is_some() && self.year == title.year && self.title_key == title.key()
    }
}

// MARK: Add-on titles

/// What an add-on title is on Trakt. Add-ons name titles by IMDb id
/// (`addon|movie|tt0000001`), and episodes by the show's id with season and
/// number (`addon|series|tt0000002|ep|tt0000002:1:3`); some use `tmdb:<id>`.
pub(crate) fn addon_subject(item_id: &str) -> Option<Subject> {
    let rest = item_id.strip_prefix("addon|")?;
    let (media_type, meta_id) = rest.split_once('|')?;
    match media_type {
        "movie" => Some(Subject::Movie(addon_named(false, meta_id)?)),
        "series" => {
            let (show_id, video_id) = meta_id.split_once("|ep|")?;
            let mut parts = video_id.rsplitn(3, ':');
            let number = parts.next()?.parse().ok()?;
            let season = parts.next()?.parse().ok()?;
            Some(Subject::Episode { show: addon_named(true, show_id)?, season, number })
        }
        _ => None,
    }
}

/// An add-on film or series id (`addon|movie|…`, `addon|series|…`) as a title.
pub(crate) fn addon_title(item_id: &str) -> Option<Title> {
    let rest = item_id.strip_prefix("addon|")?;
    let (media_type, meta_id) = rest.split_once('|')?;
    match media_type {
        "movie" => addon_named(false, meta_id),
        "series" if !meta_id.contains("|ep|") => addon_named(true, meta_id),
        _ => None,
    }
}

fn addon_named(show: bool, meta_id: &str) -> Option<Title> {
    let ids = match meta_id.strip_prefix("tmdb:") {
        Some(tmdb) => TraktIds { tmdb: valid_tmdb(tmdb), ..TraktIds::default() },
        None => TraktIds { imdb: valid_imdb(meta_id), ..TraktIds::default() },
    };
    (!ids.is_empty()).then(|| Title { show, ids, name: String::new(), year: None })
}

pub(crate) fn addon_movie_id(imdb: &str) -> String {
    format!("addon|movie|{imdb}")
}

pub(crate) fn addon_series_id(imdb: &str) -> String {
    format!("addon|series|{imdb}")
}

/// The id an add-on gives an episode (Cinemeta's `tt…:season:episode`).
pub(crate) fn addon_episode_id(imdb: &str, season: i64, number: i64) -> String {
    format!("{}|ep|{imdb}:{season}:{number}", addon_series_id(imdb))
}

// MARK: Catalog matching

/// A catalog film or series, as matched against Trakt's titles.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CatalogTitle {
    pub id: String,
    pub source_id: String,
    pub ids: TraktIds,
    /// Its title key and year (0 when unknown).
    pub name_key: (String, i64),
    pub title: String,
    pub year: Option<i64>,
    pub duration_seconds: Option<i64>,
    pub artwork_url: Option<String>,
}

impl CatalogTitle {
    pub fn of_movie(movie: &Movie) -> Self {
        Self {
            id: movie.id.clone(),
            source_id: movie.source_id.clone(),
            ids: TraktIds::from_catalog(movie.imdb_id.as_deref(), movie.tmdb_id.as_deref()),
            name_key: (title_key(&movie.title), movie.year.unwrap_or(0)),
            title: crate::names::media_title(&movie.title),
            year: movie.year,
            duration_seconds: movie.duration_seconds.filter(|seconds| *seconds > 0),
            artwork_url: movie.poster_url.clone().or_else(|| movie.backdrop_url.clone()),
        }
    }

    pub fn of_series(series: &Series) -> Self {
        Self {
            id: series.id.clone(),
            source_id: series.source_id.clone(),
            ids: TraktIds::from_catalog(series.imdb_id.as_deref(), series.tmdb_id.as_deref()),
            name_key: (title_key(&series.title), series.year.unwrap_or(0)),
            title: crate::names::media_title(&series.title),
            year: series.year,
            duration_seconds: None,
            artwork_url: series.poster_url.clone().or_else(|| series.backdrop_url.clone()),
        }
    }
}

/// What to ask the catalog for, to find these Trakt titles: their ids and
/// title keys, and the title keys of lookups that found them.
pub(crate) fn match_keys<'a>(titles: impl IntoIterator<Item = &'a TraktTitle>, lookups: &[(String, i64, TraktIds)]) -> MatchKeys {
    let mut imdb = HashSet::new();
    let mut tmdb = HashSet::new();
    let mut keys = HashSet::new();
    let mut found = Vec::new();
    for title in titles {
        imdb.extend(title.ids.imdb.clone());
        tmdb.extend(title.ids.tmdb.map(|id| id.to_string()));
        if title.year.is_some() && !title.title_key.is_empty() {
            keys.insert(title.title_key.clone());
        }
        found.push(title.ids.clone());
    }
    for (key, _, ids) in lookups {
        if found.iter().any(|title| title.shares_id(ids)) {
            keys.insert(key.clone());
        }
    }
    MatchKeys { imdb: imdb.into_iter().collect(), tmdb: tmdb.into_iter().collect(), title_keys: keys.into_iter().collect() }
}

/// Catalog titles that could be Trakt's, found by id, by title and year (for
/// catalog titles with no ids), or through lookups made earlier.
pub(crate) struct Candidates {
    /// In the user's source order.
    titles: Vec<CatalogTitle>,
    by_imdb: HashMap<String, Vec<usize>>,
    by_tmdb: HashMap<i64, Vec<usize>>,
    /// Titles with no ids of their own, by title key and year.
    by_name: HashMap<(String, i64), Vec<usize>>,
    lookups: Vec<(String, i64, TraktIds)>,
}

impl Candidates {
    pub fn new(mut titles: Vec<CatalogTitle>, lookups: Vec<(String, i64, TraktIds)>, source_order: &[String]) -> Self {
        let rank: HashMap<&str, usize> = source_order.iter().enumerate().map(|(index, id)| (id.as_str(), index)).collect();
        titles.sort_by_key(|title| rank.get(title.source_id.as_str()).copied().unwrap_or(usize::MAX));
        let mut candidates =
            Self { titles: Vec::new(), by_imdb: HashMap::new(), by_tmdb: HashMap::new(), by_name: HashMap::new(), lookups };
        for (index, title) in titles.iter().enumerate() {
            if let Some(imdb) = &title.ids.imdb {
                candidates.by_imdb.entry(imdb.clone()).or_default().push(index);
            }
            if let Some(tmdb) = title.ids.tmdb {
                candidates.by_tmdb.entry(tmdb).or_default().push(index);
            }
            if title.ids.is_empty() && !title.name_key.0.is_empty() {
                candidates.by_name.entry(title.name_key.clone()).or_default().push(index);
            }
        }
        candidates.titles = titles;
        candidates
    }

    /// The catalog titles that are this Trakt title, in the user's source order.
    pub fn matches(&self, title: &TraktTitle) -> Vec<&CatalogTitle> {
        let mut found: Vec<usize> = Vec::new();
        if let Some(imdb) = &title.ids.imdb {
            found.extend(self.by_imdb.get(imdb).into_iter().flatten());
        }
        if let Some(tmdb) = title.ids.tmdb {
            found.extend(self.by_tmdb.get(&tmdb).into_iter().flatten());
        }
        if let Some(year) = title.year {
            found.extend(self.by_name.get(&(title.title_key.clone(), year)).into_iter().flatten());
        }
        for (key, year, ids) in &self.lookups {
            if ids.shares_id(&title.ids) {
                found.extend(self.by_name.get(&(key.clone(), *year)).into_iter().flatten());
            }
        }
        found.sort_unstable();
        found.dedup();
        found.into_iter().map(|index| &self.titles[index]).collect()
    }
}

/// A series' episodes by season and number.
pub(crate) fn by_season_and_number(episodes: &[Episode]) -> HashMap<(i64, i64), &Episode> {
    episodes.iter().map(|episode| ((episode.season, episode.number), episode)).collect()
}

/// The one search result that is this title: the same title (as a title
/// key) and, when the year is known, the same year. Nothing when no result
/// is, or when several are (two shows of one name, one year): a wrong match
/// would mark the wrong title watched.
pub(crate) fn confident_match(results: &Value, show: bool, key: &str, year: Option<i64>) -> Option<TraktIds> {
    let kind = if show { "show" } else { "movie" };
    let mut matching = results.as_array()?.iter().filter_map(|result| {
        let title = result.get(kind)?;
        let same_title = json::string(title.get("title")).is_some_and(|name| title_key(&name) == key);
        let same_year = year.is_none_or(|year| json::int(title.get("year")) == Some(year));
        let ids = TraktIds::from_json(title.get("ids"));
        (same_title && same_year && !ids.is_empty()).then_some(ids)
    });
    let first = matching.next()?;
    matching.all(|other| other == first).then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trakt_title(show: bool, trakt: i64, imdb: Option<&str>, tmdb: Option<i64>, name: &str, year: Option<i64>) -> TraktTitle {
        TraktTitle {
            show,
            ids: TraktIds { trakt: Some(trakt), imdb: imdb.map(str::to_string), tmdb },
            title_key: title_key(name),
            year,
        }
    }

    fn catalog(id: &str, imdb: Option<&str>, tmdb: Option<&str>, title: &str, year: Option<i64>) -> CatalogTitle {
        CatalogTitle {
            id: id.into(),
            source_id: id.split('|').next().unwrap().into(),
            ids: TraktIds::from_catalog(imdb, tmdb),
            name_key: (title_key(title), year.unwrap_or(0)),
            title: title.into(),
            year,
            duration_seconds: None,
            artwork_url: None,
        }
    }

    #[test]
    fn only_real_ids_are_sent() {
        assert_eq!(valid_imdb(" TT0903747 ").as_deref(), Some("tt0903747"));
        assert_eq!(valid_imdb("0903747"), None);
        assert_eq!(valid_imdb("tt12"), None);
        assert_eq!(valid_tmdb("1396"), Some(1396));
        assert_eq!(valid_tmdb("demo-3"), None, "the demo library's made-up ids");
        assert_eq!(valid_tmdb("0"), None);
        assert!(TraktIds::from_catalog(None, Some("demo-series-1")).is_empty());
        let ids = TraktIds::from_json(Some(&json!({"trakt": 1, "slug": "x", "imdb": "tt0000001", "tmdb": 5, "tvdb": 9})));
        assert_eq!(ids, TraktIds { trakt: Some(1), imdb: Some("tt0000001".into()), tmdb: Some(5) });
        assert_eq!(ids.to_json(), json!({"trakt": 1, "imdb": "tt0000001", "tmdb": 5}));
        assert!(ids.shares_id(&TraktIds { tmdb: Some(5), ..TraktIds::default() }));
        assert!(!ids.shares_id(&TraktIds::default()), "nothing known matches nothing");
    }

    #[test]
    fn add_on_ids_name_their_titles() {
        let Some(Subject::Movie(film)) = addon_subject("addon|movie|tt0000001") else { panic!() };
        assert_eq!(film.ids.imdb.as_deref(), Some("tt0000001"));
        let Some(Subject::Episode { show, season, number }) = addon_subject("addon|series|tt0000002|ep|tt0000002:1:3") else { panic!() };
        assert_eq!((show.ids.imdb.as_deref(), season, number), (Some("tt0000002"), 1, 3));
        assert!(show.show);
        assert_eq!(addon_title("addon|series|tmdb:44").map(|title| title.ids.tmdb), Some(Some(44)));
        assert_eq!(addon_title("addon|series|tt0000002|ep|tt0000002:1:3"), None, "an episode isn't a title");
        assert_eq!(addon_subject("addon|channel|x"), None);
        assert_eq!(addon_subject("addon|movie|local-file-7"), None, "no id Trakt knows");
        assert_eq!(addon_episode_id("tt0000002", 1, 3), "addon|series|tt0000002|ep|tt0000002:1:3");
    }

    #[test]
    fn catalog_titles_match_by_id_then_name_then_lookup() {
        let titles = vec![
            catalog("b|mv|1", Some("tt0000001"), None, "Paper Boats", Some(2020)),
            catalog("a|mv|1", None, Some("77"), "Paper Boats", Some(2020)),
            catalog("a|mv|2", None, None, "Glass Valley", Some(2018)),
            catalog("a|mv|3", None, None, "Low Tide", None),
            catalog("a|mv|4", Some("tt0000009"), None, "Glass Valley", Some(2018)),
        ];
        let lookups = vec![(title_key("Low Tide"), 0, TraktIds { trakt: Some(30), ..TraktIds::default() })];
        let candidates = Candidates::new(titles, lookups, &["a".to_string(), "b".to_string()]);
        let ids = |found: Vec<&CatalogTitle>| found.into_iter().map(|title| title.id.clone()).collect::<Vec<_>>();

        let boats = trakt_title(false, 10, Some("tt0000001"), Some(77), "Paper Boats", Some(2020));
        assert_eq!(ids(candidates.matches(&boats)), ["a|mv|1", "b|mv|1"], "both copies, in source order");
        let valley = trakt_title(false, 20, Some("tt0000002"), None, "Glass Valley", Some(2018));
        assert_eq!(ids(candidates.matches(&valley)), ["a|mv|2"], "by name only where the catalog has no ids");
        let other_year = trakt_title(false, 21, None, None, "Glass Valley", Some(1999));
        assert!(candidates.matches(&other_year).is_empty());
        let tide = trakt_title(false, 30, None, None, "Low Tide (Director's Cut)", Some(2001));
        assert_eq!(ids(candidates.matches(&tide)), ["a|mv|3"], "through the lookup that found it");
        let keys = match_keys([&boats, &valley, &tide], &candidates.lookups);
        assert!(keys.title_keys.contains(&title_key("Low Tide")) && keys.tmdb == ["77"]);
    }

    #[test]
    fn search_results_count_only_when_one_is_certain() {
        let results = json!([
            {"type": "show", "score": 100, "show": {"title": "The Harbour", "year": 2019, "ids": {"trakt": 1, "imdb": "tt0000011"}}},
            {"type": "show", "score": 90, "show": {"title": "The Harbour", "year": 2004, "ids": {"trakt": 2}}},
            {"type": "show", "score": 40, "show": {"title": "Harbour Lights", "year": 2019, "ids": {"trakt": 3}}}
        ]);
        let key = title_key("The Harbour");
        assert_eq!(confident_match(&results, true, &key, Some(2019)).and_then(|ids| ids.trakt), Some(1));
        assert_eq!(confident_match(&results, true, &key, None), None, "two of that name and no year to choose");
        assert_eq!(confident_match(&results, true, &key, Some(2011)), None);
        assert_eq!(confident_match(&results, false, &key, Some(2019)), None, "films aren't shows");
        assert_eq!(confident_match(&json!([]), true, &key, Some(2019)), None);
    }
}
