//! Jellyfin and Emby. Jellyfin began as a fork of Emby, so the two still
//! share most of their API: the same sign-in, item lists, images and
//! per-user data. They differ in a few places, all kept here:
//!
//! - Jellyfin 10.9 moved the per-user routes (`/Users/{user}/Items` became
//!   `/Items?userId=`, `/Users/{user}/PlayedItems/{item}` became
//!   `/UserPlayedItems/{item}`…) and later versions dropped the old ones.
//!   Emby only has the old ones. Jellyfin is asked at the new routes, and at
//!   the old ones when a server doesn't have those, so 10.8 still works.
//! - Jellyfin takes the token only in the `Authorization` header. Emby takes
//!   that too, and its own `X-Emby-Authorization` and `X-Emby-Token`.
//! - Emby leaves a title's year, rating and last-played date out of lists
//!   unless they're asked for by name; Jellyfin ignores names it doesn't
//!   know, so both are asked for the same fields.
//!
//! The token never goes into an address: requests carry it in a header, and
//! artwork is kept as the server's own path behind a `xorvio-server` address
//! ([`artwork_url`]), which the app fetches with the header added.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use futures_util::{StreamExt, stream};
use reqwest::Method;
use serde_json::{Value, json};
use xorvio_model::{
    CastMember, Category, CategoryKind, Episode, MediaKind, Movie, SeasonInfo, Series, SourceKind, StreamLocator,
    StreamTarget, Timestamp, VideoFormat,
};

use super::server::{ServerLogin, ServerProgress, ServerSession, artwork_url};
use super::{ids, phase, xtream, xtream_details};
use crate::details::DetailsRecord;
use crate::error::SourceError;
use crate::http::{ApiCall, Http};
use crate::json::{self, Object};
use crate::snapshot::{CatalogSnapshot, KeptKind};

/// Items asked for per request. A page this size comes back quickly even
/// from a small NAS, and large libraries simply take more pages.
const PAGE_SIZE: usize = 300;
/// Pages asked for together, after the first has said how many there are.
const PAGES_AT_ONCE: usize = 3;
/// More items than any real library holds; a server that claims more (or
/// ignores paging) is read only this far.
const MAX_ITEMS: usize = 500_000;
/// Servers count time in ticks of 100 nanoseconds.
const TICKS_PER_SECOND: f64 = 10_000_000.0;
/// People listed with a title, at most.
const MAX_PEOPLE: usize = 30;

/// Fields asked for in film lists. The media sources give the version to
/// play, its container, and its picture and sound for the details page.
const MOVIE_FIELDS: &str = "Overview,Genres,ProviderIds,DateCreated,PremiereDate,ProductionYear,CommunityRating,\
    OfficialRating,Studios,Taglines,ProductionLocations,RemoteTrailers,OriginalTitle,MediaSources,Container,\
    UserDataPlayCount,UserDataLastPlayedDate";
const SERIES_FIELDS: &str = "Overview,Genres,ProviderIds,DateCreated,PremiereDate,ProductionYear,CommunityRating,\
    OfficialRating,Studios,Taglines,ProductionLocations,RemoteTrailers,OriginalTitle,UserDataPlayCount,\
    UserDataLastPlayedDate";
/// Episodes leave out media sources: a library can hold tens of thousands,
/// and the server works out each one's sources, which is slow. The file's
/// extension comes from its path instead, and the server plays its own
/// choice of version when none is named.
const EPISODE_FIELDS: &str =
    "Overview,DateCreated,PremiereDate,ProductionYear,Container,Path,UserDataPlayCount,UserDataLastPlayedDate";
const DETAIL_FIELDS: &str = "Overview,Genres,ProviderIds,DateCreated,PremiereDate,ProductionYear,CommunityRating,\
    OfficialRating,Studios,Taglines,ProductionLocations,RemoteTrailers,OriginalTitle,MediaSources,Container,People";
const SEASON_FIELDS: &str = "Overview,PremiereDate,ChildCount";

// MARK: - Which server

/// The server behind an address, which may not be the one that was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Product {
    Jellyfin,
    Emby,
}

impl Product {
    fn of(kind: SourceKind) -> Self {
        if kind == SourceKind::Emby { Self::Emby } else { Self::Jellyfin }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Jellyfin => "Jellyfin",
            Self::Emby => "Emby",
        }
    }

    /// "a Jellyfin", "an Emby".
    fn with_article(self) -> &'static str {
        match self {
            Self::Jellyfin => "a Jellyfin",
            Self::Emby => "an Emby",
        }
    }

    /// Where a reverse proxy often puts the server ("https://host/emby").
    fn usual_path(self) -> &'static str {
        match self {
            Self::Jellyfin => "jellyfin",
            Self::Emby => "emby",
        }
    }
}

/// What `/System/Info/Public` says: anyone may ask, before signing in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PublicInfo {
    product: Product,
    name: Option<String>,
}

/// Both servers always send their id and version; only Jellyfin names
/// itself ("Jellyfin Server").
fn public_info(value: &Value) -> Option<PublicInfo> {
    let info = value.as_object()?;
    text(info.get("Id"))?;
    text(info.get("Version"))?;
    let product = match json::string(info.get("ProductName")) {
        Some(name) if name.to_lowercase().contains("jellyfin") => Product::Jellyfin,
        _ => Product::Emby,
    };
    Some(PublicInfo { product, name: text(info.get("ServerName")) })
}

/// Addresses to try for a server typed as `base_url`: as typed, without a
/// page of the web app someone pasted ("…/web/index.html"), then under the
/// path a reverse proxy often serves it at, when that isn't there already.
fn candidate_addresses(product: Product, base_url: &str) -> Vec<String> {
    let mut base = base_url.trim_end_matches('/').to_string();
    if let Ok(mut parsed) = url::Url::parse(&base) {
        let segments: Vec<String> = parsed.path_segments().map(|parts| parts.map(str::to_string).collect()).unwrap_or_default();
        if let Some(web) = segments.iter().position(|segment| segment.eq_ignore_ascii_case("web")) {
            parsed.set_path(&format!("/{}", segments[..web].join("/")));
            base = parsed.as_str().trim_end_matches('/').to_string();
        }
    }
    let usual = format!("/{}", product.usual_path());
    let mut candidates = vec![base.clone()];
    if !base.to_lowercase().ends_with(&usual) {
        candidates.push(format!("{base}{usual}"));
    }
    candidates
}

// MARK: - Signing in

/// Signs in with a username and password. The password is sent once, here;
/// what's kept is the token the server gives back.
pub(crate) async fn sign_in(
    http: &Http,
    kind: SourceKind,
    base_url: &str,
    username: &str,
    password: &str,
    device_id: &str,
) -> Result<ServerLogin, SourceError> {
    let username = username.trim();
    if username.is_empty() {
        return Err(SourceError::MissingConfiguration("a username".into()));
    }
    let wanted = Product::of(kind);
    let (address, info) = find_server(http, wanted, base_url, device_id).await?;
    if info.product != wanted {
        // Each is added as what it is, so its own routes are used from the start.
        return Err(SourceError::Unsupported(format!(
            "This is {} server, not {}. Add it as {} source instead.",
            info.product.with_article(),
            wanted.name(),
            info.product.with_article(),
        )));
    }
    let url = endpoint(&address, &["Users", "AuthenticateByName"], &[])?;
    let headers = request_headers(wanted, device_id, None);
    // Jellyfin users may have no password, so an empty one is sent as it is.
    let body = json!({ "Username": username, "Pw": password });
    let reply = http.api(ApiCall::post(&url, &headers, Some(&body))).await?;
    let root = read_json(&reply.bytes)?;
    let token = text(root.get("AccessToken")).ok_or(SourceError::AuthenticationFailed)?;
    let user_id = text(root.get("User").and_then(|user| user.get("Id"))).ok_or(SourceError::AuthenticationFailed)?;
    Ok(ServerLogin { base_url: address, token: Some(token), user_id: Some(user_id), server_name: info.name })
}

/// The first of the candidate addresses that answers as Jellyfin or Emby.
async fn find_server(
    http: &Http,
    wanted: Product,
    base_url: &str,
    device_id: &str,
) -> Result<(String, PublicInfo), SourceError> {
    let headers = request_headers(wanted, device_id, None);
    for address in candidate_addresses(wanted, base_url) {
        let url = endpoint(&address, &["System", "Info", "Public"], &[])?;
        match http.api(ApiCall::get(&url, &headers)).await {
            Ok(reply) => {
                if let Some(info) = read_json(&reply.bytes).ok().as_ref().and_then(public_info) {
                    return Ok((address, info));
                }
            }
            // Nothing answered at all: another path on the same host won't either.
            Err(error @ SourceError::Network(_)) => return Err(error),
            Err(_) => {}
        }
    }
    Err(SourceError::Network(format!(
        "That address answered, but not as {} server. Check the address and port (usually 8096).",
        wanted.with_article()
    )))
}

// MARK: - Requests

/// The headers that sign every request (and artwork) in.
pub(crate) fn auth_headers(session: &ServerSession) -> Vec<(String, String)> {
    request_headers(Product::of(session.kind), &session.device_id, session.token.as_deref())
}

/// Both servers ask every app to say what it is, and take the token in the
/// same header: `MediaBrowser Client="Xorvio", Device="Mac", DeviceId="…",
/// Version="…", Token="…"`. Emby also gets its own header names, which
/// older versions and some proxies rely on.
fn request_headers(product: Product, device_id: &str, token: Option<&str>) -> Vec<(String, String)> {
    let token = token.filter(|token| !token.trim().is_empty());
    let header = client_header(device_id, token);
    let mut headers = vec![("Authorization".to_string(), header.clone())];
    if product == Product::Emby {
        headers.push(("X-Emby-Authorization".to_string(), header));
        if let Some(token) = token {
            headers.push(("X-Emby-Token".to_string(), token.to_string()));
        }
    }
    headers
}

fn client_header(device_id: &str, token: Option<&str>) -> String {
    // Quotes, commas or line breaks would end a value (or the header) early.
    let clean = |value: &str| value.replace(['"', ',', '\r', '\n'], "");
    let mut header = format!(
        r#"MediaBrowser Client="Xorvio", Device="{}", DeviceId="{}", Version="{}""#,
        device_name(),
        clean(device_id),
        env!("CARGO_PKG_VERSION")
    );
    if let Some(token) = token {
        header.push_str(&format!(r#", Token="{}""#, clean(token)));
    }
    header
}

/// How this computer is listed among the server's devices.
fn device_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "Mac",
        "windows" => "Windows PC",
        "linux" => "Linux PC",
        _ => "Computer",
    }
}

/// `base` with path segments (each escaped) and a query added.
fn endpoint(base: &str, segments: &[&str], query: &[(&str, &str)]) -> Result<String, SourceError> {
    let invalid = || SourceError::MissingConfiguration("a server address".into());
    let mut url = url::Url::parse(base).map_err(|_| invalid())?;
    url.path_segments_mut().map_err(|_| invalid())?.pop_if_empty().extend(segments);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Ok(url.into())
}

fn read_json(bytes: &[u8]) -> Result<Value, SourceError> {
    serde_json::from_slice(bytes).map_err(|_| SourceError::Network("The server sent a reply Xorvio couldn't read.".into()))
}

/// The requests whose routes differ between Jellyfin and Emby.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route<'a> {
    Views,
    Items,
    Item(&'a str),
    UserData(&'a str),
    Played(&'a str),
    Seasons(&'a str),
}

impl Route<'_> {
    /// The path: Jellyfin's current one, or the per-user one Emby (and
    /// Jellyfin before 10.9) uses.
    fn segments(self, user: &str, current: bool) -> Vec<String> {
        let parts: &[&str] = match (self, current) {
            (Self::Views, true) => &["UserViews"],
            (Self::Views, false) => &["Users", user, "Views"],
            (Self::Items, true) => &["Items"],
            (Self::Items, false) => &["Users", user, "Items"],
            (Self::Item(id), true) => &["Items", id],
            (Self::Item(id), false) => &["Users", user, "Items", id],
            (Self::UserData(id), true) => &["UserItems", id, "UserData"],
            (Self::UserData(id), false) => &["Users", user, "Items", id, "UserData"],
            (Self::Played(id), true) => &["UserPlayedItems", id],
            (Self::Played(id), false) => &["Users", user, "PlayedItems", id],
            (Self::Seasons(id), _) => &["Shows", id, "Seasons"],
        };
        parts.iter().map(|part| part.to_string()).collect()
    }

    fn differs(self) -> bool {
        !matches!(self, Self::Seasons(_))
    }
}

/// A signed-in session's requests.
struct Client<'a> {
    http: &'a Http,
    session: &'a ServerSession,
    user_id: &'a str,
    headers: Vec<(String, String)>,
    /// Set once a Jellyfin server turns out to have only the older routes,
    /// so the rest of a refresh goes straight to them.
    older_routes: AtomicBool,
}

impl<'a> Client<'a> {
    fn new(http: &'a Http, session: &'a ServerSession) -> Result<Self, SourceError> {
        let user_id = session
            .user_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| SourceError::MissingConfiguration("a sign-in".into()))?;
        Ok(Self { http, session, user_id, headers: auth_headers(session), older_routes: AtomicBool::new(false) })
    }

    async fn get(&self, route: Route<'_>, query: &[(&str, &str)]) -> Result<Value, SourceError> {
        read_json(&self.call(Method::GET, route, query, None).await?)
    }

    /// A request for the signed-in user. Jellyfin is asked at its current
    /// route first, and at the older one when the server doesn't have it.
    async fn call(
        &self,
        method: Method,
        route: Route<'_>,
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Vec<u8>, SourceError> {
        let mut query = query.to_vec();
        query.push(("UserId", self.user_id));
        let current =
            self.session.kind == SourceKind::Jellyfin && route.differs() && !self.older_routes.load(Ordering::Relaxed);
        match self.send(&method, route, current, &query, body).await {
            Err(SourceError::Http(404)) if current => {
                let older = self.send(&method, route, false, &query, body).await;
                if older.is_ok() {
                    self.older_routes.store(true, Ordering::Relaxed);
                }
                older
            }
            result => result,
        }
    }

    async fn send(
        &self,
        method: &Method,
        route: Route<'_>,
        current: bool,
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Vec<u8>, SourceError> {
        let segments = route.segments(self.user_id, current);
        let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
        let url = endpoint(&self.session.base_url, &segments, query)?;
        let call = ApiCall { method: method.clone(), url: &url, headers: &self.headers, json: body };
        Ok(self.http.api(call).await?.bytes)
    }

    /// One page of an item query, and how many items there are in all.
    async fn page(&self, query: &[(&str, &str)], start: usize) -> Result<(Vec<Object>, Option<usize>), SourceError> {
        let (start, limit) = (start.to_string(), PAGE_SIZE.to_string());
        let mut query = query.to_vec();
        query.extend([("StartIndex", start.as_str()), ("Limit", limit.as_str())]);
        let root = self.get(Route::Items, &query).await?;
        let items = json::objects(root.get("Items")).into_iter().cloned().collect();
        let total = json::int(root.get("TotalRecordCount")).and_then(|total| usize::try_from(total).ok());
        Ok((items, total))
    }

    /// Every item a query finds. The first page says how many there are;
    /// the rest are asked for a few at once.
    async fn all_items(&self, query: &[(&str, &str)]) -> Result<Vec<Object>, SourceError> {
        let (mut items, total) = self.page(query, 0).await?;
        match total {
            Some(total) => {
                let starts: Vec<usize> = (PAGE_SIZE..total.min(MAX_ITEMS)).step_by(PAGE_SIZE).collect();
                let mut pages = stream::iter(starts).map(|start| self.page(query, start)).buffered(PAGES_AT_ONCE);
                while let Some(page) = pages.next().await {
                    items.extend(page?.0);
                }
            }
            None => {
                // Without a count, pages are asked for until one comes back short.
                let mut last = items.len();
                while last == PAGE_SIZE && items.len() < MAX_ITEMS {
                    let (page, _) = self.page(query, items.len()).await?;
                    last = page.len();
                    items.extend(page);
                }
            }
        }
        Ok(items)
    }
}

/// An item list's query: one kind of item, anywhere in a library, with the
/// fields and images the catalog uses.
fn list_query<'q>(library: &'q str, item_type: &'q str, fields: &'q str) -> Vec<(&'q str, &'q str)> {
    vec![
        ("ParentId", library),
        ("IncludeItemTypes", item_type),
        ("Recursive", "true"),
        ("Fields", fields),
        ("EnableImageTypes", "Primary,Backdrop,Logo,Thumb"),
        ("ImageTypeLimit", "1"),
        ("EnableUserData", "true"),
        ("SortBy", "SortName"),
        ("SortOrder", "Ascending"),
        ("IsMissing", "false"),
    ]
}

// MARK: - The catalog

/// A library the person can see, as the server lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Library {
    pub id: String,
    pub name: String,
    pub holds: Holds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Holds {
    Films,
    Shows,
    /// A library of mixed content, which may hold both.
    Both,
}

impl Holds {
    fn films(self) -> bool {
        self != Self::Shows
    }

    fn shows(self) -> bool {
        self != Self::Films
    }
}

/// Film and TV libraries from the user's views. Music, photos, books,
/// playlists, collections, live TV and channels aren't listed.
fn libraries(root: &Value) -> Vec<Library> {
    json::objects(root.get("Items"))
        .into_iter()
        .filter_map(|raw| {
            let id = text(raw.get("Id"))?;
            let collection = json::string(raw.get("CollectionType")).unwrap_or_default().to_lowercase();
            let is_library = json::string(raw.get("Type")).as_deref() == Some("CollectionFolder");
            let holds = match collection.as_str() {
                "movies" => Holds::Films,
                "tvshows" => Holds::Shows,
                "" | "mixed" if is_library => Holds::Both,
                _ => return None,
            };
            Some(Library { id, name: text(raw.get("Name")).unwrap_or_else(|| "Library".into()), holds })
        })
        .collect()
}

/// What a server sent, for [`build`].
#[derive(Debug, Default)]
pub(crate) struct Lists {
    pub libraries: Vec<Library>,
    /// Films and series, each with the id of the library it's in.
    pub movies: Vec<(String, Object)>,
    pub series: Vec<(String, Object)>,
    pub episodes: Vec<Object>,
}

/// The whole library: every film, series and episode (episodes too, so
/// Continue Watching and the next episode can be worked out without asking),
/// with what's been watched. A library that can't be read keeps its titles
/// from the last refresh, and the refresh goes on with the others.
pub(crate) async fn fetch_catalog(
    http: &Http,
    session: &ServerSession,
    source_id: &str,
    report: &(dyn Fn(&str) + Send + Sync),
) -> Result<CatalogSnapshot, SourceError> {
    let client = Client::new(http, session)?;
    let views = client.get(Route::Views, &[]).await?;
    report(phase::READING_CATALOG);
    let mut lists = Lists { libraries: libraries(&views), ..Default::default() };
    let mut failures: Vec<(String, KeptKind, SourceError)> = Vec::new();
    let mut reads = 0;
    for library in lists.libraries.clone() {
        let (film_query, series_query, episode_query) = (
            list_query(&library.id, "Movie", MOVIE_FIELDS),
            list_query(&library.id, "Series", SERIES_FIELDS),
            list_query(&library.id, "Episode", EPISODE_FIELDS),
        );
        let films = async {
            if !library.holds.films() {
                return Ok(None);
            }
            client.all_items(&film_query).await.map(Some)
        };
        let shows = async {
            if !library.holds.shows() {
                return Ok(None);
            }
            let (series, episodes) = tokio::join!(client.all_items(&series_query), client.all_items(&episode_query));
            Ok::<_, SourceError>(Some((series?, episodes?)))
        };
        let (films, shows) = tokio::join!(films, shows);
        reads += usize::from(library.holds.films()) + usize::from(library.holds.shows());
        match films {
            Ok(films) => lists.movies.extend(films.unwrap_or_default().into_iter().map(|item| (library.id.clone(), item))),
            Err(error) => failures.push((library.name.clone(), KeptKind::Movies, error)),
        }
        match shows {
            Ok(Some((series, episodes))) => {
                lists.series.extend(series.into_iter().map(|item| (library.id.clone(), item)));
                lists.episodes.extend(episodes);
            }
            Ok(None) => {}
            Err(error) => failures.push((library.name.clone(), KeptKind::Series, error)),
        }
    }
    // Nothing could be read at all: fail, so the last catalog stays as it was.
    if reads > 0 && failures.len() == reads {
        return Err(failures.swap_remove(0).2);
    }
    let mut snapshot = build(source_id, &lists);
    for (name, kind, error) in failures {
        match kind {
            KeptKind::Movies => snapshot.capabilities.movies = true,
            KeptKind::Series => snapshot.capabilities.series = true,
        }
        snapshot.kept_kinds.insert(kind);
        snapshot.warnings.push(format!(
            "The library “{name}” couldn't be read ({}), so its titles from the last refresh are still shown.",
            error.to_string().trim_end_matches('.')
        ));
    }
    Ok(snapshot)
}

/// The catalog from what the server sent: a category for each film or TV
/// library, every film, series and episode, the details their lists carry,
/// and what's been watched.
pub(crate) fn build(source_id: &str, lists: &Lists) -> CatalogSnapshot {
    let mut snapshot = CatalogSnapshot::default();
    for library in &lists.libraries {
        let has = |items: &[(String, Object)]| items.iter().any(|(id, _)| *id == library.id);
        for (kind, holds, listed) in [
            (CategoryKind::Movie, Holds::Films, has(&lists.movies)),
            (CategoryKind::Series, Holds::Shows, has(&lists.series)),
        ] {
            // A mixed library is a category only for what it actually holds.
            if library.holds == holds || (library.holds == Holds::Both && listed) {
                let sort_index = snapshot.categories.len() as i64;
                snapshot.categories.push(Category {
                    id: ids::category(source_id, kind, &library.id),
                    source_id: source_id.into(),
                    kind,
                    name: library.name.clone(),
                    sort_index,
                });
            }
        }
    }

    let mut seen = HashSet::new();
    for (library, raw) in &lists.movies {
        let Some(movie) = movie(source_id, library, raw) else { continue };
        if !seen.insert(movie.id.clone()) {
            continue;
        }
        snapshot.progress.extend(progress(&movie.id, MediaKind::Movie, None, raw));
        add_details(&mut snapshot, &movie.id, item_details(source_id, raw));
        snapshot.movies.push(movie);
    }

    // The server's series id → the catalog's, for their episodes.
    let mut series_ids: HashMap<String, String> = HashMap::new();
    for (library, raw) in &lists.series {
        let Some(series) = series(source_id, library, raw) else { continue };
        let Some(key) = series.remote_id.clone() else { continue };
        if series_ids.contains_key(&key) {
            continue;
        }
        series_ids.insert(key, series.id.clone());
        add_details(&mut snapshot, &series.id, item_details(source_id, raw));
        snapshot.series.push(series);
    }

    let mut seen = HashSet::new();
    for raw in &lists.episodes {
        let Some(episode) = episode(source_id, &series_ids, raw) else { continue };
        if !seen.insert(episode.id.clone()) {
            continue;
        }
        snapshot.progress.extend(progress(&episode.id, MediaKind::Episode, Some(&episode.series_id), raw));
        snapshot.episodes.push(episode);
    }
    snapshot.episodes.sort_by(|a, b| (&a.series_id, a.season, a.number).cmp(&(&b.series_id, b.season, b.number)));
    snapshot.derive_capabilities();
    snapshot
}

fn add_details(snapshot: &mut CatalogSnapshot, id: &str, details: DetailsRecord) {
    if !details.is_empty() {
        snapshot.details.insert(id.to_string(), details);
    }
}

/// Placeholders for episodes a server knows of but doesn't have.
fn is_missing(raw: &Object) -> bool {
    json::string(raw.get("LocationType")).is_some_and(|location| location.eq_ignore_ascii_case("virtual"))
}

fn movie(source_id: &str, library: &str, raw: &Object) -> Option<Movie> {
    if is_missing(raw) {
        return None;
    }
    let key = text(raw.get("Id"))?;
    let source = media_sources(raw).into_iter().next();
    Some(Movie {
        id: ids::movie(source_id, &key),
        source_id: source_id.into(),
        title: text(raw.get("Name"))?,
        year: year(raw),
        poster_url: poster(source_id, &key, raw),
        backdrop_url: backdrop(source_id, &key, raw),
        category_id: Some(ids::category(source_id, CategoryKind::Movie, library)),
        rating: rating(raw),
        added_at: timestamp(raw.get("DateCreated")),
        plot: text(raw.get("Overview")),
        genre: Some(genres(raw).join(", ")).filter(|genre| !genre.is_empty()),
        tmdb_id: provider_id(raw, "Tmdb"),
        imdb_id: provider_id(raw, "Imdb"),
        duration_seconds: seconds(raw.get("RunTimeTicks")),
        stream: StreamLocator::Server {
            part: source.and_then(|source| text(source.get("Id"))),
            extension: extension(raw),
            key,
        },
    })
}

fn series(source_id: &str, library: &str, raw: &Object) -> Option<Series> {
    let key = text(raw.get("Id"))?;
    Some(Series {
        id: ids::series(source_id, &key),
        source_id: source_id.into(),
        title: text(raw.get("Name"))?,
        year: year(raw),
        poster_url: poster(source_id, &key, raw),
        backdrop_url: backdrop(source_id, &key, raw),
        category_id: Some(ids::category(source_id, CategoryKind::Series, library)),
        rating: rating(raw),
        added_at: timestamp(raw.get("DateCreated")),
        plot: text(raw.get("Overview")),
        genre: Some(genres(raw).join(", ")).filter(|genre| !genre.is_empty()),
        tmdb_id: provider_id(raw, "Tmdb"),
        imdb_id: provider_id(raw, "Imdb"),
        remote_id: Some(key),
    })
}

fn episode(source_id: &str, series_ids: &HashMap<String, String>, raw: &Object) -> Option<Episode> {
    if is_missing(raw) {
        return None;
    }
    let key = text(raw.get("Id"))?;
    let series_id = series_ids.get(&text(raw.get("SeriesId"))?)?.clone();
    let number = json::int(raw.get("IndexNumber")).unwrap_or(0);
    let tags = json::object(raw, "ImageTags");
    let still = ["Primary", "Thumb"].into_iter().find_map(|kind| {
        let tag = text(tags?.get(kind))?;
        Some(image(source_id, &key, kind, None, &tag, 800))
    });
    let part = media_sources(raw).into_iter().next().and_then(|source| text(source.get("Id")));
    Some(Episode {
        id: ids::episode(&series_id, &key),
        series_id,
        source_id: source_id.into(),
        season: json::int(raw.get("ParentIndexNumber")).unwrap_or(1),
        number,
        title: text(raw.get("Name")).unwrap_or_else(|| format!("Episode {number}")),
        plot: text(raw.get("Overview")),
        duration_seconds: seconds(raw.get("RunTimeTicks")),
        still_url: still,
        stream: StreamLocator::Server { part, extension: extension(raw), key },
        air_date: day(raw.get("PremiereDate")).and_then(|date| xtream::day_timestamp(&date)),
    })
}

/// Where the server says a film or episode was left, or that it was watched.
/// A position wins over the watched mark: it's being watched again.
fn progress(item_id: &str, kind: MediaKind, series_id: Option<&str>, raw: &Object) -> Option<ServerProgress> {
    let data = json::object(raw, "UserData")?;
    let position = json::double(data.get("PlaybackPositionTicks")).unwrap_or(0.0).max(0.0) / TICKS_PER_SECOND;
    let played = matches!(data.get("Played"), Some(Value::Bool(true)));
    if position <= 0.0 && !played {
        return None;
    }
    Some(ServerProgress {
        item_id: item_id.to_string(),
        kind,
        series_id: series_id.map(str::to_string),
        position_seconds: position,
        duration_seconds: seconds(raw.get("RunTimeTicks")).unwrap_or(0) as f64,
        finished: played && position <= 0.0,
        updated_at: timestamp(data.get("LastPlayedDate")),
    })
}

// MARK: - Details

/// Fuller details for a film or series, when it's opened: cast with
/// photos, crew, every backdrop, and a series' seasons.
pub(crate) async fn fetch_details(
    http: &Http,
    session: &ServerSession,
    source_id: &str,
    kind: MediaKind,
    key: &str,
) -> Result<DetailsRecord, SourceError> {
    let client = Client::new(http, session)?;
    let item = client.get(Route::Item(key), &[("Fields", DETAIL_FIELDS)]).await?;
    let Some(item) = item.as_object() else {
        return Err(SourceError::Network("The server sent a reply Xorvio couldn't read.".into()));
    };
    let mut details = item_details(source_id, item);
    if kind == MediaKind::Series {
        // The rest of the details are worth showing even without the seasons.
        match client.get(Route::Seasons(key), &[("Fields", SEASON_FIELDS)]).await {
            Ok(root) => details.seasons = seasons(source_id, &root),
            Err(error) => tracing::debug!("a series' seasons couldn't be read from the server: {error}"),
        }
    }
    Ok(details)
}

/// Everything an item's own fields say beyond what a catalog row holds.
/// List entries carry most of it; a full item adds people and backdrops.
fn item_details(source_id: &str, raw: &Object) -> DetailsRecord {
    let key = text(raw.get("Id")).unwrap_or_default();
    let people = json::objects(raw.get("People"));
    let names = |kinds: &[&str], limit: usize| -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for person in &people {
            let Some(name) = text(person.get("Name")) else { continue };
            if person_type(person).is_some_and(|kind| kinds.contains(&kind.as_str())) && !names.contains(&name) {
                names.push(name);
            }
        }
        names.truncate(limit);
        names
    };
    let cast: Vec<CastMember> = people
        .iter()
        .filter(|person| person_type(person).is_some_and(|kind| kind == "actor" || kind == "gueststar"))
        .filter_map(|person| {
            Some(CastMember {
                name: text(person.get("Name"))?,
                role: text(person.get("Role")),
                photo_url: match (text(person.get("Id")), text(person.get("PrimaryImageTag"))) {
                    (Some(id), Some(tag)) => Some(image(source_id, &id, "Primary", None, &tag, 300)),
                    _ => None,
                },
                tmdb_id: None,
            })
        })
        .take(MAX_PEOPLE)
        .collect();
    let mut creators = names(&["creator"], 6);
    if creators.is_empty() {
        creators = names(&["writer"], 6);
    }
    let title = text(raw.get("Name"));
    let streams = media_streams(raw);
    DetailsRecord {
        backdrop_urls: strings(raw.get("BackdropImageTags"))
            .into_iter()
            .enumerate()
            .map(|(index, tag)| image(source_id, &key, "Backdrop", Some(index), &tag, 1920))
            .collect(),
        logo_url: json::object(raw, "ImageTags")
            .and_then(|tags| text(tags.get("Logo")))
            .map(|tag| image(source_id, &key, "Logo", None, &tag, 800)),
        cast: cast.iter().map(|member| member.name.clone()).collect(),
        people: cast,
        director: Some(names(&["director"], 3).join(", ")).filter(|names| !names.is_empty()),
        creators,
        release_date: day(raw.get("PremiereDate")),
        country: strings(raw.get("ProductionLocations")).into_iter().next(),
        age_rating: text(raw.get("OfficialRating")).filter(|rating| rating.chars().count() <= 12),
        trailer_youtube_id: json::objects(raw.get("RemoteTrailers"))
            .into_iter()
            .find_map(|trailer| xtream_details::youtube_id(trailer.get("Url"))),
        genres: genres(raw),
        runtime_minutes: seconds(raw.get("RunTimeTicks")).map(|seconds| (seconds + 30) / 60).filter(|minutes| *minutes > 0),
        imdb_id: provider_id(raw, "Imdb"),
        original_title: text(raw.get("OriginalTitle")).filter(|original| Some(original) != title.as_ref()),
        tagline: strings(raw.get("Taglines")).into_iter().next(),
        overview: text(raw.get("Overview")),
        network: json::objects(raw.get("Studios")).into_iter().find_map(|studio| text(studio.get("Name"))),
        video: video_format(&streams),
        audio_channels: audio_channels(&streams),
        seasons: Vec::new(),
        attribution: None,
        ratings: Vec::new(),
        videos: Vec::new(),
    }
}

/// A person's role in lower case ("actor", "director", "gueststar").
fn person_type(person: &Object) -> Option<String> {
    json::string(person.get("Type")).map(|kind| kind.to_lowercase())
}

/// A series' seasons from `/Shows/{id}/Seasons`, in order.
fn seasons(source_id: &str, root: &Value) -> Vec<SeasonInfo> {
    let mut seasons: Vec<SeasonInfo> = json::objects(root.get("Items"))
        .into_iter()
        .filter(|raw| !is_missing(raw))
        .filter_map(|raw| {
            let key = text(raw.get("Id"))?;
            let cover = json::object(raw, "ImageTags").and_then(|tags| text(tags.get("Primary")));
            Some(SeasonInfo {
                number: json::int(raw.get("IndexNumber"))?,
                name: text(raw.get("Name")),
                overview: text(raw.get("Overview")),
                air_date: day(raw.get("PremiereDate")),
                cover_url: cover.map(|tag| image(source_id, &key, "Primary", None, &tag, 400)),
                episode_count: json::int(raw.get("ChildCount")).filter(|count| *count > 0),
            })
        })
        .collect();
    seasons.sort_by_key(|season| season.number);
    seasons
}

/// The streams of the version that plays (the first media source), or the
/// item's own list when there are no sources.
fn media_streams(raw: &Object) -> Vec<&Object> {
    match media_sources(raw).into_iter().next() {
        Some(source) => json::objects(source.get("MediaStreams")),
        None => json::objects(raw.get("MediaStreams")),
    }
}

fn media_sources(raw: &Object) -> Vec<&Object> {
    json::objects(raw.get("MediaSources"))
}

fn video_format(streams: &[&Object]) -> Option<VideoFormat> {
    let video = streams.iter().find(|stream| stream_type(stream) == "video")?;
    let width = json::int(video.get("Width")).filter(|width| *width > 0)?;
    let height = json::int(video.get("Height")).filter(|height| *height > 0)?;
    // Jellyfin says "HDR" with a type ("HDR10", "DOVI"); Emby says "HDR 10" or "Dolby Vision".
    let range = ["VideoRange", "VideoRangeType", "ColorTransfer"]
        .iter()
        .filter_map(|key| json::string(video.get(*key)))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let is_hdr = ["hdr", "dovi", "dolby", "hlg", "2084", "b67"].iter().any(|mark| range.contains(mark));
    Some(VideoFormat { width, height, codec: text(video.get("Codec")).map(|codec| codec.to_lowercase()), is_hdr })
}

/// The default sound track's channels, or the first one's.
fn audio_channels(streams: &[&Object]) -> Option<i64> {
    let audio: Vec<&&Object> = streams.iter().filter(|stream| stream_type(stream) == "audio").collect();
    let chosen = audio.iter().find(|stream| matches!(stream.get("IsDefault"), Some(Value::Bool(true)))).or(audio.first())?;
    json::int(chosen.get("Channels")).filter(|channels| *channels > 0)
}

fn stream_type(stream: &Object) -> String {
    json::string(stream.get("Type")).unwrap_or_default().to_lowercase()
}

// MARK: - Playing and what's been watched

/// The film or episode's file as it is (`static=true`: no transcoding on
/// the server; Xorvio's gateway converts what it must). The token goes in
/// the headers, which the gateway and ffmpeg send.
pub(crate) fn stream_target(session: &ServerSession, key: &str, part: Option<&str>) -> Result<StreamTarget, SourceError> {
    let mut query = vec![("static", "true")];
    if let Some(part) = part.filter(|part| !part.trim().is_empty()) {
        query.push(("MediaSourceId", part));
    }
    let url = endpoint(&session.base_url, &["Videos", key, "stream"], &query)?;
    Ok(StreamTarget { url, user_agent: None, headers: auth_headers(session), is_live: false })
}

/// Sets the server's resume point to where Xorvio left off, as of now, or
/// marks it watched when it was finished. The position is set as it is:
/// the server's own rules (ignore the first few percent, count the last
/// few as watched) only apply to its own players, and Xorvio has decided
/// those already. A title watched before keeps that mark while it's
/// watched again, as the server's own players do.
pub(crate) async fn report_progress(
    http: &Http,
    session: &ServerSession,
    key: &str,
    position_seconds: f64,
    _duration_seconds: f64,
    finished: bool,
) -> Result<(), SourceError> {
    if finished {
        return set_played(http, session, key, true).await;
    }
    let client = Client::new(http, session)?;
    let body = json!({
        "PlaybackPositionTicks": ticks(position_seconds),
        "LastPlayedDate": Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
    });
    client.call(Method::POST, Route::UserData(key), &[], Some(&body)).await.map(drop)
}

/// Marks a film or episode watched (which also clears its resume point) or
/// unwatched. Marking it watched again doesn't count another play.
pub(crate) async fn set_played(http: &Http, session: &ServerSession, key: &str, played: bool) -> Result<(), SourceError> {
    let client = Client::new(http, session)?;
    let method = if played { Method::POST } else { Method::DELETE };
    client.call(method, Route::Played(key), &[], None).await.map(drop)
}

// MARK: - Reading fields

/// Text that says something: trimmed, not empty.
fn text(value: Option<&Value>) -> Option<String> {
    json::string(value).map(|text| text.trim().to_string()).filter(|text| !text.is_empty())
}

/// A list of strings, without empty ones.
fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items.iter().filter_map(|item| text(Some(item))).collect(),
        _ => Vec::new(),
    }
}

fn genres(raw: &Object) -> Vec<String> {
    strings(raw.get("Genres"))
}

/// An id from `ProviderIds` ("Tmdb", "Imdb"), whatever its case.
fn provider_id(raw: &Object, name: &str) -> Option<String> {
    json::object(raw, "ProviderIds")?.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).and_then(|(_, id)| text(Some(id)))
}

/// The year it came out, or the year of its first showing.
fn year(raw: &Object) -> Option<i64> {
    json::int(raw.get("ProductionYear"))
        .or_else(|| day(raw.get("PremiereDate"))?.get(..4)?.parse().ok())
        .filter(|year| (1870..=2200).contains(year))
}

/// The community rating, already out of 10, to one decimal place.
fn rating(raw: &Object) -> Option<f64> {
    json::double(raw.get("CommunityRating")).filter(|rating| *rating > 0.0 && *rating <= 10.0).map(|rating| (rating * 10.0).round() / 10.0)
}

/// Whole seconds from a tick count.
fn seconds(value: Option<&Value>) -> Option<i64> {
    let ticks = json::double(value)?;
    json::finite_int((ticks / TICKS_PER_SECOND).round()).filter(|seconds| *seconds > 0)
}

fn ticks(seconds: f64) -> i64 {
    json::finite_int((seconds.max(0.0) * TICKS_PER_SECOND).round()).unwrap_or(0)
}

/// A server time ("2024-05-01T10:20:30.1234567Z") as Unix seconds. Unset
/// dates are written as the year 1, so anything before 1900 isn't a date.
fn timestamp(value: Option<&Value>) -> Option<Timestamp> {
    let text = text(value)?;
    let seconds = DateTime::parse_from_rfc3339(&text)
        .map(|time| time.timestamp())
        .or_else(|_| NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M:%S%.f").map(|time| time.and_utc().timestamp()))
        .ok()?;
    (seconds > -2_208_988_800).then_some(seconds)
}

/// The day part of a server date ("2020-04-12T00:00:00.0000000Z" → "2020-04-12").
fn day(value: Option<&Value>) -> Option<String> {
    let text = text(value)?;
    let day = text.get(..10)?;
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    (date.format("%Y").to_string().parse::<i64>().ok()? >= 1870).then(|| day.to_string())
}

/// The file's container as an extension ("mkv", "mp4"): the media source's,
/// else the file's own, else the item's. Jellyfin writes an item's
/// container as ffprobe names it ("mov,mp4,m4a,3gp,3g2,mj2").
fn extension(raw: &Object) -> Option<String> {
    let plain = |text: &str| {
        let text = text.trim().to_lowercase();
        (!text.is_empty() && text.len() <= 8 && text.chars().all(|c| c.is_ascii_alphanumeric())).then_some(text)
    };
    let source = media_sources(raw).into_iter().next().and_then(|source| text(source.get("Container")));
    let from_path = text(raw.get("Path")).and_then(|path| {
        let name = path.rsplit(['/', '\\']).next()?.to_string();
        name.rsplit_once('.').map(|(_, extension)| extension.to_string())
    });
    let listed = text(raw.get("Container")).map(|list| {
        let names: Vec<&str> = list.split(',').map(str::trim).collect();
        match names.as_slice() {
            [_, _, ..] if names.contains(&"mp4") => "mp4".to_string(),
            [first, ..] if first.contains("matroska") => "mkv".to_string(),
            [first, ..] => first.to_string(),
            [] => String::new(),
        }
    });
    [source, from_path, listed].into_iter().flatten().find_map(|candidate| plain(&candidate))
}

/// The film or series' poster.
fn poster(source_id: &str, key: &str, raw: &Object) -> Option<String> {
    let tag = text(json::object(raw, "ImageTags")?.get("Primary"))?;
    Some(image(source_id, key, "Primary", None, &tag, 600))
}

/// The first backdrop, or the wide thumbnail when there's none.
fn backdrop(source_id: &str, key: &str, raw: &Object) -> Option<String> {
    if let Some(tag) = strings(raw.get("BackdropImageTags")).into_iter().next() {
        return Some(image(source_id, key, "Backdrop", Some(0), &tag, 1920));
    }
    let tag = text(json::object(raw, "ImageTags")?.get("Thumb"))?;
    Some(image(source_id, key, "Thumb", None, &tag, 1920))
}

/// One of an item's images as a `xorvio-server` address, sized for where it's
/// shown. The tag changes when the image does, so caches stay right.
fn image(source_id: &str, item_id: &str, kind: &str, index: Option<usize>, tag: &str, max_width: u32) -> String {
    let escape = |text: &str| url::form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>();
    let index = index.map(|index| format!("/{index}")).unwrap_or_default();
    let path = format!("/Items/{}/Images/{kind}{index}?tag={}&maxWidth={max_width}&quality=90", escape(item_id), escape(tag));
    artwork_url(source_id, &path)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::sources::server::parse_artwork_path;

    const TOKEN: &str = "a9c1token5f0e7d2b";

    /// A reply captured from the test servers (Scripts/test-servers/jellyfin-emby.sh).
    fn fixture(name: &str) -> Value {
        let path = format!("{}/src/sources/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    fn items(value: &Value) -> Vec<Object> {
        json::objects(value.get("Items")).into_iter().cloned().collect()
    }

    /// A server's whole catalog as the fixtures have it.
    fn lists(server: &str) -> Lists {
        let libraries = libraries(&fixture(&format!("{server}-views")));
        let library = |holds: Holds| libraries.iter().find(|library| library.holds == holds).unwrap().id.clone();
        let (films, shows) = (library(Holds::Films), library(Holds::Shows));
        Lists {
            movies: items(&fixture(&format!("{server}-movies"))).into_iter().map(|item| (films.clone(), item)).collect(),
            series: items(&fixture(&format!("{server}-series"))).into_iter().map(|item| (shows.clone(), item)).collect(),
            episodes: items(&fixture(&format!("{server}-episodes"))),
            libraries,
        }
    }

    fn object(value: Value) -> Object {
        value.as_object().unwrap().clone()
    }

    /// The server path behind an artwork address.
    fn server_path(address: &str) -> String {
        let parsed = url::Url::parse(address).unwrap();
        let (source, path) = parse_artwork_path(parsed.path()).unwrap();
        assert_eq!(source, "src");
        path
    }

    #[test]
    fn public_info_tells_the_servers_apart() {
        let jellyfin = public_info(&fixture("jellyfin-public-info")).unwrap();
        assert_eq!(jellyfin.product, Product::Jellyfin);
        assert!(jellyfin.name.is_some());
        // Emby doesn't name itself.
        assert_eq!(public_info(&fixture("emby-public-info")).unwrap().product, Product::Emby);
        for other in [json!({}), json!([]), json!({"Id": "x"}), json!("<html>"), json!({"Id": "", "Version": "1"})] {
            assert_eq!(public_info(&other), None, "{other}");
        }
    }

    #[test]
    fn addresses_to_try() {
        assert_eq!(candidate_addresses(Product::Emby, "http://nas:8096"), ["http://nas:8096", "http://nas:8096/emby"]);
        assert_eq!(candidate_addresses(Product::Emby, "https://media.example/Emby/"), ["https://media.example/Emby"]);
        assert_eq!(
            candidate_addresses(Product::Jellyfin, "http://nas:8096/web/index.html"),
            ["http://nas:8096", "http://nas:8096/jellyfin"]
        );
        assert_eq!(candidate_addresses(Product::Jellyfin, "https://media.example/jellyfin/web"), ["https://media.example/jellyfin"]);
    }

    #[test]
    fn headers_say_what_xorvio_is_and_carry_the_token() {
        let jellyfin = request_headers(Product::Jellyfin, "device-1", Some(TOKEN));
        assert_eq!(jellyfin.len(), 1, "Jellyfin takes only the standard header");
        let (name, value) = &jellyfin[0];
        assert_eq!(name, "Authorization");
        assert!(value.starts_with(r#"MediaBrowser Client="Xorvio", Device=""#));
        assert!(value.contains(r#"DeviceId="device-1""#) && value.ends_with(&format!(r#"Token="{TOKEN}""#)));
        let emby = request_headers(Product::Emby, "device-1", Some(TOKEN));
        let names: Vec<&str> = emby.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["Authorization", "X-Emby-Authorization", "X-Emby-Token"]);
        let signed_out = request_headers(Product::Emby, "dev\"ice, 2", None);
        assert_eq!(signed_out.len(), 2);
        assert!(!signed_out[0].1.contains("Token") && signed_out[0].1.contains(r#"DeviceId="device 2""#));
        assert_eq!(request_headers(Product::Jellyfin, "d", Some(" ")), request_headers(Product::Jellyfin, "d", None));
    }

    #[test]
    fn routes_for_each_server() {
        let path = |route: Route<'_>, current: bool| route.segments("u1", current).join("/");
        assert_eq!(path(Route::Views, true), "UserViews");
        assert_eq!(path(Route::Views, false), "Users/u1/Views");
        assert_eq!(path(Route::Items, true), "Items");
        assert_eq!(path(Route::Items, false), "Users/u1/Items");
        assert_eq!(path(Route::Item("i9"), true), "Items/i9");
        assert_eq!(path(Route::Item("i9"), false), "Users/u1/Items/i9");
        assert_eq!(path(Route::UserData("i9"), true), "UserItems/i9/UserData");
        assert_eq!(path(Route::UserData("i9"), false), "Users/u1/Items/i9/UserData");
        assert_eq!(path(Route::Played("i9"), true), "UserPlayedItems/i9");
        assert_eq!(path(Route::Played("i9"), false), "Users/u1/PlayedItems/i9");
        assert_eq!(path(Route::Seasons("s1"), true), path(Route::Seasons("s1"), false));
        assert_eq!(
            endpoint("http://nas:8096/emby", &["Items", "a/b"], &[("Fields", "A,B")]).unwrap(),
            "http://nas:8096/emby/Items/a%2Fb?Fields=A%2CB"
        );
    }

    #[test]
    fn jellyfin_catalog_from_real_replies() {
        let snapshot = build("src", &lists("jellyfin"));
        let categories: Vec<(&str, CategoryKind, &str)> =
            snapshot.categories.iter().map(|c| (c.id.as_str(), c.kind, c.name.as_str())).collect();
        assert_eq!(categories, [
            ("src|moviecat|db4c1708cbb5dd1676284a40f2950aba", CategoryKind::Movie, "Films"),
            ("src|seriescat|a656b907eb3a73532e40e44b968d0225", CategoryKind::Series, "Shows"),
        ]);

        let one = snapshot.movies.iter().find(|movie| movie.title == "Test Film One").unwrap();
        assert_eq!(one.id, "src|mv|73b9bbbd0dd0de91c34825af0bde00b9");
        assert_eq!((one.year, one.rating, one.duration_seconds), (Some(2020), Some(7.4), Some(360)));
        assert_eq!(one.added_at, Some(1_793_403_649));
        assert_eq!(one.genre.as_deref(), Some("Science Fiction, Drama"));
        assert_eq!((one.tmdb_id.as_deref(), one.imdb_id.as_deref()), (Some("990001"), Some("tt9900001")));
        assert_eq!(one.category_id.as_deref(), Some("src|moviecat|db4c1708cbb5dd1676284a40f2950aba"));
        assert_eq!(
            one.stream,
            StreamLocator::Server {
                key: "73b9bbbd0dd0de91c34825af0bde00b9".into(),
                part: Some("73b9bbbd0dd0de91c34825af0bde00b9".into()),
                extension: Some("mp4".into()),
            }
        );
        assert_eq!(
            server_path(one.poster_url.as_deref().unwrap()),
            "/Items/73b9bbbd0dd0de91c34825af0bde00b9/Images/Primary?tag=5da423a7f47c6bfd471cab17c2be5b8e&maxWidth=600&quality=90"
        );
        assert_eq!(
            server_path(one.backdrop_url.as_deref().unwrap()),
            "/Items/73b9bbbd0dd0de91c34825af0bde00b9/Images/Backdrop/0?tag=71d1a6f43f396e1d18fe7b856dbfd7fc&maxWidth=1920&quality=90"
        );
        let two = snapshot.movies.iter().find(|movie| movie.title == "Test Film Two").unwrap();
        assert!(matches!(&two.stream, StreamLocator::Server { extension: Some(ext), .. } if ext == "mkv"));

        let details = &snapshot.details[&one.id];
        assert_eq!(details.tagline.as_deref(), Some("Every map is a guess."));
        assert_eq!((details.country.as_deref(), details.age_rating.as_deref()), (Some("Ireland"), Some("PG-13")));
        assert_eq!(details.trailer_youtube_id.as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(details.original_title.as_deref(), Some("Film Un d'Essai"));
        assert_eq!((details.network.as_deref(), details.runtime_minutes), (Some("Lantern Pictures"), Some(6)));
        assert_eq!(details.release_date.as_deref(), Some("2020-04-12"));
        assert!(details.logo_url.is_some() && details.backdrop_urls.len() == 1);
        assert_eq!(details.video, Some(VideoFormat { width: 1280, height: 720, codec: Some("h264".into()), is_hdr: false }));
        assert_eq!(details.audio_channels, Some(2));
        assert_eq!(snapshot.details[&two.id].audio_channels, Some(6));

        let show = &snapshot.series[0];
        assert_eq!(show.id, "src|se|3964b139df4c37d43b8d949fbbc17af9");
        assert_eq!(show.remote_id.as_deref(), Some("3964b139df4c37d43b8d949fbbc17af9"));
        assert_eq!((show.year, show.rating, show.genre.as_deref()), (Some(2019), Some(8.1), Some("Drama, Comedy")));
        assert_eq!(snapshot.details[&show.id].network.as_deref(), Some("Northwind Broadcasting"));

        let episodes: Vec<(i64, i64, &str)> = snapshot.episodes.iter().map(|e| (e.season, e.number, e.title.as_str())).collect();
        assert_eq!(episodes, [(1, 1, "Broadcast 1.1"), (1, 2, "Broadcast 1.2"), (2, 1, "Broadcast 2.1"), (2, 2, "Broadcast 2.2")]);
        let first = &snapshot.episodes[0];
        assert_eq!(first.id, "src|se|3964b139df4c37d43b8d949fbbc17af9|ep|cffde9b76adc04722110d2ccb7555272");
        assert_eq!(first.series_id, show.id);
        assert_eq!((first.duration_seconds, first.air_date), (Some(360), Some(1_546_862_400)));
        assert_eq!(first.plot.as_deref(), Some("Episode 1 of season 1."));
        assert_eq!(
            first.stream,
            StreamLocator::Server { key: "cffde9b76adc04722110d2ccb7555272".into(), part: None, extension: Some("mkv".into()) }
        );
        assert!(server_path(first.still_url.as_deref().unwrap()).starts_with("/Items/cffde9b76adc04722110d2ccb7555272/Images/Primary?tag="));

        // Film One was watched before and is being watched again; the first episode was watched.
        assert_eq!(snapshot.progress, [
            ServerProgress {
                item_id: one.id.clone(),
                kind: MediaKind::Movie,
                series_id: None,
                position_seconds: 120.0,
                duration_seconds: 360.0,
                finished: false,
                updated_at: Some(1_793_304_000),
            },
            ServerProgress {
                item_id: first.id.clone(),
                kind: MediaKind::Episode,
                series_id: Some(show.id.clone()),
                position_seconds: 0.0,
                duration_seconds: 360.0,
                finished: true,
                updated_at: Some(1_793_404_466),
            },
        ]);
        assert!(snapshot.capabilities.movies && snapshot.capabilities.series && !snapshot.capabilities.live);
        assert!(snapshot.warnings.is_empty() && snapshot.kept_kinds.is_empty());
    }

    #[test]
    fn emby_catalog_from_real_replies() {
        let snapshot = build("src", &lists("emby"));
        assert_eq!(snapshot.categories.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["src|moviecat|3", "src|seriescat|5"]);
        let one = snapshot.movies.iter().find(|movie| movie.title == "Test Film One").unwrap();
        assert_eq!(one.id, "src|mv|10");
        // Emby sends year and rating only when asked for them by name.
        assert_eq!((one.year, one.rating, one.duration_seconds), (Some(2020), Some(7.4), Some(360)));
        assert_eq!(
            one.stream,
            StreamLocator::Server { key: "10".into(), part: Some("mediasource_10".into()), extension: Some("mp4".into()) }
        );
        let two = snapshot.movies.iter().find(|movie| movie.title == "Test Film Two").unwrap();
        assert_eq!((two.year, two.rating), (Some(2021), Some(6.1)));
        assert_eq!(snapshot.details[&two.id].video.as_ref().and_then(|video| video.codec.as_deref()), Some("hevc"));
        assert_eq!(snapshot.series[0].remote_id.as_deref(), Some("21"));
        assert_eq!(snapshot.episodes.len(), 4);
        assert_eq!(snapshot.episodes[3].id, "src|se|21|ep|27");
        assert!(matches!(&snapshot.episodes[0].stream, StreamLocator::Server { part: None, extension: Some(ext), .. } if ext == "mkv"));
        // Emby's lists say when something was last played once asked for it.
        let film = snapshot.progress.iter().find(|progress| progress.item_id == one.id).unwrap();
        assert_eq!((film.position_seconds, film.finished, film.updated_at), (120.0, false, Some(1_793_304_000)));
        let episode = snapshot.progress.iter().find(|progress| progress.kind == MediaKind::Episode).unwrap();
        assert_eq!((episode.item_id.as_str(), episode.finished), ("src|se|21|ep|24", true));
        assert_eq!(snapshot.progress.len(), 2);
    }

    #[test]
    fn nothing_in_the_catalog_holds_a_token_or_points_elsewhere() {
        for server in ["jellyfin", "emby"] {
            let snapshot = build("src", &lists(server));
            let artwork: Vec<&String> = snapshot
                .movies
                .iter()
                .flat_map(|movie| [&movie.poster_url, &movie.backdrop_url])
                .chain(snapshot.series.iter().flat_map(|series| [&series.poster_url, &series.backdrop_url]))
                .chain(snapshot.episodes.iter().map(|episode| &episode.still_url))
                .flatten()
                .chain(snapshot.details.values().flat_map(|details| details.backdrop_urls.iter().chain(&details.logo_url)))
                .collect();
            assert!(artwork.len() > 10);
            for address in artwork {
                assert!(address.starts_with("xorvio-server://localhost/src/") || address.starts_with("http://xorvio-server.localhost/src/"));
                assert!(server_path(address).starts_with("/Items/"), "{address}");
            }
            let everything = format!("{:?}{:?}{:?}{:?}", snapshot.movies, snapshot.series, snapshot.episodes, snapshot.details);
            for secret in ["Token", "token", "api_key", "X-Emby"] {
                assert!(!everything.contains(secret), "{server}: {secret}");
            }
        }
    }

    #[test]
    fn details_from_real_replies() {
        for server in ["jellyfin", "emby"] {
            let details = item_details("src", fixture(&format!("{server}-movie-details")).as_object().unwrap());
            let cast: Vec<(&str, Option<&str>)> =
                details.people.iter().map(|person| (person.name.as_str(), person.role.as_deref())).collect();
            assert_eq!(cast, [("Mara Ellison", Some("Ada Quill")), ("Tomas Reyes", Some("The Ranger"))], "{server}");
            assert_eq!(details.cast, ["Mara Ellison", "Tomas Reyes"]);
            assert_eq!(details.director.as_deref(), Some("Joon Park"));
            assert_eq!(details.creators, ["Ines Okafor"], "writers, for a film");
            assert_eq!(details.genres, ["Science Fiction", "Drama"]);
            assert_eq!((details.imdb_id.as_deref(), details.runtime_minutes), (Some("tt9900001"), Some(6)));
            assert_eq!(details.overview.as_deref(), Some("A surveyor maps a canyon that keeps changing shape."));
            assert!(details.logo_url.is_some() && details.backdrop_urls.len() == 1);
            assert_eq!(details.video.as_ref().map(|video| (video.width, video.height)), Some((1280, 720)));

            let show = item_details("src", fixture(&format!("{server}-series-details")).as_object().unwrap());
            assert_eq!(show.people[0].name, "Nadia Brooks");
            assert_eq!((show.age_rating.as_deref(), show.video.as_ref()), (Some("TV-14"), None));
            let seasons = seasons("src", &fixture(&format!("{server}-seasons")));
            assert_eq!(seasons.iter().map(|season| (season.number, season.episode_count)).collect::<Vec<_>>(), [
                (1, Some(2)),
                (2, Some(2))
            ]);
            assert_eq!(seasons[1].name.as_deref(), Some("Season 2"));
            assert_eq!(seasons[0].overview.as_deref(), Some("The station's season 1."));
            assert!(server_path(seasons[0].cover_url.as_deref().unwrap()).contains("maxWidth=400"));
        }
    }

    #[test]
    fn odd_items_are_read_tolerantly() {
        let people = object(json!({
            "Id": "m1", "Name": "Same", "OriginalTitle": "Same", "OfficialRating": "A rating far too long to be one",
            "People": [
                {"Name": "Ann Lee", "Id": "p1", "Role": "Herself", "Type": "Actor", "PrimaryImageTag": "t1"},
                {"Name": "Guest", "Id": "p2", "Type": "GuestStar"},
                {"Name": "Wri Ter", "Id": "p3", "Type": "Writer"},
                {"Name": "Cre Ator", "Id": "p4", "Type": "Creator"},
                {"Name": "Cre Ator", "Id": "p4", "Type": "Creator"},
                {"Id": "p5", "Type": "Actor"}
            ],
            "MediaStreams": [
                {"Type": "Video", "Codec": "HEVC", "Width": 3840, "Height": 2160, "VideoRange": "HDR", "VideoRangeType": "DOVI"},
                {"Type": "Audio", "Channels": 2},
                {"Type": "Audio", "Channels": 8, "IsDefault": true}
            ]
        }));
        let details = item_details("src", &people);
        assert_eq!(details.people.len(), 2);
        assert_eq!(
            server_path(details.people[0].photo_url.as_deref().unwrap()),
            "/Items/p1/Images/Primary?tag=t1&maxWidth=300&quality=90"
        );
        assert_eq!(details.people[1].photo_url, None);
        assert_eq!(details.creators, ["Cre Ator"], "creators before writers, once each");
        assert_eq!((details.original_title, details.age_rating), (None, None));
        assert_eq!(details.video, Some(VideoFormat { width: 3840, height: 2160, codec: Some("hevc".into()), is_hdr: true }));
        assert_eq!(details.audio_channels, Some(8), "the default track");
        let emby_hdr = [object(json!({"Type": "Video", "Width": 1920, "Height": 1080, "VideoRange": "HDR 10"}))];
        assert!(video_format(&emby_hdr.iter().collect::<Vec<_>>()).unwrap().is_hdr);

        let movie = |raw: Value| super::movie("src", "lib", &object(raw));
        assert!(movie(json!({"Id": "x"})).is_none(), "no title");
        assert!(movie(json!({"Name": "No id"})).is_none());
        assert!(movie(json!({"Id": "v", "Name": "Virtual", "LocationType": "Virtual"})).is_none());
        let odd = movie(json!({
            "Id": "m2", "Name": " Odd ", "CommunityRating": 11, "ProductionYear": 0,
            "PremiereDate": "0001-01-01T00:00:00.0000000Z", "ProviderIds": {"tmdb": "5", "IMDB": ""},
            "Genres": ["", " Drama "], "RunTimeTicks": -5, "Container": "mov,mp4,m4a,3gp,3g2,mj2",
            "ImageTags": {"Thumb": "th"}, "DateCreated": "not a date"
        }))
        .unwrap();
        assert_eq!((odd.title.as_str(), odd.rating, odd.year, odd.added_at), ("Odd", None, None, None));
        assert_eq!((odd.tmdb_id.as_deref(), odd.imdb_id.as_deref()), (Some("5"), None));
        assert_eq!((odd.genre.as_deref(), odd.duration_seconds, odd.poster_url.as_deref()), (Some("Drama"), None, None));
        let wide = server_path(odd.backdrop_url.as_deref().unwrap());
        assert!(wide.starts_with("/Items/m2/Images/Thumb?"), "a wide thumbnail stands in: {wide}");
        assert!(matches!(odd.stream, StreamLocator::Server { extension: Some(ref ext), part: None, .. } if ext == "mp4"));
        let dated = movie(json!({"Id": "m3", "Name": "Dated", "PremiereDate": "1999-12-31T23:00:00Z", "Container": "matroska,webm"}));
        let dated = dated.unwrap();
        assert_eq!(dated.year, Some(1999));
        assert!(matches!(dated.stream, StreamLocator::Server { extension: Some(ref ext), .. } if ext == "mkv"));

        let series_ids = HashMap::from([("s1".to_string(), "src|se|s1".to_string())]);
        let episode = |raw: Value| super::episode("src", &series_ids, &object(raw));
        assert!(episode(json!({"Id": "e1", "Name": "Lost", "SeriesId": "elsewhere"})).is_none(), "a series not in the catalog");
        assert!(episode(json!({"Id": "e2", "SeriesId": "s1", "LocationType": "Virtual"})).is_none(), "missing episodes aren't listed");
        let bare = episode(json!({"Id": "e3", "SeriesId": "s1", "IndexNumber": 4, "Path": "C:\\Shows\\Bare.AVI"})).unwrap();
        assert_eq!((bare.season, bare.number, bare.title.as_str(), bare.still_url.as_deref()), (1, 4, "Episode 4", None));
        assert!(matches!(bare.stream, StreamLocator::Server { extension: Some(ref ext), .. } if ext == "avi"));

        // Watched marks: a position wins, nothing is nothing.
        let watch = |data: Value| {
            progress("id", MediaKind::Movie, None, &object(json!({"UserData": data, "RunTimeTicks": 600_000_000})))
        };
        assert_eq!(watch(json!({"Played": false, "PlaybackPositionTicks": 0})), None);
        assert_eq!(watch(json!({})), None);
        let resumed = watch(json!({"Played": true, "PlaybackPositionTicks": 150_000_000, "LastPlayedDate": "2026-10-29T20:00:00Z"}));
        let resumed = resumed.unwrap();
        assert_eq!((resumed.position_seconds, resumed.finished, resumed.duration_seconds), (15.0, false, 60.0));
        let watched = watch(json!({"Played": true, "LastPlayedDate": "0001-01-01T00:00:00.0000000Z"})).unwrap();
        assert_eq!((watched.finished, watched.updated_at), (true, None));
    }

    #[test]
    fn libraries_from_views() {
        let views = json!({"Items": [
            {"Id": "1", "Name": "Films", "CollectionType": "movies", "Type": "CollectionFolder"},
            {"Id": "2", "Name": "TV", "CollectionType": "tvshows", "Type": "CollectionFolder"},
            {"Id": "3", "Name": "Mixed", "Type": "CollectionFolder"},
            {"Id": "4", "Name": "Music", "CollectionType": "music", "Type": "CollectionFolder"},
            {"Id": "5", "Name": "Collections", "CollectionType": "boxsets", "Type": "CollectionFolder"},
            {"Id": "6", "Name": "A plugin channel", "Type": "Channel"},
            {"Id": "7", "Name": "Live TV", "CollectionType": "livetv", "Type": "UserView"},
            {"Id": "8", "Name": "Old mixed", "CollectionType": "mixed", "Type": "CollectionFolder"},
            {"Name": "No id", "CollectionType": "movies"}
        ]});
        let found: Vec<(String, Holds)> = libraries(&views).into_iter().map(|library| (library.name, library.holds)).collect();
        assert_eq!(found, [
            ("Films".to_string(), Holds::Films),
            ("TV".to_string(), Holds::Shows),
            ("Mixed".to_string(), Holds::Both),
            ("Old mixed".to_string(), Holds::Both)
        ]);

        // A mixed library is a category only for what it holds.
        let lists = Lists {
            libraries: libraries(&views),
            movies: vec![("3".into(), object(json!({"Id": "m", "Name": "Film in mixed"})))],
            ..Default::default()
        };
        let snapshot = build("src", &lists);
        let categories: Vec<&str> = snapshot.categories.iter().map(|category| category.id.as_str()).collect();
        assert_eq!(categories, ["src|moviecat|1", "src|seriescat|2", "src|moviecat|3"]);
        assert_eq!(snapshot.movies[0].category_id.as_deref(), Some("src|moviecat|3"));
    }

    #[test]
    fn dates_and_times() {
        let at = |text: &str| timestamp(Some(&json!(text)));
        assert_eq!(at("2026-10-29T20:00:00.0000000Z"), Some(1_793_304_000));
        assert_eq!(at("2026-10-29T20:00:00.1234567Z"), Some(1_793_304_000));
        assert_eq!(at("2026-10-29T20:00:00"), Some(1_793_304_000), "no zone reads as UTC");
        assert_eq!(at("2026-10-29T22:00:00+02:00"), Some(1_793_304_000));
        assert_eq!(at("0001-01-01T00:00:00.0000000Z"), None);
        assert_eq!(at("soon"), None);
        assert_eq!(day(Some(&json!("2020-04-12T00:00:00.0000000Z"))).as_deref(), Some("2020-04-12"));
        assert_eq!(day(Some(&json!("0001-01-01T00:00:00Z"))), None);
        assert_eq!(day(Some(&json!("2020"))), None);
        assert_eq!(seconds(Some(&json!(3_600_210_000_i64))), Some(360));
        assert_eq!((seconds(Some(&json!(0))), seconds(Some(&json!("nan")))), (None, None));
        assert_eq!((ticks(150.5), ticks(-3.0)), (1_505_000_000, 0));
    }

    #[test]
    fn stream_targets_keep_the_token_in_a_header() {
        let target = stream_target(&session(SourceKind::Emby, "http://nas:8096/emby"), "10", Some("mediasource_10")).unwrap();
        assert_eq!(target.url, "http://nas:8096/emby/Videos/10/stream?static=true&MediaSourceId=mediasource_10");
        assert!(!target.is_live && !target.url.contains(TOKEN));
        assert!(target.headers.iter().any(|(name, value)| name == "X-Emby-Token" && value == TOKEN));
        let jellyfin = stream_target(&session(SourceKind::Jellyfin, "http://nas:8096"), "a b", Some(" ")).unwrap();
        assert_eq!(jellyfin.url, "http://nas:8096/Videos/a%20b/stream?static=true");
        assert_eq!(jellyfin.headers.len(), 1);
        assert!(jellyfin.headers[0].1.contains(TOKEN));
    }

    // MARK: Against a stand-in server

    fn session(kind: SourceKind, base: &str) -> ServerSession {
        ServerSession {
            kind,
            base_url: base.into(),
            token: Some(TOKEN.into()),
            user_id: Some("u1".into()),
            device_id: "device-1".into(),
        }
    }

    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        path: String,
        query: HashMap<String, String>,
        headers: HashMap<String, String>,
        body: String,
    }

    impl Seen {
        fn target(&self) -> String {
            let mut query: Vec<String> = self.query.iter().map(|(key, value)| format!("{key}={value}")).collect();
            query.sort();
            format!("{} {}?{}", self.method, self.path, query.join("&"))
        }
    }

    type Log = Arc<Mutex<Vec<Seen>>>;

    /// A server on 127.0.0.1 answering every request with `reply`.
    async fn serve(reply: impl Fn(&Seen) -> (u16, Value) + Send + Sync + 'static) -> (String, Log) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log: Log = Arc::default();
        let (seen, reply) = (log.clone(), Arc::new(reply));
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (seen, reply) = (seen.clone(), reply.clone());
                tokio::spawn(async move {
                    let mut data = Vec::new();
                    let mut buffer = [0u8; 8192];
                    let head_end = loop {
                        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
                            break end + 4;
                        }
                        match socket.read(&mut buffer).await {
                            Ok(0) | Err(_) => return,
                            Ok(count) => data.extend_from_slice(&buffer[..count]),
                        }
                    };
                    let head = String::from_utf8_lossy(&data[..head_end]).to_string();
                    let mut lines = head.lines();
                    let mut first = lines.next().unwrap_or_default().split_whitespace();
                    let method = first.next().unwrap_or_default().to_string();
                    let target = first.next().unwrap_or("/").to_string();
                    let headers: HashMap<String, String> = lines
                        .filter_map(|line| line.split_once(':'))
                        .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_string()))
                        .collect();
                    let length: usize = headers.get("content-length").and_then(|length| length.parse().ok()).unwrap_or(0);
                    while data.len() < head_end + length {
                        match socket.read(&mut buffer).await {
                            Ok(0) | Err(_) => break,
                            Ok(count) => data.extend_from_slice(&buffer[..count]),
                        }
                    }
                    let body = String::from_utf8_lossy(&data[head_end..data.len().min(head_end + length)]).to_string();
                    let parsed = url::Url::parse(&format!("http://test{target}")).unwrap();
                    let request = Seen {
                        method,
                        path: parsed.path().to_string(),
                        query: parsed.query_pairs().map(|(key, value)| (key.into_owned(), value.into_owned())).collect(),
                        headers,
                        body,
                    };
                    seen.lock().unwrap().push(request.clone());
                    let (status, value) = reply(&request);
                    let body = if value.is_null() { String::new() } else { value.to_string() };
                    let head = format!(
                        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(body.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (base, log)
    }

    fn requests(log: &Log) -> Vec<Seen> {
        log.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn signing_in() {
        // An Emby server behind a proxy at /emby, with nothing at the root.
        let (base, log) = serve(|request| match request.path.as_str() {
            "/emby/System/Info/Public" => (200, json!({"Id": "srv", "Version": "4.9.3.0", "ServerName": "Den"})),
            "/emby/Users/AuthenticateByName" if request.body.contains("right") => {
                (200, json!({"AccessToken": TOKEN, "User": {"Id": "u7", "Name": "ann"}}))
            }
            "/emby/Users/AuthenticateByName" => (401, json!("Invalid username or password.")),
            _ => (404, json!("<html>Not found</html>")),
        })
        .await;
        let http = Http::new().unwrap();
        let login = sign_in(&http, SourceKind::Emby, &base, " ann ", "right", "device-1").await.unwrap();
        assert_eq!(login.base_url, format!("{base}/emby"));
        assert_eq!((login.token.as_deref(), login.user_id.as_deref()), (Some(TOKEN), Some("u7")));
        assert_eq!(login.server_name.as_deref(), Some("Den"));
        let sent = requests(&log);
        let auth = sent.iter().find(|request| request.path.ends_with("AuthenticateByName")).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&auth.body).unwrap(), json!({"Username": "ann", "Pw": "right"}));
        assert!(auth.headers["authorization"].contains(r#"DeviceId="device-1""#));
        assert!(auth.headers["x-emby-authorization"].starts_with("MediaBrowser Client=\"Xorvio\""));
        assert!(sent.iter().all(|request| request.query.is_empty()), "nothing secret goes in an address");

        let wrong = sign_in(&http, SourceKind::Emby, &base, "ann", "wrong", "device-1").await.unwrap_err();
        assert_eq!(wrong, SourceError::AuthenticationFailed);
        let unnamed = sign_in(&http, SourceKind::Emby, &base, " ", "right", "device-1").await.unwrap_err();
        assert_eq!(unnamed, SourceError::MissingConfiguration("a username".into()));
        let mismatch = sign_in(&http, SourceKind::Jellyfin, &format!("{base}/emby"), "ann", "right", "d").await.unwrap_err();
        assert_eq!(mismatch.to_string(), "This is an Emby server, not Jellyfin. Add it as an Emby source instead.");

        // Something else entirely, and nothing at all.
        let (other, _) = serve(|_| (200, json!({"status": "ok"}))).await;
        let error = sign_in(&http, SourceKind::Jellyfin, &other, "ann", "right", "d").await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "That address answered, but not as a Jellyfin server. Check the address and port (usually 8096)."
        );
        let closed = sign_in(&http, SourceKind::Jellyfin, "http://127.0.0.1:9", "ann", "right", "d").await.unwrap_err();
        assert!(matches!(closed, SourceError::Network(_)), "{closed:?}");
    }

    #[tokio::test]
    async fn catalog_pages_and_a_failed_library_keeps_its_titles() {
        let (base, log) = serve(|request| match (request.path.as_str(), request.query.get("ParentId").map(String::as_str)) {
            ("/UserViews", _) => (200, json!({"Items": [
                {"Id": "films", "Name": "Films", "CollectionType": "movies", "Type": "CollectionFolder"},
                {"Id": "tv", "Name": "TV", "CollectionType": "tvshows", "Type": "CollectionFolder"}
            ]})),
            ("/Items", Some("films")) => {
                let start: usize = request.query["StartIndex"].parse().unwrap();
                let limit: usize = request.query["Limit"].parse().unwrap();
                let items: Vec<Value> = (start..(start + limit).min(650))
                    .map(|index| json!({"Id": format!("m{index}"), "Name": format!("Film {index}")}))
                    .collect();
                (200, json!({"Items": items, "TotalRecordCount": 650, "StartIndex": start}))
            }
            ("/Items", Some("tv")) => (500, json!("busy")),
            _ => (404, Value::Null),
        })
        .await;
        let http = Http::new().unwrap();
        let steps = Mutex::new(Vec::new());
        let report = |step: &str| steps.lock().unwrap().push(step.to_string());
        let snapshot = fetch_catalog(&http, &session(SourceKind::Jellyfin, &base), "src", &report).await.unwrap();
        assert_eq!(snapshot.movies.len(), 650);
        assert_eq!(snapshot.movies.iter().map(|movie| movie.id.as_str()).collect::<HashSet<_>>().len(), 650);
        assert_eq!(*steps.lock().unwrap(), [phase::READING_CATALOG]);
        assert!(snapshot.kept_kinds.contains(&KeptKind::Series) && !snapshot.kept_kinds.contains(&KeptKind::Movies));
        assert!(snapshot.capabilities.series, "the series kept from before still count");
        assert_eq!(snapshot.warnings, [
            "The library “TV” couldn't be read (The server responded with HTTP 500), so its titles from the last refresh are still shown."
        ]);

        let sent = requests(&log);
        let mut starts: Vec<String> = sent
            .iter()
            .filter(|request| request.query.get("ParentId").map(String::as_str) == Some("films"))
            .map(|request| request.query["StartIndex"].clone())
            .collect();
        starts.sort();
        assert_eq!(starts, ["0", "300", "600"]);
        for request in &sent {
            assert_eq!(request.query.get("UserId").map(String::as_str), Some("u1"));
            assert!(!request.target().contains(TOKEN), "the token stays out of addresses: {}", request.target());
            assert!(request.headers["authorization"].contains(TOKEN));
        }
        let films = sent.iter().find(|request| request.query.get("ParentId").map(String::as_str) == Some("films")).unwrap();
        assert_eq!((films.query["IncludeItemTypes"].as_str(), films.query["Recursive"].as_str()), ("Movie", "true"));
        assert!(films.query["Fields"].contains("MediaSources") && films.query["Fields"].contains("UserDataLastPlayedDate"));

        // When nothing at all can be read, the refresh fails and the last catalog stays.
        let (base, _) = serve(|request| match request.path.as_str() {
            "/UserViews" => (200, json!({"Items": [{"Id": "tv", "Name": "TV", "CollectionType": "tvshows"}]})),
            _ => (500, Value::Null),
        })
        .await;
        let error = fetch_catalog(&http, &session(SourceKind::Jellyfin, &base), "src", &|_| {}).await.unwrap_err();
        assert_eq!(error, SourceError::Http(500));
        // A token the server no longer accepts.
        let (base, _) = serve(|_| (401, Value::Null)).await;
        let error = fetch_catalog(&http, &session(SourceKind::Emby, &base), "src", &|_| {}).await.unwrap_err();
        assert!(error.is_authentication());
        let signed_out = ServerSession { user_id: None, ..session(SourceKind::Emby, &base) };
        assert!(matches!(fetch_catalog(&http, &signed_out, "src", &|_| {}).await, Err(SourceError::MissingConfiguration(_))));
    }

    #[tokio::test]
    async fn older_jellyfin_routes_are_used_when_the_new_ones_are_missing() {
        // Jellyfin 10.8: only the per-user routes, and no total in item lists.
        let (base, log) = serve(|request| match request.path.as_str() {
            "/Users/u1/Views" => (200, json!({"Items": [{"Id": "f", "Name": "Films", "CollectionType": "movies"}]})),
            "/Users/u1/Items" => (200, json!({"Items": [{"Id": "m1", "Name": "Only Film"}]})),
            "/Users/u1/Items/m1" => (200, json!({"Id": "m1", "Name": "Only Film", "Taglines": ["Old but fine."]})),
            _ => (404, Value::Null),
        })
        .await;
        let http = Http::new().unwrap();
        let session = session(SourceKind::Jellyfin, &base);
        let snapshot = fetch_catalog(&http, &session, "src", &|_| {}).await.unwrap();
        assert_eq!(snapshot.movies[0].title, "Only Film");
        let details = fetch_details(&http, &session, "src", MediaKind::Movie, "m1").await.unwrap();
        assert_eq!(details.tagline.as_deref(), Some("Old but fine."));
        // Once the older routes are found, the rest of the refresh goes straight to them.
        let paths: Vec<String> = requests(&log).iter().map(|request| request.path.clone()).collect();
        assert_eq!(paths, ["/UserViews", "/Users/u1/Views", "/Users/u1/Items", "/Items/m1", "/Users/u1/Items/m1"]);
    }

    #[tokio::test]
    async fn series_details_with_seasons() {
        let (base, log) = serve(|request| match request.path.as_str() {
            "/Users/u1/Items/21" => (200, fixture("emby-series-details")),
            "/Shows/21/Seasons" => (200, fixture("emby-seasons")),
            _ => (404, Value::Null),
        })
        .await;
        let http = Http::new().unwrap();
        let details = fetch_details(&http, &session(SourceKind::Emby, &base), "src", MediaKind::Series, "21").await.unwrap();
        assert_eq!(details.seasons.len(), 2);
        assert_eq!(details.network.as_deref(), Some("Northwind Broadcasting"));
        let item = requests(&log).into_iter().find(|request| request.path == "/Users/u1/Items/21").unwrap();
        assert!(item.query["Fields"].contains("People"));
    }

    #[tokio::test]
    async fn watching_reaches_the_server() {
        for kind in [SourceKind::Jellyfin, SourceKind::Emby] {
            let (base, log) = serve(|request| match request.method.as_str() {
                "POST" | "DELETE" => (200, json!({})),
                _ => (404, Value::Null),
            })
            .await;
            let http = Http::new().unwrap();
            let session = session(kind, &base);
            report_progress(&http, &session, "i9", 754.25, 6000.0, false).await.unwrap();
            report_progress(&http, &session, "i9", 5900.0, 6000.0, true).await.unwrap();
            set_played(&http, &session, "i9", false).await.unwrap();
            let sent = requests(&log);
            let targets: Vec<String> = sent.iter().map(|request| format!("{} {}", request.method, request.path)).collect();
            let expected = match kind {
                SourceKind::Jellyfin => ["POST /UserItems/i9/UserData", "POST /UserPlayedItems/i9", "DELETE /UserPlayedItems/i9"],
                _ => ["POST /Users/u1/Items/i9/UserData", "POST /Users/u1/PlayedItems/i9", "DELETE /Users/u1/PlayedItems/i9"],
            };
            assert_eq!(targets, expected, "{kind:?}");
            let body: Value = serde_json::from_str(&sent[0].body).unwrap();
            assert_eq!(body["PlaybackPositionTicks"], 7_542_500_000_i64);
            assert!(body.get("Played").is_none(), "a resumed title keeps its watched mark");
            let played_at = timestamp(body.get("LastPlayedDate")).unwrap();
            assert!((crate::now() - played_at).abs() < 60);
            assert!(sent.iter().all(|request| request.query.get("UserId").map(String::as_str) == Some("u1")));
        }
    }
}

