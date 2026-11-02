//! Trakt end to end, through [`Core`]'s public calls, against a stand-in
//! Trakt (and a small Xtream panel for the catalog) on 127.0.0.1. Nothing
//! here reaches api.trakt.tv, the keychain or anyone's own data.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use xorvio_model::*;

use super::TraktEvent;
use super::sync::iso_time;
use crate::store::library::{TraktPaused, TraktTitle, TraktWatched};
use crate::trakt::TraktIds;
use crate::{AddonReach, BrowseQuery, Core, MemorySecrets, SecretStore, now};

const CLIENT_ID: &str = "test-client";
const DEVICE_CODE: &str = "device-code-kept-in-xorvio";

// MARK: The stand-in

#[derive(Debug, Clone)]
struct Call {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Value,
    at: Instant,
}

impl Call {
    fn is(&self, method: &str, path: &str) -> bool {
        self.method == method && self.path == path
    }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Value,
}

fn reply(status: u16, body: Value) -> Reply {
    Reply { status, headers: Vec::new(), body }
}

/// What the stand-in Trakt knows and says.
struct Fake {
    /// How many times the device token is still waiting before it's approved.
    pending_checks: usize,
    /// Seconds new access tokens last.
    lifetime: i64,
    issued: usize,
    access: String,
    refresh: String,
    /// Access tokens answered with 401 (revoked on Trakt's side).
    rejected: HashSet<String>,
    refuse_refresh: bool,
    revoked: Vec<String>,
    /// Paths answered once with a 429 and a Retry-After.
    limit_once: HashSet<String>,
    /// What checking the device code answers, instead of the usual.
    device_answer: Option<u16>,
    /// Every film and show it can name, by `{title, year, ids}`.
    titles: Vec<(bool, Value)>,
    watched_movies: Vec<Value>,
    watched_shows: Vec<Value>,
    playback: Vec<Value>,
    watchlist: Vec<Value>,
    /// Bumped as watched films, watched episodes, places and the watchlist change.
    versions: [i64; 4],
    /// Charts and public lists, by path: anyone with the app's key may read them.
    public: HashMap<String, Value>,
}

impl Fake {
    fn new() -> Fake {
        let titles = vec![
            (false, json!({"title": "The Glass Valley", "year": 2018, "ids": {"trakt": 1, "slug": "the-glass-valley", "imdb": "tt0000101", "tmdb": 550}})),
            (false, json!({"title": "Paper Boats", "year": 2021, "ids": {"trakt": 2}})),
            (true, json!({"title": "Tidewater", "year": 2025, "ids": {"trakt": 3, "imdb": "tt0000301", "tmdb": 1396}})),
            (false, json!({"title": "Old Lantern", "year": 2010, "ids": {"trakt": 4, "tmdb": 777}})),
            (true, json!({"title": "Far Shore", "year": 2020, "ids": {"trakt": 5, "imdb": "tt0000501"}})),
            (false, json!({"title": "Absent", "year": 2022, "ids": {"trakt": 6, "imdb": "tt0000601"}})),
            (false, json!({"title": "Harbour Lights", "year": 2019, "ids": {"trakt": 7, "imdb": "tt0000701"}})),
        ];
        Fake {
            pending_checks: 1,
            lifetime: 7 * 86_400,
            issued: 0,
            access: String::new(),
            refresh: String::new(),
            rejected: HashSet::new(),
            refuse_refresh: false,
            revoked: Vec::new(),
            limit_once: HashSet::new(),
            device_answer: None,
            titles,
            watched_movies: Vec::new(),
            watched_shows: Vec::new(),
            playback: Vec::new(),
            watchlist: Vec::new(),
            versions: [1, 1, 1, 1],
            public: HashMap::new(),
        }
    }

    fn title(&self, show: bool, ids: &Value) -> Option<Value> {
        let wanted = TraktIds::from_json(Some(ids));
        self.titles
            .iter()
            .find(|(is_show, title)| *is_show == show && TraktIds::from_json(title.get("ids")).shares_id(&wanted))
            .map(|(_, title)| title.clone())
    }

    fn named(&self, show: bool, trakt: i64) -> Value {
        self.title(show, &json!({"trakt": trakt})).unwrap()
    }

    fn issue(&mut self) -> Value {
        self.issued += 1;
        self.access = format!("access-token-{}", self.issued);
        self.refresh = format!("refresh-token-{}", self.issued);
        json!({"access_token": self.access, "token_type": "bearer", "expires_in": self.lifetime,
               "refresh_token": self.refresh, "scope": "public", "created_at": now()})
    }

    fn activities(&self) -> Value {
        let at = |version: i64| iso_time(1_700_000_000 + version);
        json!({
            "all": at(self.versions.iter().sum()),
            "movies": {"watched_at": at(self.versions[0]), "paused_at": at(self.versions[2]), "watchlisted_at": at(self.versions[3])},
            "episodes": {"watched_at": at(self.versions[1]), "paused_at": at(self.versions[2])},
            "shows": {"watchlisted_at": at(self.versions[3])},
            "watchlist": {"updated_at": at(self.versions[3])}
        })
    }

    fn answer(&mut self, call: &Call) -> Reply {
        if call.path == "/player_api.php" || call.path == "/xmltv.php" {
            return panel(call);
        }
        if self.limit_once.remove(&call.path) {
            return Reply { status: 429, headers: vec![("Retry-After".into(), "1".into())], body: json!({}) };
        }
        let body = &call.body;
        if call.path.starts_with("/oauth/") {
            return self.oauth(call);
        }
        // Everything else is the API, which needs the app's key and a current token.
        let bearer = call.headers.get("authorization").and_then(|value| value.strip_prefix("Bearer ")).unwrap_or_default();
        if call.headers.get("trakt-api-key").map(String::as_str) != Some(CLIENT_ID)
            || call.headers.get("trakt-api-version").map(String::as_str) != Some("2")
        {
            return reply(403, json!({}));
        }
        if let Some(body) = self.public.get(&call.path) {
            return reply(200, body.clone());
        }
        if bearer != self.access || self.rejected.contains(bearer) {
            return reply(401, json!({"error": "invalid_token"}));
        }
        let path = call.path.as_str();
        match (call.method.as_str(), path) {
            ("GET", "/users/settings") => reply(200, json!({"user": {"username": "walter", "private": false}})),
            ("GET", "/sync/last_activities") => reply(200, self.activities()),
            ("GET", "/sync/watched/movies") => reply(200, Value::Array(self.watched_movies.clone())),
            ("GET", "/sync/watched/shows") => reply(200, Value::Array(self.watched_shows.clone())),
            ("GET", "/sync/playback") => reply(200, Value::Array(self.playback.clone())),
            ("GET", "/sync/watchlist/movies") | ("GET", "/sync/watchlist/shows") => {
                let kind = if path.ends_with("movies") { "movie" } else { "show" };
                reply(200, Value::Array(self.watchlist.iter().filter(|item| item["type"] == kind).cloned().collect()))
            }
            ("POST", "/sync/watchlist") | ("POST", "/sync/watchlist/remove") => {
                let adding = path == "/sync/watchlist";
                for (key, kind, show) in [("movies", "movie", false), ("shows", "show", true)] {
                    for item in body[key].as_array().into_iter().flatten() {
                        let Some(title) = self.title(show, &item["ids"]) else { continue };
                        let ids = TraktIds::from_json(title.get("ids"));
                        self.watchlist.retain(|listed| !(listed["type"] == kind && TraktIds::from_json(listed[kind].get("ids")).shares_id(&ids)));
                        if adding {
                            self.watchlist.push(json!({"type": kind, "listed_at": iso_time(now()), kind: title}));
                        }
                    }
                }
                self.versions[3] += 1;
                reply(201, json!({"added": {}, "deleted": {}, "existing": {}, "not_found": {}}))
            }
            ("POST", "/sync/history") => reply(201, json!({"added": {"movies": 0, "episodes": 0}})),
            ("POST", "/sync/history/remove") => {
                for item in body["movies"].as_array().into_iter().flatten() {
                    let ids = TraktIds::from_json(item.get("ids"));
                    self.watched_movies.retain(|watched| !TraktIds::from_json(watched["movie"].get("ids")).shares_id(&ids));
                }
                self.versions[0] += 1;
                reply(200, json!({"deleted": {}}))
            }
            ("DELETE", _) if path.starts_with("/sync/playback/") => {
                let id: i64 = path.trim_start_matches("/sync/playback/").parse().unwrap();
                let before = self.playback.len();
                self.playback.retain(|place| place["id"] != id);
                if self.playback.len() == before {
                    return reply(404, json!({}));
                }
                self.versions[2] += 1;
                reply(204, Value::Null)
            }
            ("POST", _) if path.starts_with("/scrobble/") => reply(201, json!({"id": 0, "action": path.trim_start_matches("/scrobble/")})),
            ("GET", "/search/movie") | ("GET", "/search/show") => {
                let show = path.ends_with("show");
                let query = call.query.get("query").cloned().unwrap_or_default();
                let kind = if show { "show" } else { "movie" };
                let found: Vec<Value> = self
                    .titles
                    .iter()
                    .filter(|(is_show, title)| *is_show == show && title["title"] == query.as_str())
                    .map(|(_, title)| json!({"type": kind, "score": 100, kind: title}))
                    .collect();
                reply(200, Value::Array(found))
            }
            ("GET", "/search/tmdb/1396") => reply(200, json!([{"type": "show", "score": 1000, "show": self.named(true, 3)}])),
            ("GET", _) if path.starts_with("/shows/") => {
                // /shows/{id}/seasons/{season}/episodes/{number}
                let parts: Vec<&str> = path.split('/').collect();
                if parts.len() != 7 || parts[2] != "3" {
                    return reply(404, json!({}));
                }
                let (season, number): (i64, i64) = (parts[4].parse().unwrap(), parts[6].parse().unwrap());
                reply(200, json!({"season": season, "number": number, "title": "An episode", "ids": {"trakt": 9000 + season * 100 + number}}))
            }
            _ => reply(404, json!({})),
        }
    }

    fn oauth(&mut self, call: &Call) -> Reply {
        let body = &call.body;
        match call.path.as_str() {
            "/oauth/device/code" if body["client_id"] == CLIENT_ID => reply(
                200,
                json!({"device_code": DEVICE_CODE, "user_code": "5A8B1C2D", "verification_url": "https://trakt.tv/activate",
                       "expires_in": 600, "interval": 1}),
            ),
            // Desktop apps send no client secret: Trakt asks them not to.
            "/oauth/device/token" if body["code"] == DEVICE_CODE && body["client_id"] == CLIENT_ID && body.get("client_secret").is_none() => {
                if let Some(status) = self.device_answer {
                    return reply(status, json!({}));
                }
                if self.pending_checks > 0 {
                    self.pending_checks -= 1;
                    return reply(400, json!({}));
                }
                let tokens = self.issue();
                reply(200, tokens)
            }
            "/oauth/device/token" => reply(404, json!({})),
            "/oauth/token" => {
                if self.refuse_refresh || body.get("client_secret").is_some() || body["refresh_token"] != self.refresh.as_str() || body["grant_type"] != "refresh_token" {
                    return reply(400, json!({"error": "invalid_grant", "error_description": "session not found"}));
                }
                let tokens = self.issue();
                reply(200, tokens)
            }
            "/oauth/revoke" if body.get("client_secret").is_none() => {
                self.revoked.push(body["token"].as_str().unwrap_or_default().to_string());
                reply(200, Value::Null)
            }
            _ => reply(404, json!({})),
        }
    }
}

/// A small Xtream panel: four films and a series whose episodes come only
/// when it's opened.
fn panel(call: &Call) -> Reply {
    if call.path == "/xmltv.php" {
        return Reply { status: 200, headers: Vec::new(), body: Value::String("<tv></tv>".into()) };
    }
    let body = match call.query.get("action").map(String::as_str) {
        None => json!({"user_info": {"auth": 1, "status": "Active", "exp_date": (now() + 86_400 * 365).to_string()}, "server_info": {}}),
        Some("get_live_categories") => json!([{"category_id": "1", "category_name": "News"}]),
        Some("get_vod_categories") => json!([{"category_id": "5", "category_name": "Films"}]),
        Some("get_series_categories") => json!([{"category_id": "7", "category_name": "Series"}]),
        Some("get_live_streams") => json!([{"stream_id": 1, "name": "Harbor News", "category_id": "1"}]),
        Some("get_vod_streams") => json!([
            {"stream_id": "20", "name": "The Glass Valley (2018)", "tmdb": "550", "category_id": "5", "container_extension": "mkv"},
            {"stream_id": "21", "name": "Paper Boats (2021)", "category_id": "5", "container_extension": "mkv"},
            {"stream_id": "22", "name": "Harbour Lights (2019)", "category_id": "5", "container_extension": "mkv"},
            {"stream_id": "23", "name": "Old Lantern (2010)", "tmdb": "777", "category_id": "5", "container_extension": "mkv"}
        ]),
        Some("get_series") => json!([{"series_id": 30, "name": "Tidewater", "tmdb": "1396", "category_id": "7", "releaseDate": "2025-01-01"}]),
        Some("get_series_info") => json!({
            "info": {"name": "Tidewater"},
            "episodes": {"1": [
                {"id": "301", "episode_num": 1, "title": "Low Tide", "container_extension": "mp4", "info": {"duration_secs": "2640"}},
                {"id": "302", "episode_num": 2, "title": "High Water", "container_extension": "mp4", "info": {"duration_secs": "2700"}}
            ]}
        }),
        Some(_) => json!([]),
    };
    reply(200, body)
}

struct StandIn {
    base: String,
    calls: Arc<Mutex<Vec<Call>>>,
    fake: Arc<Mutex<Fake>>,
}

impl StandIn {
    async fn start() -> StandIn {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let calls: Arc<Mutex<Vec<Call>>> = Arc::default();
        let fake = Arc::new(Mutex::new(Fake::new()));
        let (log, state) = (calls.clone(), fake.clone());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (log, state) = (log.clone(), state.clone());
                tokio::spawn(async move {
                    let Some(call) = read_call(&mut socket).await else { return };
                    log.lock().unwrap().push(call.clone());
                    let answer = state.lock().unwrap().answer(&call);
                    let body = match &answer.body {
                        Value::Null => Vec::new(),
                        Value::String(text) => text.clone().into_bytes(),
                        other => other.to_string().into_bytes(),
                    };
                    let mut head = format!("HTTP/1.1 {} Stand-in\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", answer.status, body.len());
                    for (name, value) in &answer.headers {
                        head.push_str(&format!("{name}: {value}\r\n"));
                    }
                    head.push_str("Connection: close\r\n\r\n");
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        StandIn { base, calls, fake }
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    /// Trakt's calls only, not the panel's.
    fn trakt_calls(&self) -> Vec<Call> {
        self.calls().into_iter().filter(|call| call.path != "/player_api.php" && call.path != "/xmltv.php").collect()
    }

    fn count(&self, method: &str, path: &str) -> usize {
        self.calls().iter().filter(|call| call.is(method, path)).count()
    }

    fn last(&self, method: &str, path: &str) -> Option<Call> {
        self.calls().into_iter().rev().find(|call| call.is(method, path))
    }

    fn fake(&self) -> std::sync::MutexGuard<'_, Fake> {
        self.fake.lock().unwrap()
    }

    /// Waits (a few seconds at most) for what's sent in the background.
    async fn wait_for(&self, what: &str, done: impl Fn(&[Call]) -> bool) {
        let started = Instant::now();
        while !done(&self.calls()) {
            assert!(started.elapsed() < Duration::from_secs(10), "waited too long for {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

async fn read_call(socket: &mut tokio::net::TcpStream) -> Option<Call> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 8192];
    let head_end = loop {
        if let Some(position) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let count = socket.read(&mut buffer).await.ok().filter(|count| *count > 0)?;
        data.extend_from_slice(&buffer[..count]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).to_string();
    let mut lines = head.lines();
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_string()))
        .collect();
    let length: usize = headers.get("content-length").and_then(|length| length.parse().ok()).unwrap_or(0);
    while data.len() < head_end + length {
        let count = socket.read(&mut buffer).await.ok().filter(|count| *count > 0)?;
        data.extend_from_slice(&buffer[..count]);
    }
    let body = serde_json::from_slice(&data[head_end..head_end + length]).unwrap_or(Value::Null);
    let parsed = url::Url::parse(&format!("http://stand-in{target}")).ok()?;
    Some(Call {
        method,
        path: parsed.path().to_string(),
        query: parsed.query_pairs().map(|(key, value)| (key.into_owned(), value.into_owned())).collect(),
        headers,
        body,
        at: Instant::now(),
    })
}

// MARK: Helpers

type Events = Arc<Mutex<Vec<TraktEvent>>>;

fn open(stand_in: &StandIn, spacing: Duration) -> (tempfile::TempDir, Arc<Core>, Arc<MemorySecrets>, Events) {
    let folder = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let core = Core::open(folder.path(), secrets.clone()).unwrap();
    core.use_trakt_test_server(&stand_in.base, Some(CLIENT_ID), spacing);
    core.set_tvmaze_base(&stand_in.base);
    let events: Events = Arc::default();
    let heard = events.clone();
    core.set_trakt_listener(Arc::new(move |event| heard.lock().unwrap().push(event)));
    (folder, core, secrets, events)
}

async fn add_panel(core: &Core, stand_in: &StandIn) -> SourceConfig {
    let source = core
        .add_source(NewSource {
            name: "Panel".into(),
            kind: SourceKind::Xtream,
            secrets: SourceSecrets {
                server_url: Some(format!("{}/", stand_in.base)),
                username: Some("user".into()),
                password: Some("pass".into()),
                ..Default::default()
            },
            file_path: None,
            live_format: LiveStreamFormat::Hls,
            user_agent: None,
        })
        .await
        .unwrap();
    let status = core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert_eq!(status.last_error, None, "{status:?}");
    source
}

async fn connect(core: &Core) {
    core.trakt_start_sign_in().await.unwrap();
    while !core.trakt_check_sign_in().await.unwrap() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn film_id(core: &Core, title: &str) -> String {
    let films = core.movies(&BrowseQuery::default()).unwrap();
    films.into_iter().find(|film| film.title == title).unwrap_or_else(|| panic!("{title} is in the catalog")).id
}

fn progress(item_id: &str, kind: MediaKind, source_id: &str, position: f64, duration: f64, finished: bool, at: Timestamp) -> PlaybackProgress {
    PlaybackProgress {
        item_id: item_id.into(),
        kind,
        source_id: source_id.into(),
        series_id: None,
        position_seconds: position,
        duration_seconds: duration,
        finished,
        updated_at: at,
    }
}

fn days_ago(days: i64) -> Timestamp {
    now() - days * 86_400
}

// MARK: Tests

#[tokio::test]
async fn connecting_syncing_and_telling_trakt_end_to_end() {
    let stand_in = StandIn::start().await;
    let (folder, core, secrets, events) = open(&stand_in, Duration::from_millis(20));
    let source = add_panel(&core, &stand_in).await;
    let glass = film_id(&core, "The Glass Valley");
    let boats = film_id(&core, "Paper Boats");
    let harbour = film_id(&core, "Harbour Lights");
    let lantern = film_id(&core, "Old Lantern");
    let tidewater = core.series_list(&BrowseQuery::default()).unwrap().pop().unwrap();
    let watched_at = days_ago(30);

    // What Trakt has: two films watched (one older than Xorvio's own place in
    // it), an episode, three places left part-way and a watchlist.
    {
        let mut fake = stand_in.fake();
        let (glass_title, boats_title) = (fake.named(false, 1), fake.named(false, 2));
        fake.watched_movies = vec![
            json!({"plays": 1, "last_watched_at": iso_time(watched_at), "movie": glass_title}),
            json!({"plays": 1, "last_watched_at": iso_time(watched_at), "movie": boats_title}),
        ];
        let show = fake.named(true, 3);
        fake.watched_shows = vec![json!({"plays": 1, "last_watched_at": iso_time(days_ago(20)), "show": show,
            "seasons": [{"number": 1, "episodes": [{"number": 1, "plays": 1, "last_watched_at": iso_time(days_ago(20))}]}]})];
        let mut lantern_title = fake.named(false, 4);
        lantern_title["runtime"] = json!(100);
        let mut absent_title = fake.named(false, 6);
        absent_title["runtime"] = json!(120);
        let far_shore = fake.named(true, 5);
        fake.playback = vec![
            json!({"id": 501, "type": "movie", "progress": 40.0, "paused_at": iso_time(days_ago(2)), "movie": lantern_title}),
            json!({"id": 502, "type": "episode", "progress": 50.0, "paused_at": iso_time(days_ago(3)),
                   "episode": {"season": 2, "number": 3, "title": "Crossing", "runtime": 50}, "show": far_shore}),
            json!({"id": 503, "type": "movie", "progress": 10.0, "paused_at": iso_time(days_ago(4)), "movie": absent_title}),
        ];
        let (lantern_listed, absent_listed) = (fake.named(false, 4), fake.named(false, 6));
        fake.watchlist = vec![
            json!({"rank": 1, "id": 1, "type": "movie", "listed_at": iso_time(days_ago(10)), "movie": lantern_listed}),
            json!({"rank": 2, "id": 2, "type": "movie", "listed_at": iso_time(days_ago(9)), "movie": absent_listed}),
        ];
    }
    // Xorvio's own: further into one film than Trakt's (older) watch, and a
    // start on another long before Trakt's.
    core.save_progress(progress(&glass, MediaKind::Movie, &source.id, 1200.0, 5400.0, false, now() - 100)).unwrap();
    core.save_progress(progress(&boats, MediaKind::Movie, &source.id, 300.0, 5400.0, false, days_ago(60))).unwrap();
    let tidewater_entry = WatchlistEntry {
        id: tidewater.id.clone(),
        kind: MediaKind::Series,
        source_id: source.id.clone(),
        title: "Tidewater".into(),
        subtitle: Some("2025".into()),
        artwork_url: None,
        added_at: days_ago(1),
        channel_id: None,
        start: None,
        end: None,
        remind: false,
    };
    core.set_watchlisted(tidewater_entry, true).unwrap();

    // Not connected yet: configured by the built-in app, nothing sent anywhere.
    let status = core.trakt_status();
    assert!(status.configured && !status.connected && !status.custom_app, "{status:?}");
    assert_eq!(status.options, TraktOptions { scrobble: true, sync_history: true, sync_watchlist: true });
    core.trakt_scrobble(&glass, MediaKind::Movie, TraktScrobbleAction::Start, 1.0).await;
    assert!(core.trakt_sync().await.is_err(), "a sync needs a connection");
    assert!(stand_in.trakt_calls().is_empty(), "{:?}", stand_in.trakt_calls());

    // Connecting with a device code.
    let start = core.trakt_start_sign_in().await.unwrap();
    assert_eq!((start.user_code.as_str(), start.verification_url.as_str(), start.interval), ("5A8B1C2D", "https://trakt.tv/activate", 1));
    assert!(!serde_json::to_string(&start).unwrap().contains(DEVICE_CODE), "the device code stays in Xorvio");
    assert!(!core.trakt_check_sign_in().await.unwrap(), "waiting for the code to be entered");
    assert!(!core.trakt_check_sign_in().await.unwrap());
    assert_eq!(stand_in.count("POST", "/oauth/device/token"), 1, "not asked again before the interval");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(core.trakt_check_sign_in().await.unwrap());
    let status = core.trakt_status();
    assert!(status.connected && status.username.as_deref() == Some("walter"), "{status:?}");
    assert!(events.lock().unwrap().iter().any(|event| matches!(event, TraktEvent::Status(status) if status.connected)));
    let stored = secrets.get("trakt:connection").unwrap().unwrap();
    assert!(stored.contains("access-token-1") && stored.contains("refresh-token-1"), "the tokens are in the keychain");

    // The first sync takes in what Trakt says.
    let status = core.trakt_sync().await.unwrap();
    assert_eq!(status.last_error, None, "{status:?}");
    assert!(status.last_sync.is_some() && !status.syncing);
    assert!(events.lock().unwrap().contains(&TraktEvent::LibraryChanged));
    let glass_progress = core.progress(&glass).unwrap().unwrap();
    assert!(!glass_progress.finished && glass_progress.position_seconds == 1200.0, "Xorvio's place is newer than Trakt's watch");
    let boats_progress = core.progress(&boats).unwrap().unwrap();
    assert!(boats_progress.finished, "watched on Trakt after it was started here; matched by title and year");
    assert_eq!(boats_progress.updated_at, watched_at, "as of Trakt's watch");
    let lantern_progress = core.progress(&lantern).unwrap().unwrap();
    assert_eq!((lantern_progress.position_seconds, lantern_progress.duration_seconds), (2400.0, 6000.0), "40% of Trakt's 100 minutes");
    assert!(!lantern_progress.finished);
    let far_shore = core.progress("addon|series|tt0000501|ep|tt0000501:2:3").unwrap().unwrap();
    assert_eq!((far_shore.position_seconds, far_shore.source_id.as_str()), (1500.0, "addon"));
    assert_eq!(far_shore.series_id.as_deref(), Some("addon|series|tt0000501"));
    assert_eq!(core.progress("addon|movie|tt0000601").unwrap().map(|p| p.position_seconds), Some(720.0));
    assert!(core.progress("addon|movie|tt0000101").unwrap().is_none(), "watched films aren't made add-on progress");
    assert!(core.addon_progress(10).unwrap().iter().all(|entry| !entry.finished));

    // The watchlists came together: Trakt's two here (one as the catalog's
    // film, one as an add-on title), and Tidewater sent to Trakt.
    let watchlist = core.watchlist().unwrap();
    let saved: HashSet<&str> = watchlist.iter().map(|entry| entry.id.as_str()).collect();
    assert!(saved.contains(lantern.as_str()) && saved.contains("addon|movie|tt0000601") && saved.contains(tidewater.id.as_str()), "{saved:?}");
    let absent = watchlist.iter().find(|entry| entry.id == "addon|movie|tt0000601").unwrap();
    assert_eq!((absent.title.as_str(), absent.subtitle.as_deref(), absent.source_id.as_str()), ("Absent", Some("2022"), "addon"));
    let sent = stand_in.last("POST", "/sync/watchlist").unwrap();
    assert_eq!(sent.body, json!({"movies": [], "shows": [{"ids": {"tmdb": 1396}}]}));
    assert_eq!(stand_in.count("POST", "/sync/watchlist/remove"), 0, "nothing is removed on a first sync");

    // A second sync with nothing new pulls nothing and changes nothing.
    let pulls = stand_in.count("GET", "/sync/watched/movies");
    events.lock().unwrap().clear();
    core.trakt_sync().await.unwrap();
    core.trakt_sync().await.unwrap();
    assert_eq!(stand_in.count("GET", "/sync/watched/movies"), pulls, "last_activities said nothing changed");
    assert!(!events.lock().unwrap().contains(&TraktEvent::LibraryChanged));

    // Opening the series fetches its episodes, which pick up Trakt's watched mark.
    let episodes = core.episodes(&tidewater.id).await.unwrap();
    assert_eq!(episodes.len(), 2);
    assert!(core.progress(&episodes[0].id).unwrap().is_some_and(|p| p.finished && p.series_id.as_deref() == Some(tidewater.id.as_str())));
    assert!(core.progress(&episodes[1].id).unwrap().is_none());
    let up_next = core.continue_watching(10).unwrap();
    assert!(up_next.iter().any(|item| item.item_id == episodes[1].id && item.up_next), "{up_next:?}");

    // Marking a film watched by hand: looked up by title and year, then sent.
    core.set_watched(&[progress(&harbour, MediaKind::Movie, &source.id, 0.0, 0.0, false, 0)], true).unwrap();
    stand_in.wait_for("the history", |calls| calls.iter().any(|call| call.is("POST", "/sync/history"))).await;
    let search = stand_in.last("GET", "/search/movie").unwrap();
    assert_eq!(search.query.get("query").map(String::as_str), Some("Harbour Lights"));
    assert_eq!(search.query.get("years").map(String::as_str), Some("2019"));
    let history = stand_in.last("POST", "/sync/history").unwrap().body;
    assert_eq!(history["movies"][0]["ids"], json!({"trakt": 7, "imdb": "tt0000701"}), "the ids the lookup found");
    assert!(history["movies"][0]["watched_at"].as_str().is_some_and(|at| at.ends_with('Z')));
    assert_eq!(history["shows"], json!([]));

    // Scrobbling: the film (its lookup kept, so Trakt isn't searched again)
    // and an episode (by its own ids, from the show's).
    core.trakt_scrobble(&harbour, MediaKind::Movie, TraktScrobbleAction::Start, 5.0).await;
    assert_eq!(
        stand_in.last("POST", "/scrobble/start").unwrap().body,
        json!({"movie": {"ids": {"trakt": 7, "imdb": "tt0000701"}}, "progress": 5.0})
    );
    assert_eq!(stand_in.count("GET", "/search/movie"), 1, "looked up once");
    core.trakt_scrobble(&episodes[1].id, MediaKind::Episode, TraktScrobbleAction::Pause, 30.0).await;
    assert_eq!(stand_in.last("POST", "/scrobble/pause").unwrap().body, json!({"episode": {"ids": {"trakt": 9102}}, "progress": 30.0}));
    core.trakt_scrobble("addon|movie|tt0000601", MediaKind::Movie, TraktScrobbleAction::Stop, 85.0).await;
    assert_eq!(stand_in.last("POST", "/scrobble/stop").unwrap().body, json!({"movie": {"ids": {"imdb": "tt0000601"}}, "progress": 85.0}));
    core.trakt_scrobble("demo|mv|1", MediaKind::Movie, TraktScrobbleAction::Start, 1.0).await;
    core.trakt_scrobble(&tidewater.id, MediaKind::Series, TraktScrobbleAction::Start, 1.0).await;
    core.trakt_set_options(TraktOptions { scrobble: false, ..TraktOptions::default() }).unwrap();
    core.trakt_scrobble(&harbour, MediaKind::Movie, TraktScrobbleAction::Stop, 90.0).await;
    core.trakt_set_options(TraktOptions::default()).unwrap();
    let scrobbles = stand_in.calls().iter().filter(|call| call.path.starts_with("/scrobble/")).count();
    assert_eq!(scrobbles, 3, "nothing for the demo library, a series, or with scrobbling off");

    // The watchlist, from here: added and removed.
    let harbour_entry = WatchlistEntry {
        id: harbour.clone(),
        kind: MediaKind::Movie,
        source_id: source.id.clone(),
        title: "Harbour Lights".into(),
        subtitle: Some("2019".into()),
        artwork_url: None,
        added_at: 0,
        channel_id: None,
        start: None,
        end: None,
        remind: false,
    };
    core.set_watchlisted(harbour_entry, true).unwrap();
    stand_in.wait_for("the watchlist", |calls| calls.iter().filter(|call| call.is("POST", "/sync/watchlist")).count() == 2).await;
    assert_eq!(
        stand_in.last("POST", "/sync/watchlist").unwrap().body,
        json!({"movies": [{"ids": {"trakt": 7, "imdb": "tt0000701"}}], "shows": []})
    );
    let lantern_entry = core.watchlist().unwrap().into_iter().find(|entry| entry.id == lantern).unwrap();
    core.set_watchlisted(lantern_entry, false).unwrap();
    stand_in.wait_for("the removal", |calls| calls.iter().any(|call| call.is("POST", "/sync/watchlist/remove"))).await;
    assert_eq!(stand_in.last("POST", "/sync/watchlist/remove").unwrap().body, json!({"movies": [{"ids": {"tmdb": 777}}], "shows": []}));

    // Taking a card off Continue Watching forgets Trakt's place too.
    core.remove_from_continue_watching(&lantern).unwrap();
    stand_in.wait_for("the place", |calls| calls.iter().any(|call| call.is("DELETE", "/sync/playback/501"))).await;
    assert!(core.progress(&lantern).unwrap().is_none());

    // Unwatched by hand: Trakt's history loses it, and no sync brings it back.
    core.set_watched(&[progress(&boats, MediaKind::Movie, &source.id, 0.0, 0.0, false, 0)], false).unwrap();
    stand_in.wait_for("the history removal", |calls| calls.iter().any(|call| call.is("POST", "/sync/history/remove"))).await;
    assert_eq!(stand_in.last("POST", "/sync/history/remove").unwrap().body["movies"], json!([{"ids": {"trakt": 2}}]));

    // Meanwhile on Trakt: one film off the watchlist, a show on it.
    {
        let mut fake = stand_in.fake();
        fake.watchlist.retain(|item| item["movie"]["ids"]["trakt"] != 6);
        let show = fake.named(true, 5);
        fake.watchlist.push(json!({"type": "show", "listed_at": iso_time(now()), "show": show}));
        fake.versions[3] += 1;
    }
    let writes_before = stand_in.trakt_calls().iter().filter(|call| call.method != "GET").count();
    let status = core.trakt_sync().await.unwrap();
    assert_eq!(status.last_error, None, "{status:?}");
    let saved: HashSet<String> = core.watchlist().unwrap().into_iter().map(|entry| entry.id).collect();
    let expected: HashSet<String> = [tidewater.id.clone(), harbour.clone(), "addon|series|tt0000501".to_string()].into();
    assert_eq!(saved, expected, "removed on Trakt goes here; added on Trakt comes here; Xorvio's own changes stay");
    assert_eq!(stand_in.trakt_calls().iter().filter(|call| call.method != "GET").count(), writes_before, "both sides already agreed");
    assert!(core.progress(&boats).unwrap().is_none(), "still unwatched");
    assert!(core.progress(&lantern).unwrap().is_none(), "its place stays forgotten");

    // Every request carried the app's key and Trakt's API version; nothing
    // secret reached the databases or the status.
    for call in stand_in.trakt_calls() {
        assert_eq!(call.headers.get("trakt-api-key").map(String::as_str), Some(CLIENT_ID), "{call:?}");
        assert_eq!(call.headers.get("trakt-api-version").map(String::as_str), Some("2"));
    }
    let status_json = serde_json::to_string(&core.trakt_status()).unwrap();
    for secret in ["access-token-1", "refresh-token-1", DEVICE_CODE] {
        assert!(!status_json.contains(secret));
        for file in ["library.sqlite", "library.sqlite-wal", "catalog.sqlite", "catalog.sqlite-wal", "sources.json"] {
            let bytes = std::fs::read(folder.path().join(file)).unwrap_or_default();
            assert!(!String::from_utf8_lossy(&bytes).contains(secret), "{secret} in {file}");
        }
    }

    // Disconnecting revokes the token and forgets it.
    core.trakt_disconnect().await.unwrap();
    assert_eq!(stand_in.fake().revoked, ["access-token-1"]);
    let status = core.trakt_status();
    assert!(!status.connected && status.username.is_none() && status.last_sync.is_none(), "{status:?}");
    assert!(secrets.get("trakt:connection").unwrap().is_none());
    assert!(core.watchlist().unwrap().iter().any(|entry| entry.id == harbour), "Xorvio's own watchlist stays");
}

#[tokio::test]
async fn tokens_are_refreshed_before_they_expire_and_after_a_rejection() {
    let stand_in = StandIn::start().await;
    let (_folder, core, secrets, events) = open(&stand_in, Duration::from_millis(1));
    stand_in.fake().pending_checks = 0;
    // A token that's nearly done at once; the one it's swapped for lasts.
    stand_in.fake().lifetime = 30;
    connect(&core).await;
    stand_in.fake().lifetime = 7 * 86_400;
    core.trakt_sync().await.unwrap();
    assert_eq!(stand_in.count("POST", "/oauth/token"), 1, "refreshed before its first use");
    let refresh = stand_in.last("POST", "/oauth/token").unwrap().body;
    assert_eq!(refresh["refresh_token"], "refresh-token-1");
    assert_eq!(refresh["redirect_uri"], "http://localhost");
    let activity = stand_in.last("GET", "/sync/last_activities").unwrap();
    assert_eq!(activity.headers.get("authorization").map(String::as_str), Some("Bearer access-token-2"));
    assert!(secrets.get("trakt:connection").unwrap().unwrap().contains("refresh-token-2"), "the new refresh token is kept");

    // A long-lived token Trakt turns down anyway is refreshed once, and the request tried again.
    stand_in.fake().rejected.insert("access-token-2".into());
    let status = core.trakt_sync().await.unwrap();
    assert_eq!(status.last_error, None, "{status:?}");
    assert_eq!(stand_in.count("POST", "/oauth/token"), 2);
    assert_eq!(
        stand_in.last("GET", "/sync/last_activities").unwrap().headers.get("authorization").map(String::as_str),
        Some("Bearer access-token-3")
    );

    // When Trakt won't refresh it either, Xorvio is signed out and says so.
    {
        let mut fake = stand_in.fake();
        fake.rejected.insert("access-token-3".into());
        fake.refuse_refresh = true;
    }
    let status = core.trakt_sync().await.unwrap();
    assert!(!status.connected, "{status:?}");
    assert_eq!(status.last_error.as_deref(), Some("Trakt signed Xorvio out. Connect again."));
    assert!(secrets.get("trakt:connection").unwrap().is_none());
    let last = events.lock().unwrap().iter().rev().find_map(|event| match event {
        TraktEvent::Status(status) => Some(status.clone()),
        TraktEvent::LibraryChanged => None,
    });
    assert!(last.is_some_and(|status| !status.connected && !status.syncing));
    // Scrobbling now does nothing, quietly.
    let before = stand_in.calls().len();
    core.trakt_scrobble("addon|movie|tt0000601", MediaKind::Movie, TraktScrobbleAction::Start, 2.0).await;
    assert_eq!(stand_in.calls().len(), before);
}

#[tokio::test]
async fn rate_limits_are_respected() {
    let stand_in = StandIn::start().await;
    let (_folder, core, _, _) = open(&stand_in, Duration::from_millis(300));
    stand_in.fake().pending_checks = 0;
    connect(&core).await;
    stand_in.fake().limit_once.insert("/sync/last_activities".into());
    let started = Instant::now();
    let status = core.trakt_sync().await.unwrap();
    assert_eq!(status.last_error, None, "{status:?}");
    assert!(started.elapsed() >= Duration::from_secs(1), "waited as Retry-After said");
    assert_eq!(stand_in.count("GET", "/sync/last_activities"), 2);

    // Writes are spaced out.
    for (index, action) in [TraktScrobbleAction::Start, TraktScrobbleAction::Pause, TraktScrobbleAction::Stop].into_iter().enumerate() {
        core.trakt_scrobble("addon|movie|tt0000601", MediaKind::Movie, action, 10.0 + index as f64 * 30.0).await;
    }
    let writes: Vec<Instant> = stand_in.calls().iter().filter(|call| call.path.starts_with("/scrobble/")).map(|call| call.at).collect();
    assert_eq!(writes.len(), 3);
    for pair in writes.windows(2) {
        assert!(pair[1] - pair[0] >= Duration::from_millis(280), "{:?}", pair[1] - pair[0]);
    }
}

#[tokio::test]
async fn connecting_needs_an_app_and_a_code_that_works() {
    let stand_in = StandIn::start().await;
    let folder = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let core = Core::open(folder.path(), secrets.clone()).unwrap();
    core.use_trakt_test_server(&stand_in.base, None, Duration::from_millis(1));
    let status = core.trakt_status();
    assert!(!status.configured && !status.connected);
    let error = core.trakt_start_sign_in().await.unwrap_err().to_string();
    assert!(error.contains("Settings › Trakt"), "{error}");

    // An app entered in Settings.
    core.trakt_set_app(&format!(" {CLIENT_ID} "), "").await.unwrap();
    let status = core.trakt_status();
    assert!(status.configured && status.custom_app);
    let stored = secrets.get("trakt:app").unwrap().unwrap();
    assert!(stored.contains(CLIENT_ID) && stored.contains("http://localhost"), "kept in the keychain: {stored}");
    // An app saved by an earlier version, with a client secret, still reads (the secret unused).
    let earlier: super::api::App = serde_json::from_str(r#"{"client_id":"abc","client_secret":"old-secret"}"#).unwrap();
    assert_eq!((earlier.client_id.as_str(), earlier.redirect_uri.as_str()), ("abc", "http://localhost"));

    let error = core.trakt_check_sign_in().await.unwrap_err().to_string();
    assert_eq!(error, "Start connecting to Trakt again.");
    connect(&core).await;
    assert!(core.trakt_status().connected);
    assert!(core.trakt_check_sign_in().await.unwrap(), "asking again once connected is harmless");

    // Going back to no app disconnects: the tokens belonged to that app.
    core.trakt_set_app("", "").await.unwrap();
    let status = core.trakt_status();
    assert!(!status.configured && !status.connected && !status.custom_app);
    assert_eq!(stand_in.fake().revoked, ["access-token-1"]);
    assert!(secrets.get("trakt:app").unwrap().is_none());

    // Codes Trakt turned down, or that ran out.
    core.trakt_set_app(CLIENT_ID, "").await.unwrap();
    for (status, message) in [
        (418, "Xorvio wasn't allowed on Trakt. Start connecting again if that was a mistake."),
        (410, "The code has expired. Start connecting again for a new one."),
        (404, "Trakt didn't recognise the code. Start connecting again."),
    ] {
        core.trakt_start_sign_in().await.unwrap();
        stand_in.fake().device_answer = Some(status);
        assert_eq!(core.trakt_check_sign_in().await.unwrap_err().to_string(), message);
        assert_eq!(core.trakt_check_sign_in().await.unwrap_err().to_string(), "Start connecting to Trakt again.", "the code is gone");
    }
    assert!(!core.trakt_status().connected);
}

/// The model's JSON is what src/lib/types.ts expects.
#[test]
fn the_interface_contract() {
    let status = TraktStatus {
        configured: true,
        custom_app: false,
        connected: true,
        username: Some("walter".into()),
        last_sync: Some(5),
        last_error: None,
        syncing: false,
        options: TraktOptions::default(),
    };
    assert_eq!(
        serde_json::to_value(&status).unwrap(),
        json!({"configured": true, "customApp": false, "connected": true, "username": "walter", "lastSync": 5,
               "lastError": null, "syncing": false, "options": {"scrobble": true, "syncHistory": true, "syncWatchlist": true}})
    );
    assert_eq!(serde_json::to_value(TraktScrobbleAction::Pause).unwrap(), json!("pause"));
    assert_eq!(serde_json::from_value::<TraktScrobbleAction>(json!("stop")).unwrap(), TraktScrobbleAction::Stop);
    let start = TraktSignInStart { user_code: "C".into(), verification_url: "u".into(), expires_in: 600, interval: 5 };
    assert_eq!(serde_json::to_value(&start).unwrap(), json!({"userCode": "C", "verificationUrl": "u", "expiresIn": 600, "interval": 5}));
    let partial: TraktOptions = serde_json::from_value(json!({"scrobble": false})).unwrap();
    assert_eq!(partial, TraktOptions { scrobble: false, sync_history: true, sync_watchlist: true });
}

// MARK: Merging, on its own

fn title(show: bool, trakt: i64, imdb: Option<&str>, tmdb: Option<i64>, name: &str, year: i64) -> TraktTitle {
    TraktTitle {
        show,
        ids: TraktIds { trakt: Some(trakt), imdb: imdb.map(str::to_string), tmdb },
        title_key: crate::store::catalog::title_key(name),
        year: Some(year),
    }
}

fn place(id: i64, title: TraktTitle, season: i64, number: i64, percent: f64, runtime: Option<i64>, at: Timestamp) -> TraktPaused {
    TraktPaused { playback_id: id, title, season, number, progress: percent, runtime_seconds: runtime, paused_at: at }
}

#[tokio::test]
async fn what_trakt_says_merges_with_progress_here() {
    let stand_in = StandIn::start().await;
    let (_folder, core, _, _) = open(&stand_in, Duration::from_millis(1));
    let source = add_panel(&core, &stand_in).await;
    let (glass, boats, harbour, lantern) =
        (film_id(&core, "The Glass Valley"), film_id(&core, "Paper Boats"), film_id(&core, "Harbour Lights"), film_id(&core, "Old Lantern"));
    let library = &core.library;
    let (one, two, three, five, ten, twenty, thirty, forty) =
        (days_ago(1), days_ago(2), days_ago(3), days_ago(5), days_ago(10), days_ago(20), days_ago(30), days_ago(40));
    let glass_title = title(false, 1, Some("tt0000101"), Some(550), "The Glass Valley", 2018);
    let boats_title = title(false, 2, None, None, "Paper Boats", 2021);
    let harbour_title = title(false, 7, Some("tt0000701"), None, "Harbour Lights", 2019);
    library
        .replace_trakt_watched(
            false,
            &[
                TraktWatched { title: glass_title, season: 0, number: 0, watched_at: ten },
                TraktWatched { title: boats_title, season: 0, number: 0, watched_at: ten },
                TraktWatched { title: harbour_title, season: 0, number: 0, watched_at: ten },
            ],
        )
        .unwrap();
    library
        .replace_trakt_paused(&[
            // Older than where Xorvio is: kept as it is.
            place(1, title(false, 4, None, Some(777), "Old Lantern", 2010), 0, 0, 75.0, Some(6000), thirty),
            // An episode of a show not in the catalog: an add-on title, by its IMDb id.
            place(2, title(true, 5, Some("tt0000501"), None, "Far Shore", 2020), 1, 4, 25.0, Some(3000), one),
            // No length known anywhere: nothing to place.
            place(3, title(false, 8, Some("tt0000801"), None, "Unknown Length", 2001), 0, 0, 50.0, None, one),
        ])
        .unwrap();
    // Here: Glass Valley started after Trakt's watch; Paper Boats started
    // before it; Harbour Lights already watched; Old Lantern further on since.
    core.save_progress(progress(&glass, MediaKind::Movie, &source.id, 600.0, 5400.0, false, five)).unwrap();
    core.save_progress(progress(&boats, MediaKind::Movie, &source.id, 600.0, 5400.0, false, twenty)).unwrap();
    core.save_progress(progress(&harbour, MediaKind::Movie, &source.id, 5400.0, 5400.0, true, forty)).unwrap();
    core.save_progress(progress(&lantern, MediaKind::Movie, &source.id, 5000.0, 6000.0, false, two)).unwrap();
    // Watched here, unknown to Trakt: never removed.
    core.save_progress(progress("addon|movie|tt0000999", MediaKind::Movie, "addon", 100.0, 100.0, true, three)).unwrap();

    let changed = core.apply_trakt_history().unwrap();
    assert_eq!(changed, 2, "Paper Boats finished, and the add-on episode placed");
    assert!(!core.progress(&glass).unwrap().unwrap().finished, "started here after Trakt's watch");
    let boats_now = core.progress(&boats).unwrap().unwrap();
    assert!(boats_now.finished && boats_now.updated_at == ten, "watched on Trakt since it was started here");
    let harbour_now = core.progress(&harbour).unwrap().unwrap();
    assert_eq!(harbour_now.updated_at, forty, "already watched here");
    assert_eq!(core.progress(&lantern).unwrap().unwrap().position_seconds, 5000.0, "Xorvio's place is newer");
    let episode = core.progress("addon|series|tt0000501|ep|tt0000501:1:4").unwrap().unwrap();
    assert_eq!((episode.position_seconds, episode.duration_seconds, episode.finished), (750.0, 3000.0, false));
    assert_eq!(episode.series_id.as_deref(), Some("addon|series|tt0000501"));
    assert!(core.progress("addon|movie|tt0000801").unwrap().is_none());
    assert!(core.progress("addon|movie|tt0000999").unwrap().is_some_and(|p| p.finished));
    assert_eq!(core.apply_trakt_history().unwrap(), 0, "applying again changes nothing");

    // A newer place on Trakt moves Xorvio's along.
    library.replace_trakt_paused(&[place(1, title(false, 4, None, Some(777), "Old Lantern", 2010), 0, 0, 90.0, Some(6000), one)]).unwrap();
    assert_eq!(core.apply_trakt_history().unwrap(), 1);
    assert_eq!(core.progress(&lantern).unwrap().unwrap().position_seconds, 5400.0);
}

#[tokio::test]
async fn charts_and_lists_become_rows_of_what_can_be_played() {
    let stand_in = StandIn::start().await;
    let (_folder, core, _secrets, _events) = open(&stand_in, Duration::from_millis(20));
    add_panel(&core, &stand_in).await;
    let glass = film_id(&core, "The Glass Valley");
    let harbour = film_id(&core, "Harbour Lights");
    let lantern = film_id(&core, "Old Lantern");
    let tidewater = core.series_list(&BrowseQuery::default()).unwrap().pop().unwrap();
    {
        let mut fake = stand_in.fake();
        let mut absent = fake.named(false, 6);
        absent["images"] = json!({"poster": ["media.trakt.tv/images/movies/000/000/006/posters/medium/absent.jpg.webp"]});
        let chart = json!([
            {"watchers": 90, "movie": absent},
            {"watchers": 80, "movie": fake.named(false, 1)},
            {"watchers": 70, "movie": fake.named(false, 7)},
            {"watchers": 60, "movie": fake.named(false, 4)}
        ]);
        fake.public.insert("/movies/trending".into(), chart);
        let list = json!([
            {"rank": 1, "type": "show", "show": fake.named(true, 3)},
            {"rank": 2, "type": "episode", "episode": {"season": 1, "number": 1, "ids": {"trakt": 9101}}, "show": fake.named(true, 3)},
            {"rank": 3, "type": "movie", "movie": fake.named(false, 4)},
            {"rank": 4, "type": "show", "show": fake.named(true, 5)}
        ]);
        fake.public.insert("/users/justin/lists/2142753/items/movie,show".into(), list);
        fake.public.insert(
            "/users/justin/lists/imdb-top-rated-movies".into(),
            json!({"name": "IMDb Top 250", "item_count": 250, "likes": 1200, "ids": {"trakt": 2142753, "slug": "imdb-top-rated-movies"},
                   "user": {"username": "justin", "name": "Justin N.", "ids": {"slug": "justin"}}}),
        );
    }
    let ids = |row: &TraktRow| -> Vec<String> {
        row.items
            .iter()
            .map(|item| match &item.title {
                TraktRowTitle::Movie(movie) => movie.id.clone(),
                TraktRowTitle::Series(series) => series.id.clone(),
            })
            .collect()
    };
    let trending = TraktRowSource::Chart { chart: TraktChart::Trending, media: TraktMedia::Movies };

    // Nobody has connected an account or asked for Trakt's rows without
    // one, so Trakt isn't asked anything.
    let row = core.trakt_row(&trending, 10, AddonReach::default()).await.unwrap();
    assert!(row.items.is_empty() && row.listed == 0);
    assert_eq!(stand_in.count("GET", "/movies/trending"), 0, "nothing goes to Trakt unasked");
    core.set_trakt_rows_without_account(true);

    // Without add-ons, only what the sources carry, in Trakt's order: by
    // TMDB id, and by title and year for a film with no ids.
    let row = core.trakt_row(&trending, 10, AddonReach::default()).await.unwrap();
    assert_eq!(ids(&row), [glass.clone(), harbour.clone(), lantern.clone()]);
    assert_eq!(row.items.iter().map(|item| item.rank).collect::<Vec<_>>(), [2, 3, 4], "Trakt's own places");
    assert_eq!(row.listed, 4);
    // With add-ons that can play films, Trakt's own title stands in for one the sources lack.
    let row = core.trakt_row(&trending, 10, AddonReach { movies: true, series: false, lead: false }).await.unwrap();
    assert_eq!(ids(&row), ["addon|movie|tt0000601".to_string(), glass.clone(), harbour.clone(), lantern.clone()]);
    let TraktRowTitle::Movie(absent) = &row.items[0].title else { panic!("a film") };
    assert_eq!((absent.title.as_str(), absent.source_id.as_str()), ("Absent", "addon"));
    assert_eq!(absent.poster_url.as_deref(), Some("https://media.trakt.tv/images/movies/000/000/006/posters/medium/absent.jpg.webp"));
    assert_eq!(core.trakt_row(&trending, 2, AddonReach::default()).await.unwrap().items.len(), 2);
    assert_eq!(stand_in.count("GET", "/movies/trending"), 1, "the chart is kept for a while");
    let asked = stand_in.last("GET", "/movies/trending").unwrap();
    assert_eq!(asked.headers.get("authorization"), None, "charts need no account");
    assert_eq!(asked.query.get("extended").map(String::as_str), Some("full,images"));

    // When add-ons lead, their title comes first even where the sources carry
    // one (its page lists the sources' copies too); a film with no IMDb id
    // for add-ons to find stays the sources'.
    let leading = AddonReach { movies: true, series: false, lead: true };
    let row = core.trakt_row(&trending, 10, leading).await.unwrap();
    assert_eq!(ids(&row), ["addon|movie|tt0000601", "addon|movie|tt0000101", "addon|movie|tt0000701", lantern.as_str()]);

    // A film hidden in the sources stays off the row: no add-on title stands in for it.
    core.set_hidden(&glass, true).unwrap();
    let row = core.trakt_row(&trending, 10, AddonReach { movies: true, series: false, lead: false }).await.unwrap();
    assert_eq!(ids(&row), ["addon|movie|tt0000601".to_string(), harbour.clone(), lantern.clone()]);
    let row = core.trakt_row(&trending, 10, leading).await.unwrap();
    assert_eq!(ids(&row), ["addon|movie|tt0000601", "addon|movie|tt0000701", lantern.as_str()], "nor when add-ons lead");
    core.set_hidden(&glass, false).unwrap();

    // A list, found from its trakt.tv address, keeps films and series in its order.
    let found = core
        .trakt_lists(&TraktListQuery::Search { text: "https://trakt.tv/users/justin/lists/imdb-top-rated-movies?sort=rank,asc".into() })
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].user.as_str(), found[0].list.as_str(), found[0].name.as_str()), ("justin", "2142753", "IMDb Top 250"));
    let list = TraktRowSource::List { user: found[0].user.clone(), list: found[0].list.clone() };
    let row = core.trakt_row(&list, 40, AddonReach { movies: true, series: true, lead: false }).await.unwrap();
    assert_eq!(ids(&row), [tidewater.id.clone(), lantern.clone(), "addon|series|tt0000501".to_string()], "the episode is passed over");
    // Places count the films and series Trakt listed, the episode not among them.
    assert_eq!(row.items.iter().map(|item| item.rank).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(row.listed, 3);

    // An account's own lists and recommendations need a connection.
    assert!(core.trakt_lists(&TraktListQuery::Mine).await.is_err());
    assert!(core.trakt_row(&TraktRowSource::Recommended { media: TraktMedia::Movies }, 10, AddonReach::default()).await.is_err());
}
