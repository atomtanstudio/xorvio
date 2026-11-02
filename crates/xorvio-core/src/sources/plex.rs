//! Plex: signing in through plex.tv, finding the account's servers, and each
//! server's API.
//!
//! Every request says who's asking in Plex's client headers (Xorvio, its
//! version and platform, and this copy's device id) and carries the token in
//! the `X-Plex-Token` header, never in an address. Titles keep the server's
//! rating keys and file paths; artwork keeps the server's own paths, resized
//! by its photo transcoder, behind [`artwork_url`]; files are played as they
//! are, without Plex's transcoder, since Xorvio's gateway converts what it must.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

use serde_json::Value;
use xorvio_model::{
    CastMember, Category, CategoryKind, Episode, MediaKind, Movie, SeasonInfo, Series, StreamLocator, StreamTarget,
    VideoFormat,
};

use super::server::{ServerLogin, ServerProgress, ServerSession, artwork_url, is_server_path, normalize_address};
use super::{ids, phase, xtream};
use crate::details::DetailsRecord;
use crate::error::SourceError;
use crate::http::{ApiCall, Http};
use crate::json::{self, Object};
use crate::names;
use crate::snapshot::{CatalogSnapshot, KeptKind};

/// plex.tv, where people sign in and their servers are listed.
pub(crate) const PLEX_TV: &str = "https://plex.tv";
/// The page that approves a sign-in, opened in the person's browser.
const AUTH_PAGE: &str = "https://app.plex.tv/auth";
const PRODUCT: &str = "Xorvio";
/// Plex's name for its media library, which watched and progress requests give.
const LIBRARY: &str = "com.plexapp.plugins.library";
/// Titles asked for at a time. Big libraries take several pages, each well
/// within the time a slow server has to answer.
const PAGE_SIZE: usize = 500;
/// More pages than any library has, so a server that keeps answering can't
/// keep a refresh going forever.
const MAX_PAGES: usize = 2000;
/// How long each of a server's addresses has to answer before the next kind
/// (remote, then relay) is tried.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(4);
/// The most people kept for a title's cast.
const MAX_PEOPLE: usize = 30;

// MARK: - Requests

/// Who's asking, as Plex wants every request to say: the product, its
/// version and platform, and this copy's id, which Plex lists among the
/// account's devices and which a sign-in PIN is tied to.
fn client_headers(device_id: &str, token: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![
        ("X-Plex-Product".to_string(), PRODUCT.to_string()),
        ("X-Plex-Version".to_string(), env!("CARGO_PKG_VERSION").to_string()),
        ("X-Plex-Client-Identifier".to_string(), device_id.to_string()),
        ("X-Plex-Platform".to_string(), platform().to_string()),
        ("X-Plex-Device-Name".to_string(), PRODUCT.to_string()),
    ];
    if let Some(token) = token.map(str::trim).filter(|token| !token.is_empty()) {
        headers.push(("X-Plex-Token".to_string(), token.to_string()));
    }
    headers
}

/// [`client_headers`] for an API call, which asks for JSON (Plex answers in
/// XML otherwise).
fn api_headers(device_id: &str, token: Option<&str>) -> Vec<(String, String)> {
    let mut headers = client_headers(device_id, token);
    headers.push(("Accept".to_string(), "application/json".to_string()));
    headers
}

fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    }
}

/// The headers that let a server's artwork and files be fetched: the client
/// headers with the token.
pub(crate) fn auth_headers(session: &ServerSession) -> Vec<(String, String)> {
    client_headers(&session.device_id, session.token.as_deref())
}

/// `base` with path segments and a query added, each part escaped. `base`
/// may carry a path of its own (a server behind a proxy at "/plex").
fn link(base: &str, segments: &[&str], query: &[(&str, &str)]) -> String {
    let Ok(mut link) = url::Url::parse(base) else {
        return format!("{}/{}", base.trim_end_matches('/'), segments.join("/"));
    };
    if let Ok(mut path) = link.path_segments_mut() {
        path.pop_if_empty().extend(segments);
    }
    if !query.is_empty() {
        link.query_pairs_mut().extend_pairs(query);
    }
    link.to_string()
}

/// The `MediaContainer` every Plex reply is wrapped in.
fn container(reply: &Value) -> Option<&Object> {
    reply.get("MediaContainer")?.as_object()
}

/// A plain message about signing in. `AuthenticationFailed` speaks of a
/// username and password, which Plex sources don't have.
fn sign_in_problem(message: &str) -> SourceError {
    SourceError::Network(message.into())
}

fn not_plex() -> SourceError {
    SourceError::Network("That address doesn't answer like a Plex server. Check the address and port (Plex uses 32400 unless it's been changed).".into())
}

fn unexpected_reply() -> SourceError {
    SourceError::Network("plex.tv sent a reply Xorvio couldn't read. Try again in a moment.".into())
}

// MARK: - Signing in through plex.tv

/// A sign-in waiting for the person to approve it on plex.tv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlexPin {
    pub id: i64,
    pub code: String,
    /// The plex.tv page to open in the browser, where the person signs in
    /// and approves Xorvio.
    pub auth_url: String,
}

/// A server the signed-in account can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlexServer {
    /// The machine identifier.
    pub id: String,
    pub name: String,
    /// The account's own server, rather than one shared with it.
    pub owned: bool,
    /// The token for this server (a shared server has its own).
    pub access_token: String,
    pub connections: Vec<PlexConnection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlexConnection {
    pub uri: String,
    /// On the same network as the server.
    pub local: bool,
    /// Through Plex's relay, which is slow and limited; tried last.
    pub relay: bool,
}

/// Starts a sign-in: asks `plex_tv` for a PIN and makes the page to approve
/// it on. A strong PIN is one the person never types; the page carries it.
pub(crate) async fn start_sign_in(http: &Http, plex_tv: &str, device_id: &str) -> Result<PlexPin, SourceError> {
    let headers = api_headers(device_id, None);
    let url = link(plex_tv, &["api", "v2", "pins"], &[("strong", "true")]);
    let reply = http.api_json(ApiCall::post(&url, &headers, None)).await?;
    let id = json::int(reply.get("id")).ok_or_else(unexpected_reply)?;
    let code = json::string(reply.get("code")).filter(|code| !code.trim().is_empty()).ok_or_else(unexpected_reply)?;
    Ok(PlexPin { id, auth_url: auth_url(device_id, &code), code })
}

/// The plex.tv page that approves `code` for this copy of Xorvio. Plex reads
/// the part after "#?" as a query, so each value is escaped like one.
fn auth_url(device_id: &str, code: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("clientID", device_id)
        .append_pair("code", code)
        .append_pair("context[device][product]", PRODUCT)
        .finish();
    format!("{AUTH_PAGE}#?{query}")
}

/// The account token once the person has approved the PIN, `None` while
/// they haven't yet. An expired PIN is an error.
pub(crate) async fn check_sign_in(http: &Http, plex_tv: &str, device_id: &str, pin_id: i64) -> Result<Option<String>, SourceError> {
    let headers = api_headers(device_id, None);
    let url = link(plex_tv, &["api", "v2", "pins", &pin_id.to_string()], &[]);
    let expired = || sign_in_problem("This Plex sign-in has expired. Start signing in again.");
    let reply = match http.api_json(ApiCall::get(&url, &headers)).await {
        Ok(reply) => reply,
        Err(SourceError::Http(404)) => return Err(expired()),
        Err(error) => return Err(error),
    };
    if let Some(token) = json::string(reply.get("authToken")).filter(|token| !token.trim().is_empty()) {
        return Ok(Some(token));
    }
    if json::int(reply.get("expiresIn")).is_some_and(|seconds| seconds <= 0) {
        return Err(expired());
    }
    Ok(None)
}

/// The servers the account can use (owned and shared), from plex.tv.
pub(crate) async fn servers(http: &Http, plex_tv: &str, device_id: &str, account_token: &str) -> Result<Vec<PlexServer>, SourceError> {
    let headers = api_headers(device_id, Some(account_token));
    let url = link(plex_tv, &["api", "v2", "resources"], &[("includeHttps", "1"), ("includeRelay", "1")]);
    let reply = http.api_json(ApiCall::get(&url, &headers)).await.map_err(|error| match error {
        SourceError::AuthenticationFailed => sign_in_problem("plex.tv didn't accept the sign-in. Sign in with Plex again."),
        other => other,
    })?;
    Ok(parse_servers(&reply, account_token))
}

/// The servers in a `resources` reply, the account's own first. Players and
/// other devices on the account are left out.
fn parse_servers(reply: &Value, account_token: &str) -> Vec<PlexServer> {
    let mut servers: Vec<PlexServer> = json::objects(Some(reply))
        .into_iter()
        .filter(|device| {
            json::string(device.get("provides")).is_some_and(|provides| provides.split(',').any(|role| role.trim() == "server"))
        })
        .filter_map(|device| {
            let id = text(device.get("clientIdentifier"))?;
            let connections = json::objects(device.get("connections"))
                .into_iter()
                .filter_map(|connection| {
                    Some(PlexConnection {
                        uri: text(connection.get("uri"))?,
                        local: flag(connection.get("local")),
                        relay: flag(connection.get("relay")),
                    })
                })
                .collect();
            let owned = flag(device.get("owned"));
            // An owned server's token is the account's own when plex.tv leaves it
            // out. A shared server's owner never gets the account's token, so one
            // without a token of its own can't be used.
            let access_token = match text(device.get("accessToken")) {
                Some(token) => token,
                None if owned => account_token.to_string(),
                None => return None,
            };
            Some(PlexServer { name: text(device.get("name")).unwrap_or_else(|| "Plex".into()), owned, access_token, connections, id })
        })
        .collect();
    servers.sort_by_key(|server| !server.owned);
    servers
}

/// The first of a server's connections that answers as that server: local
/// ones first, then remote, then relay. The connections of each kind are
/// tried together, each with a short timeout, so an address that's out of
/// reach (a home address from elsewhere) costs seconds, not minutes.
pub(crate) async fn best_connection(http: &Http, device_id: &str, server: &PlexServer) -> Result<String, SourceError> {
    let headers = api_headers(device_id, Some(&server.access_token));
    for tier in connection_tiers(&server.connections) {
        let checks = tier.iter().map(|connection| {
            let headers = &headers;
            async move {
                let base = normalize_address(&connection.uri)?;
                let answered = tokio::time::timeout(CONNECTION_TIMEOUT, identity(http, &base, headers)).await;
                matches!(answered, Ok(Ok(machine_id)) if machine_id == server.id).then_some(base)
            }
        });
        if let Some(base) = futures_util::future::join_all(checks).await.into_iter().flatten().next() {
            return Ok(base);
        }
    }
    Err(SourceError::Network(format!(
        "None of {}'s addresses answered. Check that the server is on and connected, then try again.",
        server.name
    )))
}

/// Connections in the order they're tried: local, remote, then relay.
fn connection_tiers(connections: &[PlexConnection]) -> Vec<Vec<&PlexConnection>> {
    let local = connections.iter().filter(|c| c.local && !c.relay).collect();
    let remote = connections.iter().filter(|c| !c.local && !c.relay).collect();
    let relay = connections.iter().filter(|c| c.relay).collect();
    [local, remote, relay].into_iter().filter(|tier: &Vec<&PlexConnection>| !tier.is_empty()).collect()
}

// MARK: - A server

/// The machine identifier of the Plex server at `base_url`. `/identity`
/// answers without a token, so this tells "not Plex" apart from "Plex, but
/// it wants a sign-in".
async fn identity(http: &Http, base_url: &str, headers: &[(String, String)]) -> Result<String, SourceError> {
    let reply = http.api_json(ApiCall::get(&link(base_url, &["identity"], &[]), headers)).await.map_err(|error| match error {
        SourceError::Http(404) | SourceError::InvalidPlaylist(_) => not_plex(),
        other => other,
    })?;
    container(&reply).and_then(|found| text(found.get("machineIdentifier"))).ok_or_else(not_plex)
}

/// Checks a server at an address (with a token when it needs one) and
/// returns its name and machine identifier ([`ServerLogin::user_id`]).
pub(crate) async fn check_server(http: &Http, base_url: &str, token: Option<&str>, device_id: &str) -> Result<ServerLogin, SourceError> {
    let token = token.map(str::trim).filter(|token| !token.is_empty());
    let headers = api_headers(device_id, token);
    let machine_id = identity(http, base_url, &headers).await?;
    let root = http.api_json(ApiCall::get(&link(base_url, &[], &[]), &headers)).await.map_err(|error| match error {
        SourceError::AuthenticationFailed if token.is_none() => {
            sign_in_problem("This Plex server only lets people in who sign in. Choose Sign in with Plex to add it.")
        }
        SourceError::AuthenticationFailed => {
            sign_in_problem("This Plex server didn't accept the sign-in. Choose Sign in with Plex to add it again.")
        }
        other => other,
    })?;
    Ok(ServerLogin {
        base_url: base_url.to_string(),
        token: token.map(str::to_string),
        user_id: Some(machine_id),
        server_name: container(&root).and_then(|server| text(server.get("friendlyName"))),
    })
}

/// One library as it was read: its films, or its shows and their episodes,
/// or why it couldn't be read.
#[derive(Debug)]
pub(crate) struct LibraryRead {
    pub key: String,
    pub title: String,
    pub kind: CategoryKind,
    pub items: Result<LibraryItems, SourceError>,
}

#[derive(Debug, Default)]
pub(crate) struct LibraryItems {
    /// Films, or shows.
    pub titles: Vec<Object>,
    /// Every episode of the shows.
    pub episodes: Vec<Object>,
}

/// The server's whole library: film and show libraries as categories, every
/// film, show and episode, the details the lists carry, and what's been
/// watched. Episodes are read now rather than as a show is opened, so
/// Continue Watching and the next episode can see them.
///
/// A library that can't be read keeps what it had before (with a note
/// saying so) while the others are refreshed; when none can be, the refresh
/// fails and the previous catalog stays.
pub(crate) async fn fetch_catalog(
    http: &Http,
    session: &ServerSession,
    source_id: &str,
    report: &(dyn Fn(&str) + Send + Sync),
) -> Result<CatalogSnapshot, SourceError> {
    let headers = api_headers(&session.device_id, session.token.as_deref());
    let machine_id = identity(http, &session.base_url, &headers).await?;
    if let Some(expected) = session.user_id.as_deref().filter(|id| !id.is_empty())
        && expected != machine_id
    {
        return Err(SourceError::Network(
            "A different Plex server answers at this address now. Edit the source to give the server's new address.".into(),
        ));
    }
    let sections = http.api_json(ApiCall::get(&link(&session.base_url, &["library", "sections"], &[]), &headers)).await?;
    report(phase::READING_CATALOG);
    let mut libraries = Vec::new();
    for section in container(&sections).map(|found| json::objects(found.get("Directory"))).unwrap_or_default() {
        let kind = match json::string(section.get("type")).as_deref() {
            Some("movie") => CategoryKind::Movie,
            Some("show") => CategoryKind::Series,
            _ => continue,
        };
        let Some(key) = text(section.get("key")) else { continue };
        let title = text(section.get("title")).unwrap_or_else(|| key.clone());
        let items = read_library(http, session, &headers, &key, kind).await;
        if let Err(error) = &items {
            tracing::warn!("the Plex library {title} couldn't be read: {error}");
        }
        libraries.push(LibraryRead { key, title, kind, items });
    }
    if let Some(LibraryRead { items: Err(error), .. }) = libraries.first()
        && libraries.iter().all(|library| library.items.is_err())
    {
        return Err(error.clone());
    }
    Ok(build(source_id, &libraries))
}

async fn read_library(
    http: &Http,
    session: &ServerSession,
    headers: &[(String, String)],
    key: &str,
    kind: CategoryKind,
) -> Result<LibraryItems, SourceError> {
    Ok(match kind {
        CategoryKind::Movie => LibraryItems { titles: all_items(http, session, headers, key, "1").await?, episodes: Vec::new() },
        _ => LibraryItems {
            titles: all_items(http, session, headers, key, "2").await?,
            episodes: all_items(http, session, headers, key, "4").await?,
        },
    })
}

/// Every item of one type in a library (1 films, 2 shows, 4 episodes), a
/// page at a time. A server that doesn't page (it says no total) sends
/// everything at once. Items are kept once each, in case the library
/// changes between pages.
async fn all_items(
    http: &Http,
    session: &ServerSession,
    headers: &[(String, String)],
    section: &str,
    item_type: &str,
) -> Result<Vec<Object>, SourceError> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let size = PAGE_SIZE.to_string();
    for page in 0..MAX_PAGES {
        let start = (page * PAGE_SIZE).to_string();
        let mut query = vec![("type", item_type), ("X-Plex-Container-Start", start.as_str()), ("X-Plex-Container-Size", size.as_str())];
        // Films and shows carry their TMDB and IMDb ids; episodes don't need them.
        if item_type != "4" {
            query.push(("includeGuids", "1"));
        }
        let url = link(&session.base_url, &["library", "sections", section, "all"], &query);
        let reply = http.api_json(ApiCall::get(&url, headers)).await?;
        let Some(found) = container(&reply) else { break };
        let listed = json::objects(found.get("Metadata"));
        let count = listed.len();
        for item in listed {
            if text(item.get("ratingKey")).is_some_and(|key| seen.insert(key)) {
                items.push(item.clone());
            }
        }
        let more = json::int(found.get("totalSize")).is_some_and(|total| count > 0 && ((page * PAGE_SIZE + count) as i64) < total);
        if !more {
            break;
        }
    }
    Ok(items)
}

/// The catalog the libraries make.
pub(crate) fn build(source_id: &str, libraries: &[LibraryRead]) -> CatalogSnapshot {
    let mut snapshot = CatalogSnapshot::default();
    let mut kept = BTreeSet::new();
    for library in libraries {
        let category_id = ids::category(source_id, library.kind, &library.key);
        let sort_index = snapshot.categories.len() as i64;
        snapshot.categories.push(Category {
            id: category_id.clone(),
            source_id: source_id.to_string(),
            kind: library.kind,
            name: library.title.clone(),
            sort_index,
        });
        let items = match &library.items {
            Ok(items) => items,
            Err(error) => {
                kept.insert(if library.kind == CategoryKind::Movie { KeptKind::Movies } else { KeptKind::Series });
                snapshot.warnings.push(format!(
                    "The {} library couldn't be read ({}), so the titles it had before are still shown.",
                    library.title,
                    error.to_string().trim_end_matches('.')
                ));
                continue;
            }
        };
        match library.kind {
            CategoryKind::Movie => add_films(&mut snapshot, source_id, &category_id, &items.titles),
            CategoryKind::Series => add_shows(&mut snapshot, source_id, &category_id, items),
            CategoryKind::Live => {}
        }
    }
    snapshot.episodes.sort_by(|a, b| (&a.series_id, a.season, a.number).cmp(&(&b.series_id, b.season, b.number)));
    snapshot.derive_capabilities();
    for kind in kept {
        match kind {
            KeptKind::Movies => snapshot.capabilities.movies = true,
            KeptKind::Series => snapshot.capabilities.series = true,
        }
        snapshot.kept_kinds.insert(kind);
    }
    snapshot
}

fn add_films(snapshot: &mut CatalogSnapshot, source_id: &str, category_id: &str, films: &[Object]) {
    for raw in films {
        let Some(key) = text(raw.get("ratingKey")) else { continue };
        let Some((title, year)) = title_and_year(raw) else { continue };
        let id = ids::movie(source_id, &key);
        let (tmdb_id, imdb_id) = external_ids(raw);
        let details = list_details(source_id, raw, MediaKind::Movie);
        if !details.is_empty() {
            snapshot.details.insert(id.clone(), details);
        }
        let duration_seconds = seconds(raw.get("duration"));
        snapshot.progress.extend(progress(&id, MediaKind::Movie, None, raw, duration_seconds));
        snapshot.movies.push(Movie {
            id,
            source_id: source_id.to_string(),
            title,
            year,
            poster_url: artwork(source_id, raw.get("thumb"), Art::Poster),
            backdrop_url: artwork(source_id, raw.get("art"), Art::Backdrop),
            category_id: Some(category_id.to_string()),
            rating: rating(raw),
            added_at: json::int(raw.get("addedAt")).filter(|added| *added > 0),
            plot: text(raw.get("summary")),
            genre: joined(tags(raw, "Genre")),
            tmdb_id,
            imdb_id,
            duration_seconds,
            stream: locator(&key, raw),
        });
    }
}

fn add_shows(snapshot: &mut CatalogSnapshot, source_id: &str, category_id: &str, items: &LibraryItems) {
    let mut shows: HashMap<String, String> = HashMap::new();
    for raw in &items.titles {
        let Some(key) = text(raw.get("ratingKey")) else { continue };
        let Some((title, year)) = title_and_year(raw) else { continue };
        let id = ids::series(source_id, &key);
        let (tmdb_id, imdb_id) = external_ids(raw);
        let details = list_details(source_id, raw, MediaKind::Series);
        if !details.is_empty() {
            snapshot.details.insert(id.clone(), details);
        }
        shows.insert(key.clone(), id.clone());
        snapshot.series.push(Series {
            id,
            source_id: source_id.to_string(),
            title,
            year,
            poster_url: artwork(source_id, raw.get("thumb"), Art::Poster),
            backdrop_url: artwork(source_id, raw.get("art"), Art::Backdrop),
            category_id: Some(category_id.to_string()),
            rating: rating(raw),
            added_at: json::int(raw.get("addedAt")).filter(|added| *added > 0),
            plot: text(raw.get("summary")),
            genre: joined(tags(raw, "Genre")),
            tmdb_id,
            imdb_id,
            remote_id: Some(key),
        });
    }
    let mut numbered: HashMap<(String, i64), i64> = HashMap::new();
    for raw in &items.episodes {
        let Some(key) = text(raw.get("ratingKey")) else { continue };
        let Some(series_id) = text(raw.get("grandparentRatingKey")).and_then(|show| shows.get(&show)) else { continue };
        let season = json::int(raw.get("parentIndex")).unwrap_or(1);
        // Episodes Plex hasn't numbered follow the season's others.
        let counted = numbered.entry((series_id.clone(), season)).or_default();
        *counted += 1;
        let number = json::int(raw.get("index")).unwrap_or(*counted);
        let id = ids::episode(series_id, &key);
        let duration_seconds = seconds(raw.get("duration"));
        snapshot.progress.extend(progress(&id, MediaKind::Episode, Some(series_id), raw, duration_seconds));
        snapshot.episodes.push(Episode {
            id,
            series_id: series_id.clone(),
            source_id: source_id.to_string(),
            season,
            number,
            title: text(raw.get("title")).unwrap_or_else(|| format!("Episode {number}")),
            plot: text(raw.get("summary")),
            duration_seconds,
            still_url: artwork(source_id, raw.get("thumb"), Art::Still),
            stream: locator(&key, raw),
            air_date: release_date(raw).and_then(|day| xtream::day_timestamp(&day)),
        });
    }
}

/// What the server remembers of a film or episode: where it was left (a
/// title watched before and started again is in progress), or that it was
/// watched. Nothing when it was never played.
fn progress(item_id: &str, kind: MediaKind, series_id: Option<&String>, raw: &Object, duration: Option<i64>) -> Option<ServerProgress> {
    let offset = json::int(raw.get("viewOffset")).filter(|offset| *offset > 0);
    let watched = json::int(raw.get("viewCount")).is_some_and(|count| count > 0);
    if offset.is_none() && !watched {
        return None;
    }
    let duration_seconds = duration.unwrap_or(0) as f64;
    Some(ServerProgress {
        item_id: item_id.to_string(),
        kind,
        series_id: series_id.cloned(),
        position_seconds: offset.map_or(duration_seconds, |offset| offset as f64 / 1000.0),
        duration_seconds,
        finished: offset.is_none(),
        updated_at: json::int(raw.get("lastViewedAt")).filter(|at| *at > 0),
    })
}

/// The file to play: the first version that isn't one Plex optimized (a
/// lower-quality copy it made for slow connections), and its first part.
fn locator(key: &str, raw: &Object) -> StreamLocator {
    let found = playable(raw);
    StreamLocator::Server {
        key: key.to_string(),
        part: found.and_then(|(_, part)| text(part.get("key"))),
        extension: found
            .and_then(|(media, part)| text(part.get("container")).or_else(|| text(media.get("container"))))
            .map(|container| container.to_lowercase()),
    }
}

fn playable(raw: &Object) -> Option<(&Object, &Object)> {
    let media = json::objects(raw.get("Media"));
    let (originals, optimized): (Vec<&Object>, Vec<&Object>) = media.into_iter().partition(|media| media.get("proxyType").is_none());
    originals.into_iter().chain(optimized).find_map(|media| {
        let part = json::objects(media.get("Part"))
            .into_iter()
            .find(|part| text(part.get("key")).is_some_and(|key| is_server_path(&key)))?;
        Some((media, part))
    })
}

// MARK: - Details

/// Fuller details for a film or show than its list carries: cast with
/// characters and photos, writers, the video and sound of its file, its
/// logo, and for a show, its seasons.
pub(crate) async fn fetch_details(
    http: &Http,
    session: &ServerSession,
    source_id: &str,
    kind: MediaKind,
    key: &str,
) -> Result<DetailsRecord, SourceError> {
    let headers = api_headers(&session.device_id, session.token.as_deref());
    let item_url = link(&session.base_url, &["library", "metadata", key], &[("includeGuids", "1")]);
    let seasons_url = link(&session.base_url, &["library", "metadata", key, "children"], &[]);
    let (item, seasons) = if kind == MediaKind::Series {
        let (item, seasons) =
            tokio::join!(http.api_json(ApiCall::get(&item_url, &headers)), http.api_json(ApiCall::get(&seasons_url, &headers)));
        (item?, seasons.inspect_err(|error| tracing::debug!("a Plex show's seasons couldn't be read: {error}")).ok())
    } else {
        (http.api_json(ApiCall::get(&item_url, &headers)).await?, None)
    };
    let raw = container(&item)
        .and_then(|found| json::objects(found.get("Metadata")).into_iter().next())
        .ok_or_else(|| SourceError::Network("The Plex server no longer has this title.".into()))?;
    let mut details = full_details(source_id, raw, kind);
    if let Some(seasons) = seasons {
        details.seasons = season_list(source_id, &seasons);
    }
    Ok(details)
}

/// Details a film's or show's list entry carries, of those it carries in
/// full. Lists name only the first two or three genres, actors, directors,
/// writers and countries, and a refresh writes list details over what's
/// stored, so those come only with the title's own details, as do cast
/// photos and whether the video is HDR.
fn list_details(source_id: &str, raw: &Object, kind: MediaKind) -> DetailsRecord {
    let title = text(raw.get("title"));
    DetailsRecord {
        backdrop_urls: artwork(source_id, raw.get("art"), Art::Backdrop).into_iter().collect(),
        logo_url: logo(source_id, raw),
        release_date: release_date(raw),
        age_rating: age_rating(raw.get("contentRating")),
        runtime_minutes: json::int(raw.get("duration")).filter(|ms| *ms > 0).map(|ms| (ms + 30_000) / 60_000),
        imdb_id: external_ids(raw).1,
        original_title: text(raw.get("originalTitle")).filter(|original| Some(original) != title.as_ref()),
        tagline: text(raw.get("tagline")),
        network: if kind == MediaKind::Series { text(raw.get("studio")) } else { None },
        audio_channels: playable(raw).and_then(|(media, _)| json::int(media.get("audioChannels"))).filter(|channels| *channels > 0),
        ..Default::default()
    }
}

/// Everything a title's own details give.
fn full_details(source_id: &str, raw: &Object, kind: MediaKind) -> DetailsRecord {
    let people: Vec<CastMember> = json::objects(raw.get("Role"))
        .into_iter()
        .filter_map(|role| {
            Some(CastMember {
                name: text(role.get("tag"))?,
                role: text(role.get("role")),
                photo_url: artwork(source_id, role.get("thumb"), Art::Photo),
                tmdb_id: None,
            })
        })
        .take(MAX_PEOPLE)
        .collect();
    DetailsRecord {
        cast: people.iter().map(|person| person.name.clone()).collect(),
        people,
        director: joined(tags(raw, "Director")),
        creators: tags(raw, "Writer"),
        country: joined(tags(raw, "Country")),
        genres: tags(raw, "Genre"),
        overview: text(raw.get("summary")),
        video: playable(raw).and_then(|(media, part)| video_format(media, part)),
        ..list_details(source_id, raw, kind)
    }
}

/// The video of a title's file. HDR shows in its video stream: the transfer
/// HDR10 and HLG use, or Dolby Vision.
fn video_format(media: &Object, part: &Object) -> Option<VideoFormat> {
    let width = json::int(media.get("width")).filter(|width| *width > 0)?;
    let height = json::int(media.get("height")).filter(|height| *height > 0)?;
    let video = json::objects(part.get("Stream")).into_iter().find(|stream| json::int(stream.get("streamType")) == Some(1));
    let is_hdr = video.is_some_and(|video| {
        let transfer = json::string(video.get("colorTrc")).unwrap_or_default().to_lowercase();
        let title = json::string(video.get("displayTitle")).unwrap_or_default();
        transfer == "smpte2084"
            || transfer == "arib-std-b67"
            || flag(video.get("DOVIPresent"))
            || title.contains("HDR")
            || title.contains("Dolby Vision")
    });
    Some(VideoFormat { width, height, codec: text(media.get("videoCodec")), is_hdr })
}

/// A show's seasons from `/children`, in order. Specials are season 0.
fn season_list(source_id: &str, reply: &Value) -> Vec<SeasonInfo> {
    let mut seasons: Vec<SeasonInfo> = container(reply)
        .map(|found| json::objects(found.get("Metadata")))
        .unwrap_or_default()
        .into_iter()
        .filter(|raw| json::string(raw.get("type")).is_none_or(|kind| kind == "season"))
        .filter_map(|raw| {
            Some(SeasonInfo {
                number: json::int(raw.get("index"))?,
                name: text(raw.get("title")),
                overview: text(raw.get("summary")),
                air_date: release_date(raw),
                cover_url: artwork(source_id, raw.get("thumb"), Art::Poster),
                episode_count: json::int(raw.get("leafCount")),
            })
        })
        .collect();
    seasons.sort_by_key(|season| season.number);
    seasons
}

// MARK: - Playing and what's been watched

/// The file itself, with the token in a header. Plex's own transcoder is
/// left alone: Xorvio's gateway reads the file (with ranges, so seeking
/// works) and converts what the player can't take.
pub(crate) fn stream_target(session: &ServerSession, _key: &str, part: Option<&str>) -> Result<StreamTarget, SourceError> {
    let part = part.filter(|part| is_server_path(part)).ok_or_else(|| {
        SourceError::Unsupported("Xorvio doesn't know this title's file yet. Refresh the source, then try again.".into())
    })?;
    Ok(StreamTarget { url: format!("{}{part}", session.base_url), user_agent: None, headers: auth_headers(session), is_live: false })
}

/// Tells the server where a film or episode was left, as a player's
/// "stopped" timeline report: it records the position without leaving a
/// playing session on the server's dashboard. Plex itself ignores positions
/// in a title's first minute.
///
/// A finished title is marked watched instead, once: a player reports
/// every few seconds through the closing credits, and each would otherwise
/// count as another viewing.
pub(crate) async fn report_progress(
    http: &Http,
    session: &ServerSession,
    key: &str,
    position_seconds: f64,
    duration_seconds: f64,
    finished: bool,
) -> Result<(), SourceError> {
    let headers = api_headers(&session.device_id, session.token.as_deref());
    if finished {
        if is_watched(http, session, &headers, key).await? {
            return Ok(());
        }
        return set_played(http, session, key, true).await;
    }
    let metadata = format!("/library/metadata/{key}");
    let time = milliseconds(position_seconds).to_string();
    let duration = milliseconds(duration_seconds).to_string();
    let mut query =
        vec![("ratingKey", key), ("key", metadata.as_str()), ("identifier", LIBRARY), ("state", "stopped"), ("time", time.as_str())];
    if duration_seconds > 0.0 {
        query.push(("duration", duration.as_str()));
    }
    http.api(ApiCall::put(&link(&session.base_url, &[":", "timeline"], &query), &headers)).await?;
    Ok(())
}

/// Watched, and not started again since.
async fn is_watched(http: &Http, session: &ServerSession, headers: &[(String, String)], key: &str) -> Result<bool, SourceError> {
    let reply = http.api_json(ApiCall::get(&link(&session.base_url, &["library", "metadata", key], &[]), headers)).await?;
    let Some(raw) = container(&reply).and_then(|found| json::objects(found.get("Metadata")).into_iter().next()) else {
        return Ok(false);
    };
    let watched = json::int(raw.get("viewCount")).is_some_and(|count| count > 0);
    let started = json::int(raw.get("viewOffset")).is_some_and(|offset| offset > 0);
    Ok(watched && !started)
}

/// Marks a film or episode watched (`/:/scrobble`) or unwatched
/// (`/:/unscrobble`, which also forgets where it was left).
pub(crate) async fn set_played(http: &Http, session: &ServerSession, key: &str, played: bool) -> Result<(), SourceError> {
    let headers = api_headers(&session.device_id, session.token.as_deref());
    let action = if played { "scrobble" } else { "unscrobble" };
    let url = link(&session.base_url, &[":", action], &[("key", key), ("identifier", LIBRARY)]);
    http.api(ApiCall::put(&url, &headers)).await?;
    Ok(())
}

fn milliseconds(seconds: f64) -> i64 {
    json::finite_int((seconds.max(0.0) * 1000.0).round()).unwrap_or(0)
}

// MARK: - Reading Plex's fields

/// The sizes artwork is asked for, by where it's shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Art {
    Poster,
    Backdrop,
    Still,
    Photo,
    /// As it is: logos keep their transparency.
    Original,
}

impl Art {
    fn size(self) -> Option<(u32, u32)> {
        match self {
            Self::Poster => Some((480, 720)),
            Self::Backdrop => Some((1920, 1080)),
            Self::Still => Some((640, 360)),
            Self::Photo => Some((300, 300)),
            Self::Original => None,
        }
    }
}

/// The address the interface shows a Plex image at. The server's own images
/// go through its photo transcoder, sized for where they're shown (an
/// original poster can be several megabytes), behind [`artwork_url`]; images
/// Plex links elsewhere (its metadata host has cast photos) are used as they
/// are. Anything carrying a token is left out.
fn artwork(source_id: &str, value: Option<&Value>, art: Art) -> Option<String> {
    let raw = text(value)?;
    if raw.to_ascii_lowercase().contains("x-plex-token") {
        return None;
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return json::web_url(&raw);
    }
    if !is_server_path(&raw) {
        return None;
    }
    let path = match art.size() {
        Some((width, height)) => {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("width", &width.to_string())
                .append_pair("height", &height.to_string())
                .append_pair("minSize", "1")
                .append_pair("upscale", "1")
                .append_pair("url", &raw)
                .finish();
            format!("/photo/:/transcode?{query}")
        }
        None => raw,
    };
    Some(artwork_url(source_id, &path))
}

/// The title's logo (newer servers list a "clearLogo" image).
fn logo(source_id: &str, raw: &Object) -> Option<String> {
    json::objects(raw.get("Image"))
        .into_iter()
        .find(|image| json::string(image.get("type")).as_deref() == Some("clearLogo"))
        .and_then(|image| artwork(source_id, image.get("url"), Art::Original))
}

/// A title and its year. Plex's Personal Media agent leaves the year in the
/// title ("Holiday (2019)"), so a year in brackets at the end is read when
/// Plex doesn't give one; a title like "Blade Runner 2049" is left alone.
fn title_and_year(raw: &Object) -> Option<(String, Option<i64>)> {
    let title = text(raw.get("title"))?;
    let year = json::int(raw.get("year")).filter(|year| (1870..=2200).contains(year));
    if year.is_none() && title.ends_with(')') {
        let (bare, from_title) = names::split_year(&title);
        if from_title.is_some() {
            return Some((bare, from_title));
        }
    }
    let released = || release_date(raw).and_then(|date| date.get(..4)?.parse().ok());
    Some((title, year.or_else(released)))
}

/// The day it was first shown or released ("2020-05-01").
fn release_date(raw: &Object) -> Option<String> {
    text(raw.get("originallyAvailableAt")).and_then(|date| date.get(..10).map(str::to_string))
}

/// The audience rating, or the critics' when there's none; both are out of 10.
fn rating(raw: &Object) -> Option<f64> {
    [raw.get("audienceRating"), raw.get("rating")]
        .into_iter()
        .filter_map(json::double)
        .find(|rating| *rating > 0.0 && *rating <= 10.0)
}

/// TMDB and IMDb ids from the `Guid` list (asked for with `includeGuids`),
/// or from an older agent's own guid ("com.plexapp.agents.imdb://tt0133093?lang=en").
fn external_ids(raw: &Object) -> (Option<String>, Option<String>) {
    let mut guids: Vec<String> =
        json::objects(raw.get("Guid")).into_iter().filter_map(|guid| json::string(guid.get("id"))).collect();
    guids.extend(json::string(raw.get("guid")));
    let (mut tmdb, mut imdb) = (None, None);
    for guid in guids {
        let Some((scheme, rest)) = guid.split_once("://") else { continue };
        let value = rest.split(['?', '/']).next().unwrap_or_default().trim();
        match scheme {
            "tmdb" | "com.plexapp.agents.themoviedb" if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) => {
                tmdb.get_or_insert_with(|| value.to_string());
            }
            "imdb" | "com.plexapp.agents.imdb" if value.starts_with("tt") => {
                imdb.get_or_insert_with(|| value.to_string());
            }
            _ => {}
        }
    }
    (tmdb, imdb)
}

/// The names in a tag list ("Genre", "Role", "Director"…).
fn tags(raw: &Object, key: &str) -> Vec<String> {
    json::objects(raw.get(key)).into_iter().filter_map(|tag| text(tag.get("tag"))).collect()
}

fn joined(list: Vec<String>) -> Option<String> {
    (!list.is_empty()).then(|| list.join(", "))
}

/// Plex writes some ratings with their country ("gb/15", "de/12"); the
/// rating alone reads better.
fn age_rating(value: Option<&Value>) -> Option<String> {
    let raw = text(value)?;
    let rating = raw.rsplit_once('/').map_or(raw.as_str(), |(_, rating)| rating).trim();
    (!rating.is_empty()).then(|| rating.to_string())
}

/// A length in milliseconds as whole seconds.
fn seconds(value: Option<&Value>) -> Option<i64> {
    json::int(value).filter(|ms| *ms > 0).map(|ms| (ms + 500) / 1000)
}

/// Text with something in it, trimmed.
fn text(value: Option<&Value>) -> Option<String> {
    json::string(value).map(|text| text.trim().to_string()).filter(|text| !text.is_empty())
}

/// plex.tv sends true and false, older servers "1" and "0".
fn flag(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        other => json::int(other) == Some(1),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use xorvio_model::{SourceKind, Timestamp};

    use super::*;
    use crate::sources::server::parse_artwork_path;

    const SECTIONS: &str = include_str!("../../tests/fixtures/plex/sections.json");
    const MOVIES: &str = include_str!("../../tests/fixtures/plex/movies.json");
    const SHOWS: &str = include_str!("../../tests/fixtures/plex/shows.json");
    const EPISODES: &str = include_str!("../../tests/fixtures/plex/episodes.json");
    const MOVIE_DETAILS: &str = include_str!("../../tests/fixtures/plex/movie_details.json");
    const SEASONS: &str = include_str!("../../tests/fixtures/plex/seasons.json");
    const RESOURCES: &str = include_str!("../../tests/fixtures/plex/resources.json");

    fn fixture(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    fn metadata(text: &str) -> Vec<Object> {
        container(&fixture(text)).map(|found| json::objects(found.get("Metadata")).into_iter().cloned().collect()).unwrap_or_default()
    }

    fn session(base: &str, token: Option<&str>) -> ServerSession {
        ServerSession {
            kind: SourceKind::Plex,
            base_url: base.to_string(),
            token: token.map(str::to_string),
            user_id: Some("home-machine".into()),
            device_id: "device-1".into(),
        }
    }

    /// The artwork address of `path` through the photo transcoder.
    fn sized(source_id: &str, width: u32, height: u32, path: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(path.as_bytes()).collect();
        artwork_url(source_id, &format!("/photo/:/transcode?width={width}&height={height}&minSize=1&upscale=1&url={encoded}"))
    }

    fn ok(value: Value) -> (u16, String) {
        (200, value.to_string())
    }

    fn identity_reply(machine: &str) -> (u16, String) {
        ok(json!({"MediaContainer": {"size": 0, "claimed": true, "machineIdentifier": machine, "version": "1.43.5.11079-b3b13e506"}}))
    }

    // MARK: A stand-in server

    /// A request a [`Stand`] received.
    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        path: String,
        /// The path and query as sent.
        target: String,
        query: HashMap<String, String>,
        /// By lowercase name.
        headers: HashMap<String, String>,
    }

    impl Seen {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers.get(&name.to_lowercase()).map(String::as_str)
        }

        fn query(&self, name: &str) -> Option<&str> {
            self.query.get(name).map(String::as_str)
        }
    }

    type Handler = dyn Fn(&Seen) -> (u16, String) + Send + Sync;

    /// A small HTTP server on 127.0.0.1 standing in for plex.tv or a Plex
    /// server, keeping every request it gets.
    struct Stand {
        base: String,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Stand {
        async fn start(handler: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static) -> Stand {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
            let handler: Arc<Handler> = Arc::new(handler);
            let log = seen.clone();
            tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let (log, handler) = (log.clone(), handler.clone());
                    tokio::spawn(async move {
                        let mut head = Vec::new();
                        let mut buffer = [0u8; 4096];
                        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                            match socket.read(&mut buffer).await {
                                Ok(0) | Err(_) => return,
                                Ok(count) => head.extend_from_slice(&buffer[..count]),
                            }
                        }
                        let text = String::from_utf8_lossy(&head).to_string();
                        let mut lines = text.split("\r\n");
                        let mut request_line = lines.next().unwrap_or_default().split_whitespace();
                        let method = request_line.next().unwrap_or_default().to_string();
                        let target = request_line.next().unwrap_or("/").to_string();
                        let parsed = url::Url::parse(&format!("http://stand{target}")).unwrap();
                        let request = Seen {
                            method,
                            path: parsed.path().to_string(),
                            query: parsed.query_pairs().map(|(name, value)| (name.into_owned(), value.into_owned())).collect(),
                            headers: lines
                                .filter_map(|line| line.split_once(':'))
                                .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_string()))
                                .collect(),
                            target,
                        };
                        log.lock().unwrap().push(request.clone());
                        let (status, body) = handler(&request);
                        let reply = format!(
                            "HTTP/1.1 {status} Stand\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = socket.write_all(reply.as_bytes()).await;
                        let _ = socket.shutdown().await;
                    });
                }
            });
            Stand { base, seen }
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    /// An address nothing listens at.
    async fn closed_address() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    /// An address that takes connections and never answers.
    async fn silent_address() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        address
    }

    /// A Plex server with the fixtures' libraries, which wants `token` for
    /// everything but `/identity`.
    fn plex_server(machine: &'static str, token: &'static str) -> impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static {
        move |request| {
            if request.path == "/identity" {
                return identity_reply(machine);
            }
            if request.header("x-plex-token") != Some(token) {
                return (401, String::new());
            }
            match (request.path.as_str(), request.query("type")) {
                ("/", _) => ok(json!({"MediaContainer": {"friendlyName": "Home Server", "machineIdentifier": machine}})),
                ("/library/sections", _) => (200, SECTIONS.into()),
                ("/library/sections/1/all", _) => (200, MOVIES.into()),
                ("/library/sections/2/all", Some("2")) => (200, SHOWS.into()),
                ("/library/sections/2/all", _) => (200, EPISODES.into()),
                ("/library/metadata/101", _) => (200, MOVIE_DETAILS.into()),
                _ => (404, String::new()),
            }
        }
    }

    // MARK: The catalog

    #[test]
    fn a_catalog_from_plex_lists() {
        let libraries = [
            LibraryRead {
                key: "1".into(),
                title: "Films".into(),
                kind: CategoryKind::Movie,
                items: Ok(LibraryItems { titles: metadata(MOVIES), episodes: Vec::new() }),
            },
            LibraryRead {
                key: "2".into(),
                title: "Shows".into(),
                kind: CategoryKind::Series,
                items: Ok(LibraryItems { titles: metadata(SHOWS), episodes: metadata(EPISODES) }),
            },
        ];
        let snapshot = build("src", &libraries);
        let categories: Vec<(&str, CategoryKind, &str, i64)> =
            snapshot.categories.iter().map(|c| (c.id.as_str(), c.kind, c.name.as_str(), c.sort_index)).collect();
        assert_eq!(categories, [("src|moviecat|1", CategoryKind::Movie, "Films", 0), ("src|seriescat|2", CategoryKind::Series, "Shows", 1)]);

        assert_eq!(snapshot.movies.len(), 2, "an entry without a rating key is left out");
        let film = &snapshot.movies[0];
        assert_eq!(film.id, "src|mv|101");
        assert_eq!(
            (film.title.as_str(), film.year, film.rating, film.added_at, film.duration_seconds),
            ("Canyon Survey", Some(2018), Some(8.1), Some(1_700_000_000), Some(5520))
        );
        assert_eq!(film.plot.as_deref(), Some("A surveyor maps a canyon that keeps changing shape."));
        assert_eq!(film.genre.as_deref(), Some("Drama, Thriller"));
        assert_eq!((film.tmdb_id.as_deref(), film.imdb_id.as_deref()), (Some("5501"), Some("tt0000101")));
        assert_eq!(film.category_id.as_deref(), Some("src|moviecat|1"));
        assert_eq!(
            film.stream,
            StreamLocator::Server {
                key: "101".into(),
                part: Some("/library/parts/101/1790001000/file.mkv".into()),
                extension: Some("mkv".into())
            },
            "the original file, not the copy Plex optimized"
        );
        assert_eq!(film.poster_url, Some(sized("src", 480, 720, "/library/metadata/101/thumb/1790001111")));
        assert_eq!(film.backdrop_url, Some(sized("src", 1920, 1080, "/library/metadata/101/art/1790001111")));

        let clips = &snapshot.movies[1];
        assert_eq!((clips.title.as_str(), clips.year), ("Holiday Clips", Some(2019)), "the year left in the title");
        assert_eq!((clips.plot.as_deref(), clips.rating, clips.genre.as_deref()), (None, None, None));
        assert_eq!(clips.poster_url, None, "an image whose address carries a token is left out");
        assert_eq!(clips.backdrop_url.as_deref(), Some("https://metadata-static.plex.tv/b/gracenote/backdrop.jpg"));
        assert_eq!(clips.imdb_id.as_deref(), Some("tt0000102"), "from an older agent's guid");
        assert!(matches!(&clips.stream, StreamLocator::Server { extension: Some(ext), .. } if ext == "mp4"));

        let details = &snapshot.details["src|mv|101"];
        assert_eq!(details.age_rating.as_deref(), Some("15"), "without the country");
        assert_eq!(details.release_date.as_deref(), Some("2018-04-12"));
        assert_eq!(details.release_year(), Some(2018));
        assert_eq!(details.runtime_minutes, Some(92));
        assert_eq!(details.original_title.as_deref(), Some("Le Relevé"));
        assert_eq!(details.tagline.as_deref(), Some("Every line is a promise."));
        assert_eq!(details.imdb_id.as_deref(), Some("tt0000101"));
        assert!(
            details.genres.is_empty() && details.cast.is_empty() && details.director.is_none() && details.country.is_none(),
            "lists name only the first few, which would cut short the title's own details at every refresh"
        );
        assert_eq!(details.audio_channels, Some(6), "of the original, not the optimized copy");
        assert_eq!(details.network, None, "a film's studio isn't a network");
        assert_eq!(details.backdrop_urls, [sized("src", 1920, 1080, "/library/metadata/101/art/1790001111")]);
        assert_eq!(details.logo_url, Some(artwork_url("src", "/library/metadata/101/clearLogo/1790001112")), "logos as they are");
        assert_eq!(details.video, None, "lists don't say whether it's HDR, so that waits for the title's details");
        assert!(details.people.is_empty() && details.overview.is_none());

        let show = &snapshot.series[0];
        assert_eq!(
            (show.id.as_str(), show.remote_id.as_deref(), show.year, show.rating, show.added_at),
            ("src|se|201", Some("201"), Some(2021), Some(7.9), Some(1_702_000_000))
        );
        assert_eq!((show.tmdb_id.as_deref(), show.imdb_id.as_deref()), (Some("7701"), Some("tt0000201")));
        assert_eq!(show.genre.as_deref(), Some("Drama, Mystery"));
        assert_eq!(show.category_id.as_deref(), Some("src|seriescat|2"));
        assert_eq!(snapshot.details["src|se|201"].network.as_deref(), Some("Northwind TV"));
        assert_eq!(snapshot.details["src|se|201"].age_rating.as_deref(), Some("TV-14"));

        let episodes: Vec<(&str, i64, i64, &str)> =
            snapshot.episodes.iter().map(|e| (e.id.as_str(), e.season, e.number, e.title.as_str())).collect();
        assert_eq!(
            episodes,
            [
                ("src|se|201|ep|211", 1, 1, "Pilot"),
                ("src|se|201|ep|212", 1, 2, "The Second Lamp"),
                ("src|se|201|ep|221", 2, 1, "Episode 1"),
            ],
            "in order; an episode Plex hasn't numbered follows the season's others; one of a show that isn't listed is left out"
        );
        let second = &snapshot.episodes[1];
        assert_eq!(second.series_id, "src|se|201");
        assert_eq!(second.plot.as_deref(), Some("The lamp goes dark."));
        assert_eq!(second.duration_seconds, Some(2640));
        assert_eq!(second.air_date, xtream::day_timestamp("2021-10-01"));
        assert_eq!(second.still_url, Some(sized("src", 640, 360, "/library/metadata/212/thumb/1790004444")));
        assert_eq!(snapshot.episodes[0].plot, None, "an empty summary is none");
        assert!(matches!(&snapshot.episodes[2].stream, StreamLocator::Server { key, extension: Some(ext), .. } if key == "221" && ext == "avi"));

        /// Item, kind, position, duration, finished, when.
        type Remembered<'a> = (&'a str, MediaKind, f64, f64, bool, Option<Timestamp>);
        let progress: Vec<Remembered> = snapshot
            .progress
            .iter()
            .map(|p| (p.item_id.as_str(), p.kind, p.position_seconds, p.duration_seconds, p.finished, p.updated_at))
            .collect();
        assert_eq!(
            progress,
            [
                ("src|mv|101", MediaKind::Movie, 1830.0, 5520.0, false, Some(1_790_000_000)),
                ("src|mv|102", MediaKind::Movie, 61.0, 61.0, true, Some(1_791_000_000)),
                ("src|se|201|ep|212", MediaKind::Episode, 600.0, 2640.0, false, Some(1_792_000_000)),
                ("src|se|201|ep|211", MediaKind::Episode, 2700.0, 2700.0, true, Some(1_791_500_000)),
            ]
        );
        assert_eq!(snapshot.progress[2].series_id.as_deref(), Some("src|se|201"));
        assert_eq!(snapshot.progress[0].series_id, None);

        let capabilities = &snapshot.capabilities;
        assert!(capabilities.movies && capabilities.series && !capabilities.live && !capabilities.guide && !capabilities.replay);
        assert!(snapshot.warnings.is_empty() && snapshot.kept_kinds.is_empty());
    }

    #[test]
    fn a_library_that_cant_be_read_keeps_its_titles() {
        let libraries = [
            LibraryRead {
                key: "1".into(),
                title: "Films".into(),
                kind: CategoryKind::Movie,
                items: Ok(LibraryItems { titles: metadata(MOVIES), episodes: Vec::new() }),
            },
            LibraryRead { key: "2".into(), title: "Shows".into(), kind: CategoryKind::Series, items: Err(SourceError::Http(500)) },
        ];
        let snapshot = build("src", &libraries);
        assert_eq!(snapshot.movies.len(), 2);
        assert!(snapshot.series.is_empty());
        assert_eq!(snapshot.kept_kinds, BTreeSet::from([KeptKind::Series]));
        assert!(snapshot.capabilities.series, "the series it had are still there");
        assert_eq!(snapshot.categories.len(), 2);
        assert_eq!(
            snapshot.warnings,
            ["The Shows library couldn't be read (The server responded with HTTP 500), so the titles it had before are still shown."]
        );
    }

    #[test]
    fn titles_and_years() {
        let read = |value: Value| title_and_year(value.as_object().unwrap());
        assert_eq!(read(json!({"title": "Blade Runner 2049"})), Some(("Blade Runner 2049".into(), None)), "a number in the title stays");
        assert_eq!(read(json!({"title": "Holiday (2019)"})), Some(("Holiday".into(), Some(2019))));
        assert_eq!(read(json!({"title": "Holiday (2019)", "year": 2020})), Some(("Holiday (2019)".into(), Some(2020))), "Plex's year wins");
        assert_eq!(read(json!({"title": "Dated", "originallyAvailableAt": "1999-03-31"})), Some(("Dated".into(), Some(1999))));
        assert_eq!(read(json!({"title": "  "})), None);
        assert_eq!(read(json!({"title": "Zero", "year": 0})), Some(("Zero".into(), None)));
    }

    #[test]
    fn artwork_is_sized_and_never_carries_a_token() {
        let address = artwork("src", Some(&json!("/library/metadata/1/thumb/99")), Art::Poster).unwrap();
        let parsed = url::Url::parse(&address).unwrap();
        assert_eq!(
            parse_artwork_path(parsed.path()),
            Some(("src".into(), "/photo/:/transcode?width=480&height=720&minSize=1&upscale=1&url=%2Flibrary%2Fmetadata%2F1%2Fthumb%2F99".into())),
            "the app fetches the transcoder's path from the server"
        );
        assert_eq!(
            artwork("src", Some(&json!("/library/metadata/1/clearLogo/9")), Art::Original),
            Some(artwork_url("src", "/library/metadata/1/clearLogo/9"))
        );
        assert_eq!(
            artwork("src", Some(&json!(" https://metadata-static.plex.tv/p/a.jpg ")), Art::Photo).as_deref(),
            Some("https://metadata-static.plex.tv/p/a.jpg"),
            "Plex's own metadata host is public"
        );
        for refused in [
            "http://192.168.1.5:32400/photo?X-Plex-Token=abc",
            "/library/metadata/1/thumb?x-plex-token=abc",
            "//evil.example/x.jpg",
            "file:///etc/passwd",
            "thumb.jpg",
            "",
        ] {
            assert_eq!(artwork("src", Some(&json!(refused)), Art::Poster), None, "{refused}");
        }
        assert_eq!(artwork("src", None, Art::Poster), None);
    }

    #[test]
    fn external_ids_from_new_and_old_agents() {
        let ids = |value: Value| external_ids(value.as_object().unwrap());
        assert_eq!(
            ids(json!({"guid": "plex://movie/5d77", "Guid": [{"id": "tvdb://3"}, {"id": "tmdb://603"}, {"id": "imdb://tt0133093"}]})),
            (Some("603".into()), Some("tt0133093".into()))
        );
        assert_eq!(ids(json!({"guid": "com.plexapp.agents.themoviedb://1399?lang=en"})), (Some("1399".into()), None));
        assert_eq!(ids(json!({"guid": "com.plexapp.agents.thetvdb://121361/1/1?lang=en"})), (None, None));
        assert_eq!(ids(json!({"Guid": [{"id": "imdb://nm123"}, {"id": "tmdb://abc"}]})), (None, None));
    }

    // MARK: Details

    #[test]
    fn details_from_a_title_and_its_seasons() {
        let raw = &metadata(MOVIE_DETAILS)[0];
        let details = full_details("src", raw, MediaKind::Movie);
        let served_photo = sized("src", 300, 300, "/library/metadata/101/person/31");
        assert_eq!(
            details.people,
            [
                CastMember {
                    name: "Mara Ellison".into(),
                    role: Some("Ada".into()),
                    photo_url: Some("https://metadata-static.plex.tv/people/mara.jpg".into()),
                    tmdb_id: None,
                },
                CastMember { name: "Teo Varga".into(), role: Some("The Surveyor".into()), photo_url: Some(served_photo), tmdb_id: None },
                CastMember { name: "Ines Bell".into(), role: None, photo_url: None, tmdb_id: None },
            ]
        );
        assert_eq!(details.cast, ["Mara Ellison", "Teo Varga", "Ines Bell"]);
        assert_eq!(details.creators, ["Ada Lune", "Bea Script"]);
        assert_eq!(details.director.as_deref(), Some("Joon Park"));
        assert_eq!(details.genres, ["Drama", "Thriller", "Mystery"]);
        assert_eq!(details.country.as_deref(), Some("Ireland, France"));
        assert_eq!(details.overview.as_deref(), Some("A surveyor maps a canyon that keeps changing shape."));
        assert_eq!(details.video, Some(VideoFormat { width: 3840, height: 2160, codec: Some("hevc".into()), is_hdr: true }));
        assert_eq!(details.audio_channels, Some(6));
        assert_eq!(details.logo_url, Some(artwork_url("src", "/library/metadata/101/clearLogo/1790001112")));
        assert_eq!(details.runtime_minutes, Some(92));
        assert_eq!(details.imdb_id.as_deref(), Some("tt0000101"));

        let seasons = season_list("src", &fixture(SEASONS));
        /// Number, name, overview, air date, episodes.
        type Season<'a> = (i64, Option<&'a str>, Option<&'a str>, Option<&'a str>, Option<i64>);
        let read: Vec<Season> = seasons
            .iter()
            .map(|s| (s.number, s.name.as_deref(), s.overview.as_deref(), s.air_date.as_deref(), s.episode_count))
            .collect();
        assert_eq!(
            read,
            [
                (0, Some("Specials"), None, None, Some(0)),
                (1, Some("Season 1"), None, Some("2021-09-24"), Some(2)),
                (2, Some("Season 2"), Some("The storm season."), None, Some(1)),
            ],
            "specials first; a season without a number is left out"
        );
        assert_eq!(seasons[2].cover_url, Some(sized("src", 480, 720, "/library/metadata/220/thumb/1790005555")));
    }

    #[test]
    fn hdr_in_any_form() {
        let hdr = |stream: Value| {
            let media = json!({"width": 3840, "height": 2160, "videoCodec": "hevc"});
            let part = json!({"Stream": [stream, {"streamType": 2, "channels": 6}]});
            video_format(media.as_object().unwrap(), part.as_object().unwrap()).map(|video| video.is_hdr)
        };
        assert_eq!(hdr(json!({"streamType": 1, "colorTrc": "smpte2084"})), Some(true));
        assert_eq!(hdr(json!({"streamType": 1, "colorTrc": "arib-std-b67"})), Some(true), "HLG");
        assert_eq!(hdr(json!({"streamType": 1, "DOVIPresent": true, "colorTrc": "bt709"})), Some(true), "Dolby Vision");
        assert_eq!(hdr(json!({"streamType": 1, "displayTitle": "4K DoVi/HDR10 (HEVC Main 10)"})), Some(true));
        assert_eq!(hdr(json!({"streamType": 1, "colorTrc": "bt709", "displayTitle": "1080p (H.264)"})), Some(false));
        assert_eq!(video_format(json!({"videoCodec": "h264"}).as_object().unwrap(), &Object::new()), None, "no size, no format");
    }

    #[tokio::test]
    async fn details_ask_for_the_title_and_a_shows_seasons() {
        let stand = Stand::start(|request| match request.path.as_str() {
            "/library/metadata/101" => (200, MOVIE_DETAILS.into()),
            "/library/metadata/201" => (200, SHOWS.into()),
            "/library/metadata/201/children" => (200, SEASONS.into()),
            _ => (404, String::new()),
        })
        .await;
        let http = Http::new().unwrap();
        let film = fetch_details(&http, &session(&stand.base, Some("t")), "src", MediaKind::Movie, "101").await.unwrap();
        assert_eq!(film.people.len(), 3);
        assert!(film.seasons.is_empty());
        let show = fetch_details(&http, &session(&stand.base, Some("t")), "src", MediaKind::Series, "201").await.unwrap();
        assert_eq!(show.seasons.iter().map(|s| s.number).collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(show.network.as_deref(), Some("Northwind TV"));
        let asked: Vec<String> = stand.seen().iter().map(|r| r.target.clone()).collect();
        assert_eq!(asked.len(), 3);
        assert!(asked.contains(&"/library/metadata/101?includeGuids=1".to_string()), "{asked:?}");
        assert!(asked.contains(&"/library/metadata/201/children".to_string()), "{asked:?}");
        let missing = fetch_details(&http, &session(&stand.base, Some("t")), "src", MediaKind::Movie, "404").await;
        assert_eq!(missing, Err(SourceError::Http(404)));
    }

    // MARK: Reading a server

    #[tokio::test]
    async fn a_catalog_is_read_a_page_at_a_time_with_the_token_in_a_header() {
        let stand = Stand::start(|request| {
            if request.path == "/identity" {
                return identity_reply("home-machine");
            }
            match (request.path.as_str(), request.query("type")) {
                ("/library/sections", _) => (200, SECTIONS.into()),
                ("/library/sections/1/all", _) => (200, MOVIES.into()),
                ("/library/sections/2/all", Some("2")) => (200, SHOWS.into()),
                ("/library/sections/2/all", _) => {
                    let start: usize = request.query("X-Plex-Container-Start").unwrap().parse().unwrap();
                    let size: usize = request.query("X-Plex-Container-Size").unwrap().parse().unwrap();
                    let total = 1203;
                    let page: Vec<Value> = (start..(start + size).min(total))
                        .map(|n| {
                            json!({"ratingKey": format!("e{n}"), "grandparentRatingKey": "201", "parentIndex": 1 + n / 100,
                                   "index": 1 + n % 100, "title": format!("Episode {n}")})
                        })
                        .collect();
                    ok(json!({"MediaContainer": {"size": page.len(), "totalSize": total, "offset": start, "Metadata": page}}))
                }
                _ => (404, String::new()),
            }
        })
        .await;
        let steps: Arc<Mutex<Vec<String>>> = Arc::default();
        let noted = steps.clone();
        let report = move |step: &str| noted.lock().unwrap().push(step.to_string());
        let snapshot = fetch_catalog(&Http::new().unwrap(), &session(&stand.base, Some("server-token")), "src", &report).await.unwrap();
        assert_eq!(snapshot.episodes.len(), 1203);
        assert_eq!((snapshot.movies.len(), snapshot.series.len()), (2, 1));
        assert_eq!(*steps.lock().unwrap(), [phase::READING_CATALOG]);
        let pages: Vec<(String, String)> = stand
            .seen()
            .iter()
            .filter(|r| r.path == "/library/sections/2/all" && r.query("type") == Some("4"))
            .map(|r| (r.query("X-Plex-Container-Start").unwrap().to_string(), r.query("X-Plex-Container-Size").unwrap().to_string()))
            .collect();
        assert_eq!(pages, [("0".into(), "500".into()), ("500".into(), "500".into()), ("1000".into(), "500".into())]);
        let films = stand.seen().into_iter().find(|r| r.path == "/library/sections/1/all").unwrap();
        assert_eq!((films.query("type"), films.query("includeGuids")), (Some("1"), Some("1")));

        for request in stand.seen() {
            assert_eq!(request.header("x-plex-token"), Some("server-token"), "{}", request.target);
            assert!(!request.target.to_lowercase().contains("token"), "{}", request.target);
            assert_eq!(request.header("x-plex-client-identifier"), Some("device-1"));
            assert_eq!(request.header("x-plex-product"), Some("Xorvio"));
            assert_eq!(request.header("accept"), Some("application/json"));
            assert_eq!(request.method, "GET");
        }
        let catalog = serde_json::to_string(&(&snapshot.categories, &snapshot.movies, &snapshot.series, &snapshot.episodes)).unwrap();
        let details = serde_json::to_string(&snapshot.details).unwrap();
        for text in [catalog, details] {
            for secret in ["server-token", "X-Plex-Token", "leaked-token"] {
                assert!(!text.contains(secret), "the catalog holds {secret}");
            }
        }
    }

    #[tokio::test]
    async fn a_server_that_doesnt_page_is_read_once() {
        let stand = Stand::start(|request| match request.path.as_str() {
            "/identity" => identity_reply("home-machine"),
            "/library/sections" => ok(json!({"MediaContainer": {"Directory": [{"key": "1", "type": "movie", "title": "Films"}]}})),
            "/library/sections/1/all" => {
                let mut films = fixture(MOVIES);
                films["MediaContainer"].as_object_mut().unwrap().remove("totalSize");
                ok(films)
            }
            _ => (404, String::new()),
        })
        .await;
        let snapshot = fetch_catalog(&Http::new().unwrap(), &session(&stand.base, None), "src", &|_| {}).await.unwrap();
        assert_eq!(snapshot.movies.len(), 2);
        assert_eq!(stand.seen().iter().filter(|r| r.path == "/library/sections/1/all").count(), 1);
        assert!(!snapshot.capabilities.series);
    }

    #[tokio::test]
    async fn catalog_failures() {
        let http = Http::new().unwrap();
        // Another server has taken the address.
        let elsewhere = Stand::start(|request| match request.path.as_str() {
            "/identity" => identity_reply("someone-else"),
            _ => (200, SECTIONS.into()),
        })
        .await;
        let error = fetch_catalog(&http, &session(&elsewhere.base, None), "src", &|_| {}).await.unwrap_err();
        assert!(error.to_string().contains("different Plex server"), "{error}");
        assert_eq!(elsewhere.seen().len(), 1, "nothing read from the wrong server");

        // Every library fails: the refresh fails, keeping the catalog there was.
        let failing = Stand::start(|request| match request.path.as_str() {
            "/identity" => identity_reply("home-machine"),
            "/library/sections" => (200, SECTIONS.into()),
            _ => (500, String::new()),
        })
        .await;
        assert_eq!(fetch_catalog(&http, &session(&failing.base, None), "src", &|_| {}).await.unwrap_err(), SourceError::Http(500));

        // One fails: the other is read, and the failed one's titles are kept.
        let half = Stand::start(|request| match (request.path.as_str(), request.query("type")) {
            ("/identity", _) => identity_reply("home-machine"),
            ("/library/sections", _) => (200, SECTIONS.into()),
            ("/library/sections/1/all", _) => (200, MOVIES.into()),
            ("/library/sections/2/all", Some("2")) => (200, SHOWS.into()),
            _ => (503, String::new()),
        })
        .await;
        let snapshot = fetch_catalog(&http, &session(&half.base, None), "src", &|_| {}).await.unwrap();
        assert_eq!(snapshot.movies.len(), 2);
        assert!(snapshot.series.is_empty() && snapshot.kept_kinds.contains(&KeptKind::Series));
        assert_eq!(snapshot.warnings.len(), 1);

        // A token that's been revoked is a sign-in problem, so the source says it needs one.
        let locked = Stand::start(plex_server("home-machine", "right-token")).await;
        let error = fetch_catalog(&http, &session(&locked.base, Some("revoked")), "src", &|_| {}).await.unwrap_err();
        assert!(error.is_authentication(), "{error:?}");
    }

    // MARK: Adding a server by its address

    #[tokio::test]
    async fn checking_a_server_at_an_address() {
        let http = Http::new().unwrap();
        let open = Stand::start(|request| match request.path.as_str() {
            "/identity" => identity_reply("m1"),
            "/" => ok(json!({"MediaContainer": {"friendlyName": "Living Room", "machineIdentifier": "m1"}})),
            _ => (404, String::new()),
        })
        .await;
        let login = check_server(&http, &open.base, None, "device-1").await.unwrap();
        assert_eq!(
            login,
            ServerLogin { base_url: open.base.clone(), token: None, user_id: Some("m1".into()), server_name: Some("Living Room".into()) }
        );

        let locked = Stand::start(plex_server("m2", "good")).await;
        let error = check_server(&http, &locked.base, None, "device-1").await.unwrap_err();
        assert!(error.to_string().contains("Sign in with Plex"), "{error}");
        let error = check_server(&http, &locked.base, Some("bad"), "device-1").await.unwrap_err();
        assert!(error.to_string().contains("didn't accept the sign-in"), "{error}");
        let login = check_server(&http, &locked.base, Some(" good "), "device-1").await.unwrap();
        assert_eq!(
            (login.token.as_deref(), login.user_id.as_deref(), login.server_name.as_deref()),
            (Some("good"), Some("m2"), Some("Home Server"))
        );

        let jellyfin = Stand::start(|request| match request.path.as_str() {
            "/identity" => (200, "<html>Jellyfin</html>".into()),
            _ => (404, String::new()),
        })
        .await;
        let error = check_server(&http, &jellyfin.base, None, "device-1").await.unwrap_err();
        assert!(error.to_string().contains("doesn't answer like a Plex server"), "{error}");
        let missing = Stand::start(|_| (404, String::new())).await;
        assert_eq!(check_server(&http, &missing.base, None, "device-1").await.unwrap_err(), not_plex());
        let error = check_server(&http, &closed_address().await, None, "device-1").await.unwrap_err();
        assert!(error.to_string().contains("not accepting connections"), "{error}");
    }

    // MARK: Signing in through plex.tv

    #[tokio::test]
    async fn signing_in_through_plex_tv() {
        let polls = Arc::new(AtomicUsize::new(0));
        let counted = polls.clone();
        let plex_tv = Stand::start(move |request| match (request.method.as_str(), request.path.as_str()) {
            ("POST", "/api/v2/pins") => (201, json!({"id": 4242, "code": "abcd1234efgh", "expiresIn": 1800, "authToken": null}).to_string()),
            ("GET", "/api/v2/pins/4242") if counted.fetch_add(1, Ordering::SeqCst) == 0 => {
                ok(json!({"id": 4242, "code": "abcd1234efgh", "expiresIn": 1700, "authToken": null}))
            }
            ("GET", "/api/v2/pins/4242") => ok(json!({"id": 4242, "expiresIn": 1690, "authToken": "account-token"})),
            ("GET", "/api/v2/pins/77") => (404, json!({"errors": [{"code": 1020, "message": "Code not found or expired"}]}).to_string()),
            ("GET", "/api/v2/pins/78") => ok(json!({"id": 78, "expiresIn": 0, "authToken": null})),
            ("GET", "/api/v2/resources") if request.header("x-plex-token") == Some("account-token") => (200, RESOURCES.into()),
            ("GET", "/api/v2/resources") => (401, String::new()),
            _ => (404, String::new()),
        })
        .await;
        let http = Http::new().unwrap();
        let pin = start_sign_in(&http, &plex_tv.base, "device 1").await.unwrap();
        assert_eq!((pin.id, pin.code.as_str()), (4242, "abcd1234efgh"));
        assert_eq!(pin.auth_url, "https://app.plex.tv/auth#?clientID=device+1&code=abcd1234efgh&context%5Bdevice%5D%5Bproduct%5D=Xorvio");
        let asked = &plex_tv.seen()[0];
        assert_eq!((asked.method.as_str(), asked.query("strong")), ("POST", Some("true")));
        assert_eq!(asked.header("x-plex-client-identifier"), Some("device 1"));
        assert_eq!(asked.header("x-plex-product"), Some("Xorvio"));
        assert!(asked.header("x-plex-version").is_some() && asked.header("x-plex-platform").is_some());
        assert_eq!(asked.header("x-plex-token"), None);

        assert_eq!(check_sign_in(&http, &plex_tv.base, "device 1", 4242).await, Ok(None), "not approved yet");
        assert_eq!(check_sign_in(&http, &plex_tv.base, "device 1", 4242).await, Ok(Some("account-token".into())));
        for expired in [77, 78] {
            let error = check_sign_in(&http, &plex_tv.base, "device 1", expired).await.unwrap_err();
            assert_eq!(error.to_string(), "This Plex sign-in has expired. Start signing in again.");
        }

        let servers = servers(&http, &plex_tv.base, "device 1", "account-token").await.unwrap();
        let listed: Vec<(&str, &str, bool, &str)> =
            servers.iter().map(|s| (s.id.as_str(), s.name.as_str(), s.owned, s.access_token.as_str())).collect();
        assert_eq!(
            listed,
            [
                ("home-machine", "Home Server", true, "home-server-token"),
                ("nas-machine", "Old NAS", true, "account-token"),
                ("shared-machine", "Friend's Server", false, "shared-server-token"),
            ],
            "the account's own first; players, devices without an id and shared servers without a token left out"
        );
        assert_eq!(servers[0].connections.len(), 3);
        assert_eq!(
            servers[0].connections[2],
            PlexConnection { uri: "https://192-168-1-5.def456.plex.direct:32400".into(), local: true, relay: false }
        );
        assert!(servers[0].connections[0].relay);
        assert_eq!(servers[1].connections, [PlexConnection { uri: "http://10.0.0.2:32400".into(), local: true, relay: false }]);
        let asked = plex_tv.seen().into_iter().find(|r| r.path == "/api/v2/resources").unwrap();
        assert_eq!((asked.query("includeHttps"), asked.query("includeRelay")), (Some("1"), Some("1")));
        assert!(!asked.target.contains("account-token"));

        let error = servers_or_error(&http, &plex_tv.base).await;
        assert!(error.contains("Sign in with Plex again"), "{error}");
    }

    async fn servers_or_error(http: &Http, plex_tv: &str) -> String {
        servers(http, plex_tv, "device 1", "stale-token").await.unwrap_err().to_string()
    }

    #[tokio::test]
    async fn the_nearest_address_that_answers_as_the_server() {
        let http = Http::new().unwrap();
        let near = Stand::start(|_| identity_reply("home-machine")).await;
        let relay = Stand::start(|_| identity_reply("home-machine")).await;
        let impostor = Stand::start(|_| identity_reply("someone-else")).await;
        let (closed, silent) = (closed_address().await, silent_address().await);
        let server = |connections: &[(&str, bool, bool)]| PlexServer {
            id: "home-machine".into(),
            name: "Home Server".into(),
            owned: true,
            access_token: "home-server-token".into(),
            connections: connections.iter().map(|(uri, local, relay)| PlexConnection { uri: uri.to_string(), local: *local, relay: *relay }).collect(),
        };

        // A local address wins over one listed first.
        let chosen = best_connection(&http, "device-1", &server(&[(&relay.base, false, false), (&near.base, true, false)])).await;
        assert_eq!(chosen, Ok(near.base.clone()));
        assert_eq!(near.seen()[0].header("x-plex-token"), Some("home-server-token"));
        assert_eq!(near.seen()[0].path, "/identity");
        // Local out of reach and a remote address that's another server: the relay.
        let chosen = best_connection(
            &http,
            "device-1",
            &server(&[(&relay.base, false, true), (&impostor.base, false, false), (&closed, true, false)]),
        )
        .await;
        assert_eq!(chosen, Ok(relay.base.clone()));
        // A local address that never answers costs a few seconds, not the whole wait.
        let started = std::time::Instant::now();
        let chosen = best_connection(&http, "device-1", &server(&[(&silent, true, false), (&near.base, false, false)])).await;
        assert_eq!(chosen, Ok(near.base.clone()));
        assert!(started.elapsed() < CONNECTION_TIMEOUT + Duration::from_secs(2), "{:?}", started.elapsed());
        // Nothing answers.
        let error = best_connection(&http, "device-1", &server(&[(&closed, true, false), (&impostor.base, false, false)])).await.unwrap_err();
        assert!(error.to_string().starts_with("None of Home Server's addresses answered."), "{error}");
        assert!(best_connection(&http, "device-1", &server(&[])).await.is_err());
    }

    // MARK: Playing and what's been watched

    #[test]
    fn streams_keep_the_token_in_a_header() {
        let plex = session("http://plex.local:32400", Some("server-token"));
        let target = stream_target(&plex, "101", Some("/library/parts/101/1790001000/file.mkv")).unwrap();
        assert_eq!(target.url, "http://plex.local:32400/library/parts/101/1790001000/file.mkv");
        assert!(!target.is_live);
        assert!(target.headers.contains(&("X-Plex-Token".into(), "server-token".into())));
        assert!(target.headers.contains(&("X-Plex-Client-Identifier".into(), "device-1".into())));
        assert!(target.headers.iter().all(|(name, _)| name != "Accept"), "the file is a file, not JSON");
        for part in [None, Some("http://elsewhere.example/file.mkv"), Some("//elsewhere/file.mkv")] {
            let error = stream_target(&plex, "101", part).unwrap_err();
            assert!(error.to_string().contains("Refresh the source"), "{error}");
        }
        let open = stream_target(&session("http://plex.local:32400", None), "101", Some("/library/parts/1/2/file.mp4")).unwrap();
        assert!(open.headers.iter().all(|(name, _)| name != "X-Plex-Token"));
    }

    #[tokio::test]
    async fn where_titles_were_left_and_what_was_watched_reach_the_server() {
        // The server's (viewCount, viewOffset) for the title.
        let state = Arc::new(Mutex::new((0, 0)));
        let shared = state.clone();
        let stand = Stand::start(move |request| match request.path.as_str() {
            "/library/metadata/101" => {
                let (count, offset) = *shared.lock().unwrap();
                ok(json!({"MediaContainer": {"Metadata": [{"ratingKey": "101", "viewCount": count, "viewOffset": offset}]}}))
            }
            "/:/timeline" => ok(json!({"MediaContainer": {"playbackState": "progress"}})),
            "/:/scrobble" | "/:/unscrobble" => (200, String::new()),
            _ => (404, String::new()),
        })
        .await;
        let http = Http::new().unwrap();
        let plex = session(&stand.base, Some("server-token"));
        let calls = |from: usize| -> Vec<String> { stand.seen()[from..].iter().map(|r| format!("{} {}", r.method, r.path)).collect() };

        report_progress(&http, &plex, "101", 125.5, 5520.0, false).await.unwrap();
        let timeline = &stand.seen()[0];
        assert_eq!((timeline.method.as_str(), timeline.path.as_str()), ("PUT", "/:/timeline"));
        let query: Vec<(&str, Option<&str>)> =
            ["ratingKey", "key", "identifier", "state", "time", "duration"].iter().map(|name| (*name, timeline.query(name))).collect();
        assert_eq!(
            query,
            [
                ("ratingKey", Some("101")),
                ("key", Some("/library/metadata/101")),
                ("identifier", Some("com.plexapp.plugins.library")),
                ("state", Some("stopped")),
                ("time", Some("125500")),
                ("duration", Some("5520000")),
            ]
        );
        assert_eq!(timeline.header("x-plex-token"), Some("server-token"));

        // Finished, and the server hasn't counted it: marked watched.
        report_progress(&http, &plex, "101", 5400.0, 5520.0, true).await.unwrap();
        assert_eq!(calls(1), ["GET /library/metadata/101", "PUT /:/scrobble"]);
        let scrobble = stand.seen().pop().unwrap();
        assert_eq!((scrobble.query("key"), scrobble.query("identifier")), (Some("101"), Some("com.plexapp.plugins.library")));
        // Already watched: the credits' reports don't count it again.
        *state.lock().unwrap() = (1, 0);
        report_progress(&http, &plex, "101", 5450.0, 5520.0, true).await.unwrap();
        assert_eq!(calls(3), ["GET /library/metadata/101"]);
        // Watched before and being watched again: this viewing counts.
        *state.lock().unwrap() = (1, 600_000);
        report_progress(&http, &plex, "101", 5460.0, 5520.0, true).await.unwrap();
        assert_eq!(calls(4), ["GET /library/metadata/101", "PUT /:/scrobble"]);

        set_played(&http, &plex, "101", true).await.unwrap();
        set_played(&http, &plex, "101", false).await.unwrap();
        assert_eq!(calls(6), ["PUT /:/scrobble", "PUT /:/unscrobble"]);
        for request in stand.seen() {
            assert_eq!(request.header("x-plex-token"), Some("server-token"));
            assert!(!request.target.contains("server-token"), "{}", request.target);
        }
    }

    // MARK: Through Xorvio

    #[tokio::test]
    async fn signing_in_with_plex_adds_a_server_and_its_catalog() {
        let server = Stand::start(plex_server("home-machine", "home-server-token")).await;
        let unreachable = closed_address().await;
        let resources = json!([
            {"name": "Home Server", "clientIdentifier": "home-machine", "provides": "server", "owned": true, "accessToken": "home-server-token",
             "connections": [{"uri": unreachable, "local": true, "relay": false}, {"uri": server.base.clone(), "local": false, "relay": false}]},
            {"name": "Friend's Server", "clientIdentifier": "shared-machine", "provides": "server", "owned": false,
             "accessToken": "shared-server-token", "connections": []}
        ]);
        let approved = Arc::new(AtomicUsize::new(0));
        let polls = approved.clone();
        let plex_tv = Stand::start(move |request| match (request.method.as_str(), request.path.as_str()) {
            ("POST", "/api/v2/pins") => (201, json!({"id": 9, "code": "strongcode", "expiresIn": 1800}).to_string()),
            ("GET", "/api/v2/pins/9") if polls.fetch_add(1, Ordering::SeqCst) == 0 => ok(json!({"id": 9, "authToken": null, "expiresIn": 1700})),
            ("GET", "/api/v2/pins/9") => ok(json!({"id": 9, "authToken": "account-token", "expiresIn": 1600})),
            ("GET", "/api/v2/resources") if request.header("x-plex-token") == Some("account-token") => ok(resources.clone()),
            _ => (401, String::new()),
        })
        .await;
        let folder = tempfile::tempdir().unwrap();
        let core = crate::Core::open(folder.path(), Arc::new(crate::MemorySecrets::default())).unwrap();
        core.set_plex_tv_base(&plex_tv.base);

        let start = core.plex_start_sign_in().await.unwrap();
        assert_eq!(start.code, "strongcode");
        assert!(start.auth_url.starts_with("https://app.plex.tv/auth#?clientID="), "{}", start.auth_url);
        assert!(core.plex_check_sign_in(start.pin_id).await.unwrap().is_none(), "waiting for approval");
        let choices = core.plex_check_sign_in(start.pin_id).await.unwrap().unwrap();
        assert_eq!(choices.iter().map(|c| (c.name.as_str(), c.owned)).collect::<Vec<_>>(), [("Home Server", true), ("Friend's Server", false)]);
        let shown = serde_json::to_string(&choices).unwrap();
        assert!(!shown.contains("token"), "the interface never sees a token: {shown}");

        let config = core.add_plex_server(start.pin_id, "home-machine", "").await.unwrap();
        assert_eq!((config.name.as_str(), config.kind), ("Home Server", SourceKind::Plex));
        assert_eq!(core.source_secrets(&config.id).unwrap().token, None, "the edit sheet never sees the token");
        assert!(core.add_plex_server(start.pin_id, "home-machine", "").await.is_err(), "a sign-in adds one server");

        let status = core.refresh_source(&config.id, &|_| {}).await.unwrap();
        assert_eq!(status.last_error, None);
        assert_eq!((status.movie_count, status.series_count), (2, 1));
        assert_eq!(status.catalog_warning, None);
        for request in server.seen() {
            if request.path != "/identity" {
                assert_eq!(request.header("x-plex-token"), Some("home-server-token"), "{}", request.target);
            }
            assert!(!request.target.contains("token"), "{}", request.target);
        }

        let film_id = format!("{}|mv|101", config.id);
        let film = core.movie(&film_id).unwrap().unwrap();
        let episodes = core.episodes(&format!("{}|se|201", config.id)).await.unwrap();
        assert_eq!(episodes.len(), 3, "episodes come with the catalog");
        let catalog = serde_json::to_string(&(core.movies(&crate::BrowseQuery::default()).unwrap(), episodes)).unwrap();
        for secret in ["home-server-token", "account-token", "X-Plex-Token"] {
            assert!(!catalog.contains(secret), "the catalog holds {secret}");
        }
        let remembered = core.progress(&film_id).unwrap().expect("where the server says it was left");
        assert_eq!((remembered.position_seconds, remembered.finished), (1830.0, false));
        assert!(core.continue_watching(10).unwrap().iter().any(|item| item.item_id == film_id));

        let details = core.details(&film_id).await.unwrap();
        assert_eq!(details.people.len(), 3);
        assert!(details.video.is_some_and(|video| video.is_hdr));

        let target = core.stream_target(&config.id, &film.stream, false).unwrap();
        assert_eq!(target.url, format!("{}/library/parts/101/1790001000/file.mkv", server.base));
        assert!(target.headers.contains(&("X-Plex-Token".into(), "home-server-token".into())));
    }
}
