//! Rows for Home from Trakt: its charts (trending, popular, most watched,
//! box office), any public list (the IMDb Top 250 kept by a
//! Trakt member, someone's favourites) or the connected account's own and
//! liked lists, and what Trakt recommends. Charts and public lists need only
//! the app's key, not a connection.
//!
//! Each title is shown as the sources' copy when they carry it (the first
//! source in the user's order); otherwise, when an add-on can open and play
//! it, as that add-on's title, with Trakt's artwork. When add-ons lead Home,
//! the add-on's title comes first, as on Home's add-on rows: its page lists
//! every add-on's copies and then the sources' own. Trakt's order is kept.
//! Hidden, locked and profile-excluded titles are left out, as on every row.
//! What Trakt says is kept for half an hour, and used for longer when
//! Trakt can't be reached.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde_json::Value;
use xorvio_model::{
    Movie, Series, StreamLocator, TraktChart, TraktListInfo, TraktListQuery, TraktMedia, TraktRow, TraktRowItem, TraktRowSource, TraktRowTitle,
};

use super::api::TraktError;
use super::ids::{self, ADDON_SOURCE, CatalogTitle, Candidates};
use super::sync;
use crate::store::library::TraktTitle;
use crate::{Core, Result, json};

/// How long a chart or list is used before Trakt is asked again.
const FRESH: Duration = Duration::from_secs(30 * 60);
/// Titles asked of a chart: enough that the row fills when the sources lack some.
const CHART_LENGTH: usize = 100;
/// The most items read from one list.
const LONGEST_LIST: usize = 1000;
/// What Trakt sends with each title: its overview and genres (for profiles), and artwork.
const EXTENDED: &str = "extended=full,images";

/// Which titles add-ons can stand in for: those an enabled add-on can both
/// describe and find copies of.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AddonReach {
    pub movies: bool,
    pub series: bool,
    /// Add-ons lead Home: their title is shown even when the sources carry
    /// it too, so its page offers both.
    pub lead: bool,
}

/// A film or show from a Trakt chart or list.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Listed {
    pub title: TraktTitle,
    pub name: String,
    pub overview: Option<String>,
    pub genres: Vec<String>,
    /// On a 10-point scale.
    pub rating: Option<f64>,
    pub runtime_minutes: Option<i64>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
}

impl Core {
    /// A Trakt chart, list or recommendations as titles to show, up to `limit`.
    pub async fn trakt_row(&self, source: &TraktRowSource, limit: usize, addons: AddonReach) -> Result<TraktRow> {
        let path = row_path(source)?;
        let connected = self.trakt.is_connected();
        if matches!(source, TraktRowSource::Recommended { .. }) && !connected {
            return Err(TraktError::NotConnected.into());
        }
        // Home's rows ask Trakt nothing until someone connects an account or
        // chooses to see its charts and lists without one.
        if !connected && !self.trakt_rows_without_account.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(TraktRow { items: Vec::new(), listed: 0 });
        }
        let media = match source {
            TraktRowSource::Chart { media, .. } | TraktRowSource::Recommended { media } => Some(*media),
            TraktRowSource::List { .. } => None,
        };
        let Some(path) = path else { return Ok(TraktRow { items: Vec::new(), listed: 0 }) };
        let reply = self.trakt.get_kept(&path, FRESH).await?;
        let listed = listed_titles(&reply, media);
        Ok(TraktRow { items: self.trakt_row_items(&listed, limit, addons)?, listed: listed.len() as i64 })
    }

    /// Titles for `listed`, in its order: the sources' copies, else add-on
    /// titles (add-on titles first when add-ons lead). A title whose every
    /// copy is hidden, locked or outside the profile is left out, never stood
    /// in for by an add-on.
    pub(super) fn trakt_row_items(&self, listed: &[Listed], limit: usize, addons: AddonReach) -> Result<Vec<TraktRowItem>> {
        let sources = self.trakt_sources();
        let (shows, films): (Vec<&Listed>, Vec<&Listed>) = listed.iter().partition(|item| item.title.show);

        let lookups = self.library.trakt_lookups_found(false)?;
        let keys = ids::match_keys(films.iter().map(|item| &item.title), &lookups);
        let found = self.catalog.movies_matching(&sources, &keys)?;
        let film_candidates = Candidates::new(found.iter().map(CatalogTitle::of_movie).collect(), lookups, &sources);
        let mut catalog_films: HashMap<String, Movie> = self.visible_movies(found)?.into_iter().map(|movie| (movie.id.clone(), movie)).collect();

        let lookups = self.library.trakt_lookups_found(true)?;
        let keys = ids::match_keys(shows.iter().map(|item| &item.title), &lookups);
        let found = self.catalog.series_matching(&sources, &keys)?;
        let show_candidates = Candidates::new(found.iter().map(CatalogTitle::of_series).collect(), lookups, &sources);
        let mut catalog_shows: HashMap<String, Series> = self.visible_series(found)?.into_iter().map(|series| (series.id.clone(), series)).collect();

        let copies = |item: &Listed| -> Vec<String> {
            let candidates = if item.title.show { &show_candidates } else { &film_candidates };
            candidates.matches(&item.title).into_iter().map(|copy| copy.id.clone()).collect()
        };

        // Titles the sources carry with a copy here to show; the others they
        // carry are hidden, locked or outside the profile.
        let visible: HashSet<String> = catalog_films.keys().chain(catalog_shows.keys()).cloned().collect();
        let from_addons = |copies: &[String]| copies.is_empty() || (addons.lead && copies.iter().any(|id| visible.contains(id)));

        // What add-ons would stand in for, judged by the profile as add-on rows are.
        let mut addon_films: Vec<Movie> = Vec::new();
        let mut addon_shows: Vec<Series> = Vec::new();
        for item in listed.iter().filter(|item| from_addons(&copies(item))) {
            match (item.title.show, addon_movie(item), addon_series(item)) {
                (false, Some(movie), _) if addons.movies => addon_films.push(movie),
                (true, _, Some(series)) if addons.series => addon_shows.push(series),
                _ => {}
            }
        }
        self.addon_titles_in_profile(&mut addon_films, &mut addon_shows)?;
        let mut addon_films: HashMap<String, Movie> =
            self.visible_movies(addon_films)?.into_iter().map(|movie| (movie.id.clone(), movie)).collect();
        let mut addon_shows: HashMap<String, Series> =
            self.visible_series(addon_shows)?.into_iter().map(|series| (series.id.clone(), series)).collect();

        let mut chosen: Vec<TraktRowItem> = Vec::new();
        let mut seen = HashSet::new();
        for (index, item) in listed.iter().enumerate() {
            if chosen.len() >= limit {
                break;
            }
            let copies = copies(item);
            // The add-on's title when it's wanted, else (or when the profile
            // leaves it out) the sources' copy.
            let addon_title = if from_addons(&copies) { take_addon_title(item, &mut addon_films, &mut addon_shows) } else { None };
            let title = addon_title.or_else(|| take_catalog_title(item.title.show, &copies, &mut catalog_films, &mut catalog_shows));
            if let Some(title) = title
                && seen.insert(title_id(&title).to_string())
            {
                chosen.push(TraktRowItem { rank: index as i64 + 1, title });
            }
        }
        self.presented(chosen)
    }

    /// Catalog titles as every row shows them (posters filled in, names
    /// tidied); add-on titles as they are.
    fn presented(&self, items: Vec<TraktRowItem>) -> Result<Vec<TraktRowItem>> {
        let (mut movies, mut series) = (Vec::new(), Vec::new());
        for item in &items {
            match &item.title {
                TraktRowTitle::Movie(movie) if movie.source_id != ADDON_SOURCE => movies.push(movie.clone()),
                TraktRowTitle::Series(show) if show.source_id != ADDON_SOURCE => series.push(show.clone()),
                _ => {}
            }
        }
        let mut movies: HashMap<String, Movie> = self.present_movies(movies)?.into_iter().map(|movie| (movie.id.clone(), movie)).collect();
        let mut series: HashMap<String, Series> = self.present_series_list(series)?.into_iter().map(|show| (show.id.clone(), show)).collect();
        Ok(items
            .into_iter()
            .map(|TraktRowItem { rank, title }| TraktRowItem {
                rank,
                title: match title {
                    TraktRowTitle::Movie(movie) => TraktRowTitle::Movie(movies.remove(&movie.id).unwrap_or(movie)),
                    TraktRowTitle::Series(show) => TraktRowTitle::Series(series.remove(&show.id).unwrap_or(show)),
                },
            })
            .collect())
    }

    /// Lists to choose from: the connected account's own or liked ones, the
    /// most liked on Trakt, or those matching words (or the one a trakt.tv
    /// address names).
    pub async fn trakt_lists(&self, query: &TraktListQuery) -> Result<Vec<TraktListInfo>> {
        let trakt = &self.trakt;
        let needs_account = matches!(query, TraktListQuery::Mine | TraktListQuery::Liked);
        if needs_account && !trakt.is_connected() {
            return Err(TraktError::NotConnected.into());
        }
        let reply = match query {
            TraktListQuery::Mine => trakt.get("/users/me/lists").await?,
            TraktListQuery::Liked => trakt.get("/users/likes/lists?limit=100").await?,
            TraktListQuery::Popular => trakt.get_kept("/lists/popular?limit=40", FRESH).await?,
            TraktListQuery::Search { text } => {
                let text = text.trim();
                if text.is_empty() {
                    return Ok(Vec::new());
                }
                if let Some((user, list)) = list_address(text) {
                    return Ok(self.trakt_list(&user, &list).await?.into_iter().collect());
                }
                let words: String = url::form_urlencoded::byte_serialize(text.as_bytes()).collect();
                trakt.get_public(&format!("/search/list?query={words}&limit=30")).await?
            }
        };
        let mut seen = HashSet::new();
        Ok(reply
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(list_info)
            .filter(|info| seen.insert((info.user.clone(), info.list.clone())))
            .collect())
    }

    /// One list by its owner and slug or id, as a trakt.tv address names it.
    async fn trakt_list(&self, user: &str, list: &str) -> Result<Option<TraktListInfo>> {
        if list == WATCHLIST {
            return Ok(Some(TraktListInfo {
                user: user.to_string(),
                list: WATCHLIST.into(),
                name: format!("{user}'s Watchlist"),
                description: None,
                owner: user.to_string(),
                item_count: 0,
                likes: 0,
            }));
        }
        let path = if user.is_empty() { format!("/lists/{list}") } else { format!("/users/{user}/lists/{list}") };
        match self.trakt.get_public(&path).await {
            Ok(reply) => Ok(list_info(&reply)),
            Err(TraktError::NotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

/// A person's watchlist, which an address can name like a list.
const WATCHLIST: &str = "watchlist";

/// Where a row's titles are read from; `None` for a chart Trakt doesn't
/// have (box office is films only).
fn row_path(source: &TraktRowSource) -> Result<Option<String>> {
    Ok(Some(match source {
        TraktRowSource::Chart { chart, media } => {
            let media = media_path(*media);
            let chart = match chart {
                TraktChart::Trending => "trending",
                TraktChart::Popular => "popular",
                TraktChart::Watched => "watched/weekly",
                TraktChart::BoxOffice if media == "movies" => "boxoffice",
                TraktChart::BoxOffice => return Ok(None),
            };
            format!("/{media}/{chart}?limit={CHART_LENGTH}&{EXTENDED}")
        }
        TraktRowSource::Recommended { media } => {
            format!("/recommendations/{}?limit={CHART_LENGTH}&ignore_collected=true&ignore_watchlisted=true&{EXTENDED}", media_path(*media))
        }
        TraktRowSource::List { user, list } => {
            if !is_slug(list) || !(user.is_empty() || is_slug(user)) {
                return Err(anyhow::anyhow!("That isn't a Trakt list."));
            }
            match (user.as_str(), list.as_str()) {
                ("", _) => format!("/lists/{list}/items/movie,show?{EXTENDED}"),
                (_, WATCHLIST) => format!("/users/{user}/watchlist?{EXTENDED}"),
                _ => format!("/users/{user}/lists/{list}/items/movie,show?{EXTENDED}"),
            }
        }
    }))
}

fn media_path(media: TraktMedia) -> &'static str {
    match media {
        TraktMedia::Movies => "movies",
        TraktMedia::Shows => "shows",
    }
}

/// Trakt's slugs and ids: letters, digits, `-`, `_` and `.`.
fn is_slug(text: &str) -> bool {
    !text.is_empty() && text.len() <= 200 && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The owner and list a trakt.tv address names: `/users/{user}/lists/{list}`,
/// `/users/{user}/watchlist` or `/lists/{id}` (the owner then empty).
pub(super) fn list_address(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let with_scheme = if text.contains("://") { text.to_string() } else { format!("https://{text}") };
    let address = url::Url::parse(&with_scheme).ok()?;
    let host = address.host_str()?.to_ascii_lowercase();
    if host != "trakt.tv" && !host.ends_with(".trakt.tv") {
        return None;
    }
    let parts: Vec<&str> = address.path_segments()?.filter(|part| !part.is_empty()).collect();
    let (user, list) = match parts.as_slice() {
        ["users", user, "lists", list, ..] => (user.to_string(), list.to_string()),
        ["users", user, "watchlist", ..] => (user.to_string(), WATCHLIST.to_string()),
        ["lists", list, ..] if *list != "official" && *list != "popular" && *list != "trending" => (String::new(), list.to_string()),
        _ => return None,
    };
    (is_slug(&list) && (user.is_empty() || is_slug(&user))).then_some((user, list))
}

/// The films and shows in a chart, list or recommendations reply, in order.
/// Chart items wrap the title (`{"watchers", "movie": {…}}`); popular charts
/// and recommendations are bare titles of `media`; list items say their type,
/// and seasons, episodes and people in a list are passed over.
pub(super) fn listed_titles(reply: &Value, media: Option<TraktMedia>) -> Vec<Listed> {
    reply
        .as_array()
        .into_iter()
        .flatten()
        .take(LONGEST_LIST)
        .filter_map(|item| {
            if let Some(kind) = item.get("type").and_then(Value::as_str)
                && kind != "movie"
                && kind != "show"
            {
                return None;
            }
            let (object, show) = match (item.get("movie"), item.get("show")) {
                (Some(movie), _) if movie.is_object() => (movie, false),
                (_, Some(show)) if show.is_object() => (show, true),
                _ if item.get("ids").is_some() => (item, media? == TraktMedia::Shows),
                _ => return None,
            };
            listed(object, show)
        })
        .collect()
}

fn listed(object: &Value, show: bool) -> Option<Listed> {
    let title = sync::title(Some(object), show)?;
    let name = json::string(object.get("title")).filter(|name| !name.trim().is_empty())?;
    let images = object.get("images");
    Some(Listed {
        title,
        name,
        overview: json::string(object.get("overview")).filter(|text| !text.trim().is_empty()),
        genres: object.get("genres").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(genre_name).collect(),
        rating: json::double(object.get("rating")).filter(|rating| *rating > 0.0),
        runtime_minutes: json::int(object.get("runtime")).filter(|minutes| *minutes > 0),
        poster_url: image(images, "poster"),
        backdrop_url: image(images, "fanart"),
    })
}

/// Trakt's genre slugs ("science-fiction") as words ("Science Fiction").
fn genre_name(slug: &str) -> String {
    slug.split('-')
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map(|first| first.to_uppercase().chain(letters).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The first image of a kind: Trakt sends addresses without their scheme
/// (`media.trakt.tv/images/…`), in a list for each kind.
fn image(images: Option<&Value>, kind: &str) -> Option<String> {
    let value = images?.get(kind)?;
    let text = match value {
        Value::Array(list) => list.iter().find_map(Value::as_str)?,
        Value::String(text) => text.as_str(),
        Value::Object(sizes) => ["medium", "full", "thumb"].iter().find_map(|size| sizes.get(*size).and_then(Value::as_str))?,
        _ => return None,
    };
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    json::web_url(&if text.starts_with("http://") || text.starts_with("https://") { text.to_string() } else { format!("https://{text}") })
}

/// A Trakt film as an add-on title, by its IMDb id.
fn addon_movie(item: &Listed) -> Option<Movie> {
    let imdb = item.title.ids.imdb.clone()?;
    Some(Movie {
        id: ids::addon_movie_id(&imdb),
        source_id: ADDON_SOURCE.into(),
        title: item.name.clone(),
        year: item.title.year,
        poster_url: item.poster_url.clone(),
        backdrop_url: item.backdrop_url.clone(),
        category_id: None,
        rating: item.rating,
        added_at: None,
        plot: item.overview.clone(),
        genre: (!item.genres.is_empty()).then(|| item.genres.join(", ")),
        tmdb_id: item.title.ids.tmdb.map(|id| id.to_string()),
        imdb_id: Some(imdb.clone()),
        duration_seconds: item.runtime_minutes.map(|minutes| minutes * 60),
        stream: StreamLocator::Addon { media_type: "movie".into(), media_id: imdb },
    })
}

/// A Trakt show as an add-on series, by its IMDb id.
fn addon_series(item: &Listed) -> Option<Series> {
    let imdb = item.title.ids.imdb.clone()?;
    Some(Series {
        id: ids::addon_series_id(&imdb),
        source_id: ADDON_SOURCE.into(),
        title: item.name.clone(),
        year: item.title.year,
        poster_url: item.poster_url.clone(),
        backdrop_url: item.backdrop_url.clone(),
        category_id: None,
        rating: item.rating,
        added_at: None,
        plot: item.overview.clone(),
        genre: (!item.genres.is_empty()).then(|| item.genres.join(", ")),
        tmdb_id: item.title.ids.tmdb.map(|id| id.to_string()),
        imdb_id: Some(imdb.clone()),
        remote_id: Some(imdb),
    })
}

/// A list as Trakt describes it, alone or wrapped (`{"list": {…}}`, as
/// liked, popular and search results are).
pub(super) fn list_info(value: &Value) -> Option<TraktListInfo> {
    let list = value.get("list").filter(|list| list.is_object()).unwrap_or(value);
    let name = json::string(list.get("name")).map(|name| name.trim().to_string()).filter(|name| !name.is_empty())?;
    let ids = list.get("ids");
    let trakt_id = json::int(ids.and_then(|ids| ids.get("trakt"))).filter(|id| *id > 0).map(|id| id.to_string());
    let slug = json::string(ids.and_then(|ids| ids.get("slug"))).filter(|slug| is_slug(slug));
    let user = list.get("user").filter(|user| user.is_object());
    let username = user.and_then(|user| {
        json::string(user.get("ids").and_then(|ids| ids.get("slug"))).or_else(|| json::string(user.get("username")))
    });
    let username = username.filter(|name| is_slug(name));
    // A list with an owner is read through them, so a private one of the
    // connected account's can be; an official list (no owner) by its id.
    let (user_slug, list_key) = match (&username, &trakt_id) {
        (Some(username), Some(id)) => (username.clone(), id.clone()),
        (Some(username), None) => (username.clone(), slug?),
        (None, Some(id)) => (String::new(), id.clone()),
        (None, None) => return None,
    };
    let owner = user
        .and_then(|user| json::string(user.get("name")).filter(|name| !name.trim().is_empty()).or_else(|| json::string(user.get("username"))))
        .unwrap_or_else(|| "Trakt".into());
    Some(TraktListInfo {
        user: user_slug,
        list: list_key,
        name,
        description: json::string(list.get("description")).map(|text| text.trim().to_string()).filter(|text| !text.is_empty()),
        owner,
        item_count: json::int(list.get("item_count")).unwrap_or(0).max(0),
        likes: json::int(list.get("likes")).or_else(|| json::int(value.get("like_count"))).unwrap_or(0).max(0),
    })
}

/// The add-on's title for a Trakt film or show, if it's one to show.
fn take_addon_title(item: &Listed, films: &mut HashMap<String, Movie>, shows: &mut HashMap<String, Series>) -> Option<TraktRowTitle> {
    if item.title.show {
        addon_series(item).and_then(|series| shows.remove(&series.id)).map(TraktRowTitle::Series)
    } else {
        addon_movie(item).and_then(|movie| films.remove(&movie.id)).map(TraktRowTitle::Movie)
    }
}

/// The first of `copies` (catalog ids, in the user's source order) that's shown.
fn take_catalog_title(show: bool, copies: &[String], films: &mut HashMap<String, Movie>, shows: &mut HashMap<String, Series>) -> Option<TraktRowTitle> {
    if show {
        copies.iter().find_map(|id| shows.remove(id)).map(TraktRowTitle::Series)
    } else {
        copies.iter().find_map(|id| films.remove(id)).map(TraktRowTitle::Movie)
    }
}

fn title_id(title: &TraktRowTitle) -> &str {
    match title {
        TraktRowTitle::Movie(movie) => &movie.id,
        TraktRowTitle::Series(series) => &series.id,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn chart_list_and_bare_replies_are_read_in_order() {
        let trending = json!([
            {"watchers": 40, "movie": {"title": "Kite Season", "year": 2026, "ids": {"trakt": 10, "imdb": "tt0001010", "tmdb": 1010},
             "overview": "Up it goes.", "genres": ["science-fiction", "drama"], "rating": 7.4, "runtime": 101,
             "images": {"poster": ["media.trakt.tv/images/movies/000/000/010/posters/medium/a.jpg.webp"], "fanart": []}}},
            {"watchers": 30, "movie": {"title": "No Id", "year": 2026, "ids": {}}}
        ]);
        let films = listed_titles(&trending, Some(TraktMedia::Movies));
        assert_eq!(films.len(), 1, "a title without a Trakt id is passed over");
        let kite = &films[0];
        assert_eq!((kite.name.as_str(), kite.title.year, kite.title.show), ("Kite Season", Some(2026), false));
        assert_eq!(kite.genres, ["Science Fiction", "Drama"]);
        assert_eq!(kite.poster_url.as_deref(), Some("https://media.trakt.tv/images/movies/000/000/010/posters/medium/a.jpg.webp"));
        assert_eq!(kite.backdrop_url, None);

        let popular = json!([{"title": "Far Shore", "year": 2020, "ids": {"trakt": 5, "imdb": "tt0000501"}}]);
        assert!(listed_titles(&popular, Some(TraktMedia::Shows))[0].title.show, "bare titles are of the chart's kind");
        assert!(listed_titles(&popular, None).is_empty(), "a list never has bare titles");

        let list = json!([
            {"rank": 1, "type": "show", "show": {"title": "Far Shore", "year": 2020, "ids": {"trakt": 5}}},
            {"rank": 2, "type": "season", "season": {"number": 1, "ids": {"trakt": 99}}, "show": {"title": "Far Shore", "year": 2020, "ids": {"trakt": 5}}},
            {"rank": 3, "type": "person", "person": {"name": "Someone", "ids": {"trakt": 7}}},
            {"rank": 4, "type": "movie", "movie": {"title": "Kite Season", "year": 2026, "ids": {"trakt": 10}}}
        ]);
        let items = listed_titles(&list, None);
        assert_eq!(items.iter().map(|item| (item.name.as_str(), item.title.show)).collect::<Vec<_>>(), [("Far Shore", true), ("Kite Season", false)]);
    }

    #[test]
    fn row_items_reach_the_interface_as_types_ts_says() {
        let listed = listed_titles(&json!([{"title": "Kite Season", "year": 2026, "ids": {"trakt": 10, "imdb": "tt0001010"}}]), Some(TraktMedia::Movies));
        let item = TraktRowItem { rank: 3, title: TraktRowTitle::Movie(addon_movie(&listed[0]).unwrap()) };
        let value = serde_json::to_value(&item).unwrap();
        assert_eq!((value["rank"].as_i64(), value["movie"]["id"].as_str()), (Some(3), Some("addon|movie|tt0001010")));
        assert_eq!(serde_json::from_value::<TraktRowItem>(value).unwrap(), item);
    }

    #[test]
    fn lists_are_described_alone_or_wrapped() {
        let liked = json!({"liked_at": "2026-01-01T00:00:00.000Z", "type": "list", "list": {
            "name": "IMDb Top 250", "description": "The top rated.", "item_count": 250, "likes": 1200,
            "ids": {"trakt": 2142753, "slug": "imdb-top-rated-movies"},
            "user": {"username": "justin", "name": "Justin N.", "ids": {"slug": "justin"}}}});
        let info = list_info(&liked).unwrap();
        assert_eq!((info.user.as_str(), info.list.as_str(), info.name.as_str()), ("justin", "2142753", "IMDb Top 250"));
        assert_eq!((info.owner.as_str(), info.item_count, info.likes), ("Justin N.", 250, 1200));

        let official = json!({"name": "Star Wars", "item_count": 12, "ids": {"trakt": 77, "slug": "star-wars"}, "user": null});
        let info = list_info(&official).unwrap();
        assert_eq!((info.user.as_str(), info.list.as_str(), info.owner.as_str()), ("", "77", "Trakt"));
        assert_eq!(list_info(&json!({"name": "", "ids": {"trakt": 1}})), None);
    }

    #[test]
    fn trakt_addresses_name_lists() {
        assert_eq!(list_address("https://trakt.tv/users/justin/lists/imdb-top-rated-movies?sort=rank,asc"), Some(("justin".into(), "imdb-top-rated-movies".into())));
        assert_eq!(list_address("trakt.tv/users/justin/lists/imdb-top-rated-movies"), Some(("justin".into(), "imdb-top-rated-movies".into())));
        assert_eq!(list_address("https://app.trakt.tv/users/sean/watchlist"), Some(("sean".into(), WATCHLIST.into())));
        assert_eq!(list_address("https://trakt.tv/lists/2142753"), Some((String::new(), "2142753".into())));
        assert_eq!(list_address("https://trakt.tv/lists/popular"), None);
        assert_eq!(list_address("https://example.com/users/justin/lists/top"), None);
        assert_eq!(list_address("imdb top 250"), None);
    }

    #[test]
    fn rows_read_from_trakts_paths() {
        let path = |source: TraktRowSource| row_path(&source).unwrap();
        let chart = |chart, media| TraktRowSource::Chart { chart, media };
        assert_eq!(path(chart(TraktChart::Trending, TraktMedia::Movies)).unwrap(), "/movies/trending?limit=100&extended=full,images");
        assert_eq!(path(chart(TraktChart::Watched, TraktMedia::Shows)).unwrap(), "/shows/watched/weekly?limit=100&extended=full,images");
        assert_eq!(path(chart(TraktChart::BoxOffice, TraktMedia::Shows)), None, "box office is films only");
        let list = |user: &str, list: &str| TraktRowSource::List { user: user.into(), list: list.into() };
        assert_eq!(path(list("justin", "2142753")).unwrap(), "/users/justin/lists/2142753/items/movie,show?extended=full,images");
        assert_eq!(path(list("", "77")).unwrap(), "/lists/77/items/movie,show?extended=full,images");
        assert_eq!(path(list("sean", WATCHLIST)).unwrap(), "/users/sean/watchlist?extended=full,images");
        assert!(row_path(&list("../sync", "x")).is_err(), "only slugs make a path");
    }
}
