//! TMDB's API (version 3): a film, series or person with what's appended to
//! it, titles found by IMDb id or by name, and the replies read into
//! Xorvio's own shapes. Images stay on TMDB's servers (image.tmdb.org) and
//! are shown from there.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use xorvio_model::{CastMember, RatingSource, SeasonInfo, TitleRating, VideoLink};

use crate::details::DetailsRecord;
use crate::error::SourceError;
use crate::http::{ApiCall, Http};
use crate::json::{self, Object};
use crate::names;

pub(crate) const DEFAULT_BASE: &str = "https://api.themoviedb.org";
const IMAGES: &str = "https://image.tmdb.org/t/p";
/// The most a request may append (TMDB refuses more).
pub(crate) const MOST_APPENDED: usize = 20;
const MOST_PEOPLE: usize = 20;
const MOST_RECOMMENDED: usize = 24;
const MOST_VIDEOS: usize = 12;
/// What a kept [`TmdbTitle`] holds: 2 added ratings, videos and the poster.
/// Titles kept in an earlier form are asked for again (and still shown
/// meanwhile, or when TMDB can't be reached).
pub(crate) const TITLE_FORMAT: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Movie,
    Tv,
}

impl Kind {
    /// TMDB's name for it, in paths and in the cache.
    pub fn path(self) -> &'static str {
        match self {
            Kind::Movie => "movie",
            Kind::Tv => "tv",
        }
    }
}

/// A TMDB key: a v3 API key, sent in the address as TMDB asks, or a v4 read
/// access token, sent as a bearer token. Never shown or logged.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Key(String);

impl Key {
    pub fn new(text: &str) -> Option<Key> {
        let text = text.trim();
        (!text.is_empty() && text.chars().all(|c| c.is_ascii_graphic())).then(|| Key(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Read access tokens are JWTs ("eyJ…" in three dotted parts); API keys are 32 hex digits.
    fn is_token(&self) -> bool {
        self.0.starts_with("eyJ") && self.0.matches('.').count() == 2
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key(…)")
    }
}

// MARK: What's kept

/// A film's or series' details as TMDB has them, what it recommends, and a
/// series' episodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct TmdbTitle {
    pub details: DetailsRecord,
    pub recommendations: Vec<TmdbRef>,
    /// Every season TMDB lists episodes for, specials (0) included.
    pub season_numbers: Vec<i64>,
    pub episodes: Vec<TmdbEpisode>,
    /// TMDB's poster, sized for lists.
    pub poster_url: Option<String>,
    /// [`TITLE_FORMAT`] when made; kept titles without one are older.
    #[serde(default)]
    pub format: u32,
}

impl Default for TmdbTitle {
    fn default() -> Self {
        Self {
            details: DetailsRecord::default(),
            recommendations: Vec::new(),
            season_numbers: Vec::new(),
            episodes: Vec::new(),
            poster_url: None,
            format: TITLE_FORMAT,
        }
    }
}

impl TmdbTitle {
    /// TMDB had no such title (kept so it isn't asked again soon).
    pub fn is_empty(&self) -> bool {
        self.details.is_empty() && self.recommendations.is_empty() && self.episodes.is_empty()
    }
}

/// Another film or series, by TMDB's id, with what it's called.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct TmdbRef {
    pub id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i64>,
}

impl TmdbRef {
    /// A catalog title by this name (or its original one) and year (within one, when both are known).
    pub fn matches(&self, title_key: &str, year: Option<i64>) -> bool {
        if title_key.is_empty() {
            return false;
        }
        let named = names::match_key(&self.title) == title_key
            || self.original_title.as_deref().is_some_and(|original| names::match_key(original) == title_key);
        named && year.zip(self.year).is_none_or(|(ours, theirs)| (ours - theirs).abs() <= 1)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct TmdbEpisode {
    pub season: i64,
    pub number: i64,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub still_url: Option<String>,
    pub runtime_minutes: Option<i64>,
    /// "2019-04-14".
    pub air_date: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct TmdbPerson {
    pub id: i64,
    pub name: String,
    pub photo_url: Option<String>,
    pub biography: Option<String>,
    pub department: Option<String>,
    pub birthday: Option<String>,
    pub deathday: Option<String>,
    pub place_of_birth: Option<String>,
    /// What they acted in, directed or created, best known first.
    pub credits: Vec<TmdbCredit>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct TmdbCredit {
    pub series: bool,
    pub work: TmdbRef,
    pub popularity: f64,
}

/// Someone a name search found, and the films and series they're known for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FoundPerson {
    pub id: i64,
    pub name: String,
    pub known_for: Vec<(Kind, i64)>,
}

// MARK: Asking

pub(crate) struct Client<'a> {
    pub http: &'a Http,
    pub base: &'a str,
    pub key: &'a Key,
}

impl Client<'_> {
    async fn get(&self, path: &str, params: &[(&str, &str)]) -> Result<Object, SourceError> {
        let mut link = url::Url::parse(&format!("{}/3/{path}", self.base.trim_end_matches('/')))
            .map_err(|_| SourceError::Unsupported("TMDB's address isn't valid.".into()))?;
        {
            let mut query = link.query_pairs_mut();
            for (name, value) in params {
                query.append_pair(name, value);
            }
            if !self.key.is_token() {
                query.append_pair("api_key", self.key.as_str());
            }
        }
        let mut headers = vec![("Accept".to_string(), "application/json".to_string())];
        if self.key.is_token() {
            headers.push(("Authorization".to_string(), format!("Bearer {}", self.key.as_str())));
        }
        match self.http.api_json(ApiCall::get(link.as_str(), &headers)).await? {
            Value::Object(object) => Ok(object),
            _ => Err(SourceError::Network("TMDB sent a reply Xorvio couldn't read.".into())),
        }
    }

    /// Asks for nothing in particular, to see that the key works.
    pub async fn check_key(&self) -> Result<(), SourceError> {
        self.get("configuration", &[]).await.map(|_| ())
    }

    /// The film or series with this IMDb id ("tt0133093").
    pub async fn find_by_imdb(&self, kind: Kind, imdb_id: &str) -> Result<Option<i64>, SourceError> {
        let found = self.get(&format!("find/{imdb_id}"), &[("external_source", "imdb_id")]).await?;
        let results = match kind {
            Kind::Movie => "movie_results",
            Kind::Tv => "tv_results",
        };
        Ok(json::objects(found.get(results)).first().and_then(|result| json::int(result.get("id"))).filter(|id| *id > 0))
    }

    /// The film or series by this name and year: searched with the year
    /// first, then without it (providers' years are often a year out), and
    /// taken only when the name is the same and the year within one.
    pub async fn best_match(&self, kind: Kind, title: &str, year: Option<i64>) -> Result<Option<i64>, SourceError> {
        let key = names::match_key(title);
        if let Some(year) = year {
            let found = self.search(kind, title, Some(year)).await?;
            if let Some(id) = pick_match(&found, &key, Some(year)) {
                return Ok(Some(id));
            }
        }
        let found = self.search(kind, title, None).await?;
        Ok(pick_match(&found, &key, year))
    }

    async fn search(&self, kind: Kind, title: &str, year: Option<i64>) -> Result<Vec<TmdbRef>, SourceError> {
        let year = year.map(|year| year.to_string());
        let mut params = vec![("query", title), ("include_adult", "false")];
        if let Some(year) = year.as_deref() {
            params.push((if kind == Kind::Movie { "year" } else { "first_air_date_year" }, year));
        }
        let found = self.get(&format!("search/{}", kind.path()), &params).await?;
        Ok(json::objects(found.get("results")).into_iter().filter_map(|raw| reference(raw, kind)).collect())
    }

    pub async fn movie(&self, id: i64) -> Result<TmdbTitle, SourceError> {
        let root = self
            .get(&format!("movie/{id}"), &[("append_to_response", "images,credits,release_dates,videos,recommendations,external_ids"), LANGUAGES[0], LANGUAGES[1]])
            .await?;
        Ok(parse_movie(&root))
    }

    /// A film's or series' genres, story and credits only, as details: a
    /// much smaller reply than its page's, for looking a library up for
    /// search.
    pub async fn search_details(&self, kind: Kind, id: i64) -> Result<DetailsRecord, SourceError> {
        let title = match kind {
            Kind::Movie => parse_movie(&self.get(&format!("movie/{id}"), &[("append_to_response", "credits")]).await?),
            Kind::Tv => parse_series(&self.get(&format!("tv/{id}"), &[("append_to_response", "aggregate_credits")]).await?),
        };
        Ok(title.details)
    }

    /// Only a film's or series' poster, sized for lists: a small reply,
    /// for titles shown in a list whose source has none.
    pub async fn poster(&self, kind: Kind, id: i64) -> Result<Option<String>, SourceError> {
        let root = self.get(&format!("{}/{id}", kind.path()), &[]).await?;
        Ok(image(root.get("poster_path"), POSTER_SIZE))
    }

    /// A series without its episodes ([`Client::episodes`] asks for those).
    pub async fn series(&self, id: i64) -> Result<TmdbTitle, SourceError> {
        let root = self
            .get(&format!("tv/{id}"), &[("append_to_response", "images,aggregate_credits,content_ratings,videos,recommendations,external_ids"), LANGUAGES[0], LANGUAGES[1]])
            .await?;
        Ok(parse_series(&root))
    }

    /// The episodes of up to [`MOST_APPENDED`] seasons, in one request.
    pub async fn episodes(&self, id: i64, seasons: &[i64]) -> Result<Vec<TmdbEpisode>, SourceError> {
        let appended: Vec<String> = seasons.iter().take(MOST_APPENDED).map(|season| format!("season/{season}")).collect();
        let root = self.get(&format!("tv/{id}"), &[("append_to_response", &appended.join(","))]).await?;
        Ok(parse_episodes(&root, seasons))
    }

    pub async fn person(&self, id: i64) -> Result<TmdbPerson, SourceError> {
        let root = self.get(&format!("person/{id}"), &[("append_to_response", "combined_credits")]).await?;
        Ok(parse_person(&root))
    }

    pub async fn search_person(&self, name: &str) -> Result<Vec<FoundPerson>, SourceError> {
        let found = self.get("search/person", &[("query", name), ("include_adult", "false")]).await?;
        Ok(json::objects(found.get("results"))
            .into_iter()
            .filter_map(|raw| {
                Some(FoundPerson {
                    id: json::int(raw.get("id")).filter(|id| *id > 0)?,
                    name: json::string(raw.get("name"))?,
                    known_for: json::objects(raw.get("known_for"))
                        .into_iter()
                        .filter_map(|work| {
                            let kind = match json::string(work.get("media_type"))?.as_str() {
                                "movie" => Kind::Movie,
                                "tv" => Kind::Tv,
                                _ => return None,
                            };
                            Some((kind, json::int(work.get("id"))?))
                        })
                        .collect(),
                })
            })
            .collect())
    }
}

/// Posters in lists are at most a few hundred points wide.
const POSTER_SIZE: &str = "w342";

/// English artwork, and artwork with no words on it (backdrops look best
/// without their title printed on them, and a logo is drawn over them).
const LANGUAGES: [(&str, &str); 2] = [("include_image_language", "en,null"), ("include_video_language", "en,null")];

/// The search result with this name and the nearest year (within one), or
/// with this name when the year isn't known; TMDB lists the best known first.
pub(crate) fn pick_match(found: &[TmdbRef], title_key: &str, year: Option<i64>) -> Option<i64> {
    found
        .iter()
        .filter(|candidate| candidate.matches(title_key, year))
        // Ours has a year and theirs doesn't: an announced title, not this one.
        .filter(|candidate| year.is_none() || candidate.year.is_some())
        .min_by_key(|candidate| year.zip(candidate.year).map_or(0, |(ours, theirs)| (ours - theirs).abs()))
        .map(|candidate| candidate.id)
}

// MARK: Reading replies

fn image(path: Option<&Value>, size: &str) -> Option<String> {
    let path = json::string(path)?;
    (path.starts_with('/') && !path.contains("..")).then(|| format!("{IMAGES}/{size}{path}"))
}

fn text(value: Option<&Value>) -> Option<String> {
    json::string(value).map(|text| text.trim().to_string()).filter(|text| !text.is_empty())
}

fn year_of(date: Option<&Value>) -> Option<i64> {
    text(date)?.get(..4)?.parse().ok()
}

fn reference(raw: &Object, kind: Kind) -> Option<TmdbRef> {
    let (title, original, date) = match kind {
        Kind::Movie => ("title", "original_title", "release_date"),
        Kind::Tv => ("name", "original_name", "first_air_date"),
    };
    let title = text(raw.get(title))?;
    let original_title = text(raw.get(original)).filter(|original| original != &title);
    Some(TmdbRef { id: json::int(raw.get("id")).filter(|id| *id > 0)?, title, original_title, year: year_of(raw.get(date)) })
}

/// What films and series share: artwork, tagline, genres, the trailer and
/// what TMDB recommends.
fn common(root: &Object, kind: Kind) -> (DetailsRecord, Vec<TmdbRef>) {
    let images = json::object(root, "images");
    let mut backdrops = json::objects(images.and_then(|images| images.get("backdrops")));
    // Wordless backdrops first, then the best rated.
    backdrops.sort_by(|a, b| {
        let worded = |entry: &Object| text(entry.get("iso_639_1")).is_some();
        let votes = |entry: &Object| json::double(entry.get("vote_average")).unwrap_or(0.0);
        worded(a).cmp(&worded(b)).then(votes(b).total_cmp(&votes(a)))
    });
    let mut logos: Vec<&Object> = json::objects(images.and_then(|images| images.get("logos")))
        .into_iter()
        .filter(|logo| text(logo.get("iso_639_1")).as_deref() == Some("en"))
        .collect();
    logos.sort_by(|a, b| {
        let votes = |entry: &Object| json::double(entry.get("vote_average")).unwrap_or(0.0);
        votes(b).total_cmp(&votes(a))
    });
    let details = DetailsRecord {
        backdrop_urls: backdrops.iter().filter_map(|entry| image(entry.get("file_path"), "w1280")).take(5).collect(),
        logo_url: logos.first().and_then(|logo| image(logo.get("file_path"), "w500")),
        tagline: text(root.get("tagline")),
        overview: text(root.get("overview")),
        genres: json::objects(root.get("genres")).into_iter().filter_map(|genre| text(genre.get("name"))).collect(),
        trailer_youtube_id: trailer(json::object(root, "videos")),
        videos: videos(json::object(root, "videos")),
        ratings: vote(root).into_iter().collect(),
        attribution: Some("TMDB".into()),
        ..Default::default()
    };
    let recommendations = json::objects(json::object(root, "recommendations").and_then(|list| list.get("results")))
        .into_iter()
        .filter_map(|raw| reference(raw, kind))
        .take(MOST_RECOMMENDED)
        .collect();
    (details, recommendations)
}

/// The best YouTube video: a trailer before a teaser before a clip, official ones first.
fn trailer(videos: Option<&Object>) -> Option<String> {
    const ORDER: [&str; 4] = ["Trailer", "Teaser", "Clip", "Featurette"];
    let results = json::objects(videos.and_then(|videos| videos.get("results")));
    results
        .into_iter()
        .filter(|video| text(video.get("site")).as_deref() == Some("YouTube"))
        .filter_map(|video| {
            let rank = ORDER.iter().position(|kind| text(video.get("type")).as_deref() == Some(kind))?;
            let unofficial = video.get("official").and_then(Value::as_bool) != Some(true);
            Some(((rank, unofficial), text(video.get("key"))?))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, key)| key)
}

/// YouTube videos in the order a page shows them: trailers, teasers, clips,
/// then the rest, official ones first within each.
fn videos(videos: Option<&Object>) -> Vec<VideoLink> {
    const ORDER: [&str; 7] = ["Trailer", "Teaser", "Clip", "Featurette", "Behind the Scenes", "Bloopers", "Opening Credits"];
    let mut found: Vec<((usize, bool), VideoLink)> = json::objects(videos.and_then(|videos| videos.get("results")))
        .into_iter()
        .filter(|video| text(video.get("site")).as_deref() == Some("YouTube"))
        .filter_map(|video| {
            let kind = text(video.get("type"));
            let rank = kind.as_deref().and_then(|kind| ORDER.iter().position(|known| *known == kind)).unwrap_or(ORDER.len());
            let unofficial = video.get("official").and_then(Value::as_bool) != Some(true);
            let youtube_id = text(video.get("key"))?;
            let title = text(video.get("name")).unwrap_or_else(|| kind.clone().unwrap_or_else(|| "Video".into()));
            Some(((rank, unofficial), VideoLink { youtube_id, title, kind }))
        })
        .collect();
    // A stable sort keeps TMDB's own order within each kind.
    found.sort_by_key(|(rank, _)| *rank);
    let mut seen = std::collections::HashSet::new();
    found.into_iter().map(|(_, video)| video).filter(|video| seen.insert(video.youtube_id.clone())).take(MOST_VIDEOS).collect()
}

/// TMDB's own users' score, once enough have voted for it to mean something.
fn vote(root: &Object) -> Option<TitleRating> {
    let votes = json::int(root.get("vote_count")).filter(|votes| *votes >= 10)?;
    let score = json::double(root.get("vote_average")).filter(|score| *score > 0.0 && *score <= 10.0)?;
    Some(TitleRating { source: RatingSource::Tmdb, score: (score * 10.0).round() / 10.0, votes: Some(votes) })
}

fn people(cast: Option<&Value>, series: bool) -> Vec<CastMember> {
    json::objects(cast)
        .into_iter()
        .filter_map(|raw| {
            let role = if series {
                json::objects(raw.get("roles")).first().and_then(|role| text(role.get("character")))
            } else {
                text(raw.get("character"))
            };
            Some(CastMember {
                name: text(raw.get("name"))?,
                role,
                photo_url: image(raw.get("profile_path"), "w185"),
                tmdb_id: json::int(raw.get("id")).filter(|id| *id > 0).map(|id| id.to_string()),
            })
        })
        .take(MOST_PEOPLE)
        .collect()
}

/// The US rating ("PG-13", "TV-MA"), the one Xorvio's other sources use.
fn us_rating(list: Option<&Object>, rating: impl Fn(&Object) -> Option<String>) -> Option<String> {
    json::objects(list.and_then(|list| list.get("results")))
        .into_iter()
        .find(|entry| text(entry.get("iso_3166_1")).as_deref() == Some("US"))
        .and_then(rating)
}

pub(crate) fn parse_movie(root: &Object) -> TmdbTitle {
    let (mut details, recommendations) = common(root, Kind::Movie);
    let credits = json::object(root, "credits");
    details.people = people(credits.and_then(|credits| credits.get("cast")), false);
    details.director = json::objects(credits.and_then(|credits| credits.get("crew")))
        .into_iter()
        .find(|member| text(member.get("job")).as_deref() == Some("Director"))
        .and_then(|member| text(member.get("name")));
    details.runtime_minutes = json::int(root.get("runtime")).filter(|minutes| *minutes > 0);
    details.imdb_id = text(root.get("imdb_id")).or_else(|| json::object(root, "external_ids").and_then(|ids| text(ids.get("imdb_id"))));
    details.country = json::objects(root.get("production_countries")).first().and_then(|country| text(country.get("name")));
    details.release_date = text(root.get("release_date"));
    details.original_title = text(root.get("original_title")).filter(|original| Some(original) != text(root.get("title")).as_ref());
    details.age_rating = us_rating(json::object(root, "release_dates"), |entry| {
        json::objects(entry.get("release_dates")).into_iter().find_map(|release| text(release.get("certification")))
    });
    TmdbTitle { details, recommendations, poster_url: image(root.get("poster_path"), POSTER_SIZE), ..Default::default() }
}

pub(crate) fn parse_series(root: &Object) -> TmdbTitle {
    let (mut details, recommendations) = common(root, Kind::Tv);
    details.people = people(json::object(root, "aggregate_credits").and_then(|credits| credits.get("cast")), true);
    details.creators = json::objects(root.get("created_by")).into_iter().filter_map(|person| text(person.get("name"))).collect();
    details.runtime_minutes = root
        .get("episode_run_time")
        .and_then(Value::as_array)
        .and_then(|times| json::int(times.first()))
        .or_else(|| json::object(root, "last_episode_to_air").and_then(|episode| json::int(episode.get("runtime"))))
        .filter(|minutes| *minutes > 0);
    details.imdb_id = json::object(root, "external_ids").and_then(|ids| text(ids.get("imdb_id")));
    details.release_date = text(root.get("first_air_date"));
    details.original_title = text(root.get("original_name")).filter(|original| Some(original) != text(root.get("name")).as_ref());
    details.network = json::objects(root.get("networks")).first().and_then(|network| text(network.get("name")));
    details.age_rating = us_rating(json::object(root, "content_ratings"), |entry| text(entry.get("rating")));
    let seasons = json::objects(root.get("seasons"));
    details.seasons = seasons
        .iter()
        .filter_map(|raw| {
            let number = json::int(raw.get("season_number")).filter(|number| *number > 0)?;
            Some(SeasonInfo {
                number,
                name: text(raw.get("name")),
                overview: text(raw.get("overview")),
                air_date: text(raw.get("air_date")),
                cover_url: image(raw.get("poster_path"), "w500"),
                episode_count: json::int(raw.get("episode_count")),
            })
        })
        .collect();
    let season_numbers = seasons
        .iter()
        .filter(|raw| json::int(raw.get("episode_count")).is_none_or(|count| count > 0))
        .filter_map(|raw| json::int(raw.get("season_number")).filter(|number| *number >= 0))
        .collect();
    TmdbTitle { details, recommendations, season_numbers, poster_url: image(root.get("poster_path"), POSTER_SIZE), ..Default::default() }
}

/// The episodes of each `season/<n>` appended to a series.
pub(crate) fn parse_episodes(root: &Object, seasons: &[i64]) -> Vec<TmdbEpisode> {
    seasons
        .iter()
        .filter_map(|season| json::object(root, &format!("season/{season}")))
        .flat_map(|season| json::objects(season.get("episodes")))
        .filter_map(|raw| {
            Some(TmdbEpisode {
                season: json::int(raw.get("season_number"))?,
                number: json::int(raw.get("episode_number"))?,
                name: text(raw.get("name")),
                overview: text(raw.get("overview")),
                still_url: image(raw.get("still_path"), "w780"),
                runtime_minutes: json::int(raw.get("runtime")).filter(|minutes| *minutes > 0),
                air_date: text(raw.get("air_date")),
            })
        })
        .collect()
}

pub(crate) fn parse_person(root: &Object) -> TmdbPerson {
    let credits = json::object(root, "combined_credits");
    let mut works: Vec<TmdbCredit> = Vec::new();
    let acted = json::objects(credits.and_then(|credits| credits.get("cast")));
    let crew = json::objects(credits.and_then(|credits| credits.get("crew")))
        .into_iter()
        .filter(|credit| matches!(text(credit.get("job")).as_deref(), Some("Director" | "Creator")));
    for raw in acted.into_iter().chain(crew) {
        let series = match text(raw.get("media_type")).as_deref() {
            Some("movie") => false,
            Some("tv") => true,
            _ => continue,
        };
        // Talk shows, award shows and the news list people appearing as
        // themselves; those aren't their work.
        let character = text(raw.get("character")).unwrap_or_default().to_lowercase();
        if series && ["self", "himself", "herself", "themselves", "host"].iter().any(|word| character.starts_with(word)) {
            continue;
        }
        let Some(work) = reference(raw, if series { Kind::Tv } else { Kind::Movie }) else { continue };
        let popularity = json::double(raw.get("popularity")).unwrap_or(0.0);
        match works.iter_mut().find(|known| known.series == series && known.work.id == work.id) {
            Some(known) => known.popularity = known.popularity.max(popularity),
            None => works.push(TmdbCredit { series, work, popularity }),
        }
    }
    works.sort_by(|a, b| b.popularity.total_cmp(&a.popularity));
    TmdbPerson {
        id: json::int(root.get("id")).unwrap_or_default(),
        name: text(root.get("name")).unwrap_or_default(),
        photo_url: image(root.get("profile_path"), "h632"),
        biography: text(root.get("biography")),
        department: text(root.get("known_for_department")),
        birthday: text(root.get("birthday")),
        deathday: text(root.get("deathday")),
        place_of_birth: text(root.get("place_of_birth")),
        credits: works,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Object {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn keys_and_tokens_are_told_apart_and_never_printed() {
        assert!(!Key::new("0123456789abcdef0123456789abcdef").unwrap().is_token());
        assert!(Key::new(" eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiJ4In0.c2ln ").unwrap().is_token());
        assert_eq!(Key::new("  "), None);
        assert_eq!(Key::new("has space"), None);
        assert_eq!(format!("{:?}", Key::new("secret-value").unwrap()), "Key(…)");
    }

    #[test]
    fn searches_take_only_a_convincing_match() {
        let found = vec![
            TmdbRef { id: 1, title: "Halloween".into(), original_title: None, year: Some(2018) },
            TmdbRef { id: 2, title: "Halloween".into(), original_title: None, year: Some(1978) },
            TmdbRef { id: 3, title: "Halloween II".into(), original_title: None, year: Some(1981) },
            TmdbRef { id: 4, title: "Le Samouraï".into(), original_title: Some("Le Samouraï".into()), year: Some(1967) },
            TmdbRef { id: 5, title: "Announced".into(), original_title: None, year: None },
        ];
        let key = names::match_key("Halloween");
        assert_eq!(pick_match(&found, &key, Some(1979)), Some(2), "the nearest year within one");
        assert_eq!(pick_match(&found, &key, None), Some(1), "the best known without a year");
        assert_eq!(pick_match(&found, &key, Some(1995)), None);
        assert_eq!(pick_match(&found, &names::match_key("Le Samourai"), Some(1967)), Some(4), "accents don't matter");
        assert_eq!(pick_match(&found, &names::match_key("Announced"), Some(2027)), None, "theirs has no year");
        assert_eq!(pick_match(&found, "", None), None);
    }

    #[test]
    fn a_film_reads_into_details() {
        let root = object(json!({
            "id": 603, "title": "The Matrix", "original_title": "The Matrix", "tagline": "Welcome to the Real World.",
            "overview": "A hacker learns the truth.", "runtime": 136, "release_date": "1999-03-30", "imdb_id": "tt0133093",
            "genres": [{"id": 28, "name": "Action"}], "production_countries": [{"name": "United States of America"}],
            "images": {
                "backdrops": [
                    {"file_path": "/worded.jpg", "iso_639_1": "en", "vote_average": 9.0},
                    {"file_path": "/plain.jpg", "iso_639_1": null, "vote_average": 5.0},
                    {"file_path": "../escape.jpg", "iso_639_1": null, "vote_average": 4.0}
                ],
                "logos": [{"file_path": "/fr.png", "iso_639_1": "fr", "vote_average": 9.0}, {"file_path": "/en.png", "iso_639_1": "en", "vote_average": 5.0}]
            },
            "credits": {
                "cast": [{"id": 6384, "name": "Keanu Reeves", "character": "Neo", "profile_path": "/keanu.jpg"}],
                "crew": [{"name": "Someone", "job": "Producer"}, {"name": "Lana Wachowski", "job": "Director"}]
            },
            "release_dates": {"results": [
                {"iso_3166_1": "GB", "release_dates": [{"certification": "15"}]},
                {"iso_3166_1": "US", "release_dates": [{"certification": ""}, {"certification": "R"}]}
            ]},
            "videos": {"results": [
                {"site": "YouTube", "type": "Featurette", "key": "feat", "official": true},
                {"site": "Vimeo", "type": "Trailer", "key": "vimeo"},
                {"site": "YouTube", "type": "Trailer", "key": "fan", "official": false},
                {"site": "YouTube", "type": "Trailer", "key": "official", "official": true}
            ]},
            "recommendations": {"results": [{"id": 604, "title": "The Matrix Reloaded", "release_date": "2003-05-15"}, {"id": 0, "title": "Broken"}]}
        }));
        let title = parse_movie(&root);
        let details = &title.details;
        assert_eq!(details.backdrop_urls, ["https://image.tmdb.org/t/p/w1280/plain.jpg", "https://image.tmdb.org/t/p/w1280/worded.jpg"]);
        assert_eq!(details.logo_url.as_deref(), Some("https://image.tmdb.org/t/p/w500/en.png"));
        assert_eq!(details.trailer_youtube_id.as_deref(), Some("official"));
        assert_eq!(details.director.as_deref(), Some("Lana Wachowski"));
        assert_eq!(details.age_rating.as_deref(), Some("R"));
        assert_eq!(details.original_title, None, "the same as the title");
        assert_eq!(details.release_year(), Some(1999));
        assert_eq!(details.people[0].tmdb_id.as_deref(), Some("6384"));
        assert_eq!(details.people[0].photo_url.as_deref(), Some("https://image.tmdb.org/t/p/w185/keanu.jpg"));
        assert_eq!(title.recommendations, [TmdbRef { id: 604, title: "The Matrix Reloaded".into(), original_title: None, year: Some(2003) }]);
    }

    #[test]
    fn a_series_reads_with_its_seasons_and_episodes() {
        let root = object(json!({
            "id": 1399, "name": "Paper Boats", "first_air_date": "2021-01-10", "episode_run_time": [],
            "last_episode_to_air": {"runtime": 52}, "networks": [{"name": "Harbor"}], "created_by": [{"name": "Joon Park"}],
            "external_ids": {"imdb_id": "tt7700001"},
            "aggregate_credits": {"cast": [{"id": 9, "name": "Mara Ellison", "roles": [{"character": "Ada"}]}]},
            "content_ratings": {"results": [{"iso_3166_1": "US", "rating": "TV-14"}]},
            "seasons": [
                {"season_number": 0, "name": "Specials", "episode_count": 1},
                {"season_number": 1, "name": "Season 1", "episode_count": 2, "poster_path": "/s1.jpg", "air_date": "2021-01-10"},
                {"season_number": 2, "name": "Season 2", "episode_count": 0}
            ],
            "season/1": {"episodes": [
                {"season_number": 1, "episode_number": 1, "name": "Launch", "overview": "It begins.", "still_path": "/e1.jpg", "runtime": 50, "air_date": "2021-01-10"},
                {"season_number": 1, "episode_number": 2, "name": "", "overview": ""}
            ]}
        }));
        let title = parse_series(&root);
        assert_eq!(title.details.runtime_minutes, Some(52));
        assert_eq!(title.details.people[0].role.as_deref(), Some("Ada"));
        assert_eq!(title.details.seasons.iter().map(|s| s.number).collect::<Vec<_>>(), [1, 2], "specials aren't a season to show");
        assert_eq!(title.season_numbers, [0, 1], "seasons with no episodes aren't asked for");
        let episodes = parse_episodes(&root, &[0, 1]);
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].still_url.as_deref(), Some("https://image.tmdb.org/t/p/w780/e1.jpg"));
        assert_eq!((episodes[1].name.clone(), episodes[1].overview.clone()), (None, None), "blanks are nothing");
    }

    #[test]
    fn a_person_lists_their_work_without_appearances_as_themselves() {
        let root = object(json!({
            "id": 6384, "name": "Keanu Reeves", "known_for_department": "Acting", "biography": "Born in Beirut.",
            "combined_credits": {
                "cast": [
                    {"media_type": "movie", "id": 603, "title": "The Matrix", "release_date": "1999-03-30", "popularity": 80.0},
                    {"media_type": "tv", "id": 22, "name": "Late Talk", "character": "Self - Guest", "popularity": 99.0},
                    {"media_type": "movie", "id": 604, "title": "The Matrix Reloaded", "popularity": 40.0}
                ],
                "crew": [
                    {"media_type": "movie", "id": 603, "title": "The Matrix", "job": "Director", "popularity": 85.0},
                    {"media_type": "movie", "id": 777, "title": "Man of Tai Chi", "job": "Director", "popularity": 10.0},
                    {"media_type": "movie", "id": 778, "title": "Catering", "job": "Caterer", "popularity": 50.0}
                ]
            }
        }));
        let person = parse_person(&root);
        let works: Vec<(i64, f64)> = person.credits.iter().map(|credit| (credit.work.id, credit.popularity)).collect();
        assert_eq!(works, [(603, 85.0), (604, 40.0), (777, 10.0)]);
        assert_eq!(person.department.as_deref(), Some("Acting"));
    }
}
