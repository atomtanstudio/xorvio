//! Xorvio against real Jellyfin and Emby servers, through its public API:
//! signing in, a refresh, details, playing, and what's been watched going
//! both ways. Ignored unless asked for, since it needs the servers:
//!
//! ```text
//! Scripts/test-servers/jellyfin-emby.sh      # starts both, prints these
//! export XORVIO_TEST_JELLYFIN=http://localhost:18096
//! export XORVIO_TEST_EMBY=http://localhost:18097
//! export XORVIO_TEST_SERVER_USER=… XORVIO_TEST_SERVER_PASSWORD=…
//! cargo test -p xorvio-core --test media_servers_docker -- --ignored --nocapture
//! ```
//!
//! A server whose address isn't set is skipped. Everything Xorvio keeps goes
//! to a temporary folder and [`MemorySecrets`]; the servers hold only the
//! script's test media, and the watch state this changes there.

use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use xorvio_core::{BrowseQuery, Core, MemorySecrets, SecretStore, parse_artwork_path};
use xorvio_model::{
    CategoryKind, Episode, MediaKind, Movie, NewSource, PlaybackProgress, Series, SourceKind, SourceSecrets,
    StreamLocator, StreamTarget,
};

#[tokio::test]
#[ignore = "needs the Docker test servers (Scripts/test-servers/jellyfin-emby.sh)"]
async fn jellyfin_through_xorvio() {
    run(SourceKind::Jellyfin, "XORVIO_TEST_JELLYFIN").await;
}

#[tokio::test]
#[ignore = "needs the Docker test servers (Scripts/test-servers/jellyfin-emby.sh)"]
async fn emby_through_xorvio() {
    run(SourceKind::Emby, "XORVIO_TEST_EMBY").await;
}

/// The test servers' login, from the environment.
struct Login {
    address: String,
    username: String,
    password: String,
}

fn login(variable: &str) -> Option<Login> {
    let value = |name: &str| std::env::var(name).ok().filter(|value| !value.trim().is_empty());
    let Some(address) = value(variable) else {
        eprintln!("{variable} isn't set; skipped (Scripts/test-servers/jellyfin-emby.sh starts the servers)");
        return None;
    };
    let (Some(username), Some(password)) = (value("XORVIO_TEST_SERVER_USER"), value("XORVIO_TEST_SERVER_PASSWORD")) else {
        eprintln!("XORVIO_TEST_SERVER_USER and XORVIO_TEST_SERVER_PASSWORD aren't set; skipped");
        return None;
    };
    Some(Login { address, username, password })
}

fn new_source(kind: SourceKind, address: &str, username: &str, password: &str) -> NewSource {
    NewSource {
        name: String::new(),
        kind,
        secrets: SourceSecrets {
            server_url: Some(address.into()),
            username: Some(username.into()),
            password: Some(password.into()),
            ..Default::default()
        },
        file_path: None,
        live_format: Default::default(),
        user_agent: None,
    }
}

async fn run(kind: SourceKind, variable: &str) {
    let Some(login) = login(variable) else { return };
    let folder = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let core = Core::open(folder.path(), secrets.clone()).unwrap();
    let other = if kind == SourceKind::Jellyfin { SourceKind::Emby } else { SourceKind::Jellyfin };

    // MARK: Signing in

    let wrong = core.add_source(new_source(kind, &login.address, &login.username, "not the password")).await.unwrap_err();
    assert_eq!(wrong.to_string(), "The provider rejected the username or password.");
    let mismatch = core.add_source(new_source(other, &login.address, &login.username, &login.password)).await.unwrap_err();
    assert!(mismatch.to_string().contains("Add it as"), "the other server is named: {mismatch}");
    let nothing = core.add_source(new_source(kind, "http://127.0.0.1:9", &login.username, &login.password)).await.unwrap_err();
    assert!(nothing.to_string().contains("not accepting connections"), "{nothing}");

    // Typed without a scheme, from the web app's address bar.
    let typed = format!("{}/web/index.html", login.address.trim_start_matches("http://"));
    let config = core.add_source(new_source(kind, &typed, &login.username, &login.password)).await.unwrap();
    assert!(!config.name.is_empty(), "named after the server");
    let stored: SourceSecrets = serde_json::from_str(&secrets.get(&config.id).unwrap().unwrap()).unwrap();
    assert_eq!(stored.server_url.as_deref(), Some(login.address.trim_end_matches('/')));
    assert_eq!(stored.password, None, "the keychain never keeps the password");
    assert_eq!(stored.username.as_deref(), Some(login.username.as_str()));
    let token = stored.token.clone().expect("the keychain keeps the token");
    let user_id = stored.user_id.clone().expect("and the user's id");
    let shown = core.source_secrets(&config.id).unwrap();
    assert_eq!((shown.token, shown.user_id, shown.password), (None, None, None), "the interface never sees the token");
    let server = ServerApi { kind, base: login.address.trim_end_matches('/').to_string(), token: token.clone(), user_id };

    // A fresh start for the titles whose watch state this changes.
    let film_two_key = server.find("Movie", "Test Film Two").await;
    let episode_keys = [server.find("Episode", "Broadcast 1.2").await, server.find("Episode", "Broadcast 2.1").await];
    for key in [&film_two_key, &episode_keys[0], &episode_keys[1]] {
        server.set_played(key, false).await;
    }

    // MARK: The catalog

    let status = core.refresh_source(&config.id, &|_| {}).await.unwrap();
    assert_eq!(status.last_error, None);
    assert_eq!(status.catalog_warning, None);
    assert_eq!((status.movie_count, status.series_count), (2, 1));
    assert!(status.capabilities.movies && status.capabilities.series && !status.capabilities.live);
    let film_categories = core.categories(CategoryKind::Movie).unwrap();
    let show_categories = core.categories(CategoryKind::Series).unwrap();
    assert_eq!(film_categories.iter().map(|category| category.name.as_str()).collect::<Vec<_>>(), ["Films"]);
    assert_eq!(show_categories.iter().map(|category| category.name.as_str()).collect::<Vec<_>>(), ["Shows"]);

    let movies = core.movies(&BrowseQuery::default()).unwrap();
    let film_one = movie(&movies, "Test Film One");
    let film_two = movie(&movies, "Test Film Two");
    assert_eq!((film_one.year, film_one.rating, film_one.duration_seconds), (Some(2020), Some(7.4), Some(360)));
    assert_eq!(film_one.genre.as_deref(), Some("Science Fiction, Drama"));
    assert_eq!((film_one.tmdb_id.as_deref(), film_one.imdb_id.as_deref()), (Some("990001"), Some("tt9900001")));
    assert_eq!(film_one.plot.as_deref(), Some("A surveyor maps a canyon that keeps changing shape."));
    assert_eq!(film_one.category_id.as_deref(), Some(film_categories[0].id.as_str()));
    assert!(film_one.added_at.is_some_and(|added| added > 1_700_000_000));
    assert!(matches!(&film_one.stream, StreamLocator::Server { extension: Some(ext), part: Some(_), .. } if ext == "mp4"));
    assert!(matches!(&film_two.stream, StreamLocator::Server { extension: Some(ext), part: Some(_), .. } if ext == "mkv"));
    assert_eq!(film_two.year, Some(2021));

    let shows = core.series_list(&BrowseQuery::default()).unwrap();
    let show = series(&shows, "Test Show");
    assert_eq!((show.year, show.rating, show.genre.as_deref()), (Some(2019), Some(8.1), Some("Drama, Comedy")));
    assert_eq!(show.category_id.as_deref(), Some(show_categories[0].id.as_str()));
    let episodes = core.episodes(&show.id).await.unwrap();
    assert_eq!(
        episodes.iter().map(|episode| (episode.season, episode.number, episode.title.as_str())).collect::<Vec<_>>(),
        [(1, 1, "Broadcast 1.1"), (1, 2, "Broadcast 1.2"), (2, 1, "Broadcast 2.1"), (2, 2, "Broadcast 2.2")]
    );
    let first = &episodes[0];
    assert_eq!((first.duration_seconds, first.plot.as_deref()), (Some(360), Some("Episode 1 of season 1.")));
    assert!(first.still_url.is_some() && first.air_date.is_some());
    assert!(matches!(&first.stream, StreamLocator::Server { extension: Some(ext), .. } if ext == "mkv"));

    // Nothing in the catalog holds the token; artwork is the server's own
    // path behind a xorvio-server address, fetched with the token added.
    let catalog = serde_json::to_string(&(&movies, &shows, &episodes)).unwrap();
    assert!(!catalog.contains(&token), "the token never reaches the catalog");
    for artwork in [&film_one.poster_url, &film_one.backdrop_url, &show.poster_url, &first.still_url] {
        let address = artwork.as_deref().expect("artwork");
        let parsed = url::Url::parse(address).unwrap();
        let (source, path) = parse_artwork_path(parsed.path()).expect("a server path");
        assert_eq!(source, config.id);
        let (bytes, content_type) = core.artwork(&config.id, &path).await.unwrap();
        assert!(bytes.len() > 100, "{path}");
        assert!(content_type.is_some_and(|kind| kind.starts_with("image/")), "{path}");
    }

    // MARK: Details

    let details = core.details(&film_one.id).await.unwrap();
    let mara = details.people.iter().find(|person| person.name == "Mara Ellison").expect("cast");
    assert_eq!(mara.role.as_deref(), Some("Ada Quill"));
    assert_eq!(details.director.as_deref(), Some("Joon Park"));
    assert_eq!(details.creators, ["Ines Okafor"]);
    assert_eq!(details.release_date.as_deref(), Some("2020-04-12"));
    assert_eq!((details.country.as_deref(), details.age_rating.as_deref()), (Some("Ireland"), Some("PG-13")));
    assert_eq!(details.trailer_youtube_id.as_deref(), Some("dQw4w9WgXcQ"));
    assert_eq!(details.tagline.as_deref(), Some("Every map is a guess."));
    assert_eq!(details.network.as_deref(), Some("Lantern Pictures"));
    assert_eq!((details.imdb_id.as_deref(), details.runtime_minutes), (Some("tt9900001"), Some(6)));
    assert_eq!(details.genres, ["Science Fiction", "Drama"]);
    assert!(details.logo_url.is_some() && !details.backdrop_urls.is_empty());
    let video = details.video.expect("the picture");
    assert_eq!((video.width, video.height, video.codec.as_deref(), video.is_hdr), (1280, 720, Some("h264"), false));
    assert_eq!(details.audio_channels, Some(2));
    let details = core.details(&film_two.id).await.unwrap();
    let video = details.video.expect("the picture");
    assert_eq!((video.width, video.height, video.codec.as_deref()), (1920, 1080, Some("hevc")));
    assert_eq!(details.audio_channels, Some(6));

    let details = core.details(&show.id).await.unwrap();
    assert_eq!(details.seasons.iter().map(|season| (season.number, season.episode_count)).collect::<Vec<_>>(), [
        (1, Some(2)),
        (2, Some(2))
    ]);
    assert!(details.seasons.iter().all(|season| season.cover_url.is_some()));
    assert_eq!(details.seasons[0].overview.as_deref(), Some("The station's season 1."));
    assert_eq!((details.network.as_deref(), details.age_rating.as_deref()), (Some("Northwind Broadcasting"), Some("TV-14")));
    assert_eq!(details.people.first().map(|person| person.name.as_str()), Some("Nadia Brooks"));

    // MARK: Playing

    for (locator, codecs) in [(&film_one.stream, "h264 aac"), (&film_two.stream, "hevc ac3"), (&first.stream, "h264 aac")] {
        let target = core.stream_target(&config.id, locator, false).unwrap();
        assert!(!target.url.contains(&token), "the token goes in a header, not the address");
        assert!(!target.is_live);
        assert_eq!(probe(&target).as_deref(), Some(codecs));
        assert_eq!(range_length(&target).await, 100, "ranges work, for seeking");
    }

    // MARK: What's been watched, both ways

    let started = progress(film_two, None, 150.0, false);
    core.save_progress(started).unwrap();
    server.wait_for(&film_two_key, |data| ticks(data) == 1_500_000_000 && !played(data), "the position reached the server").await;

    let finished_episode = episode(&episodes, 1, 2);
    core.save_progress(progress_of_episode(finished_episode, 340.0, true)).unwrap();
    server.wait_for(&episode_keys[0], |data| played(data) && ticks(data) == 0, "the finished episode is watched there").await;
    core.set_watched(&[progress_of_episode(finished_episode, 0.0, false)], false).unwrap();
    server.wait_for(&episode_keys[0], |data| !played(data), "and unwatched again").await;

    core.set_watched(&[progress(film_two, None, 0.0, true)], true).unwrap();
    server.wait_for(&film_two_key, |data| played(data) && ticks(data) == 0, "marked watched there").await;
    core.set_watched(&[progress(film_two, None, 0.0, false)], false).unwrap();
    server.wait_for(&film_two_key, |data| !played(data) && ticks(data) == 0, "and unwatched").await;

    // Watched elsewhere: a position set on the server shows up here after a refresh.
    let elsewhere = episode(&episodes, 2, 1);
    server.set_position(&episode_keys[1], 200.0).await;
    let status = core.refresh_source(&config.id, &|_| {}).await.unwrap();
    assert_eq!(status.last_error, None);
    let taken = core.progress(&elsewhere.id).unwrap().expect("the server's position came over");
    assert_eq!((taken.position_seconds, taken.finished, taken.kind), (200.0, false, MediaKind::Episode));
    assert_eq!(taken.series_id.as_deref(), Some(show.id.as_str()));
    assert!(taken.updated_at > 1_700_000_000, "as of when it was played there");

    // Leave the servers as they were found.
    server.set_played(&episode_keys[1], false).await;
}

fn movie<'a>(movies: &'a [Movie], title: &str) -> &'a Movie {
    movies.iter().find(|movie| movie.title == title).unwrap_or_else(|| panic!("{title} is in the catalog"))
}

fn series<'a>(shows: &'a [Series], title: &str) -> &'a Series {
    shows.iter().find(|show| show.title == title).unwrap_or_else(|| panic!("{title} is in the catalog"))
}

fn episode(episodes: &[Episode], season: i64, number: i64) -> &Episode {
    episodes.iter().find(|episode| (episode.season, episode.number) == (season, number)).expect("the episode")
}

fn progress(movie: &Movie, series_id: Option<&str>, position: f64, finished: bool) -> PlaybackProgress {
    PlaybackProgress {
        item_id: movie.id.clone(),
        kind: MediaKind::Movie,
        source_id: movie.source_id.clone(),
        series_id: series_id.map(str::to_string),
        position_seconds: position,
        duration_seconds: 360.0,
        finished,
        updated_at: 0,
    }
}

fn progress_of_episode(episode: &Episode, position: f64, finished: bool) -> PlaybackProgress {
    PlaybackProgress {
        item_id: episode.id.clone(),
        kind: MediaKind::Episode,
        source_id: episode.source_id.clone(),
        series_id: Some(episode.series_id.clone()),
        position_seconds: position,
        duration_seconds: 360.0,
        finished,
        updated_at: 0,
    }
}

/// The codecs ffprobe finds in a stream, opened with its headers.
fn probe(target: &StreamTarget) -> Option<String> {
    if Command::new("ffprobe").arg("-version").output().is_err() {
        eprintln!("ffprobe isn't installed; the stream wasn't probed");
        return Some("skipped".into());
    }
    let headers: String = target.headers.iter().map(|(name, value)| format!("{name}: {value}\r\n")).collect();
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-headers", &headers, "-show_entries", "stream=codec_name", "-of", "csv=p=0", &target.url])
        .output()
        .unwrap();
    assert!(output.status.success(), "ffprobe: {}", String::from_utf8_lossy(&output.stderr));
    Some(String::from_utf8_lossy(&output.stdout).split_whitespace().collect::<Vec<_>>().join(" "))
}

/// The length of the first 100 bytes, asked for as a range with the target's headers.
async fn range_length(target: &StreamTarget) -> usize {
    let mut request = reqwest::Client::new().get(&target.url).header("Range", "bytes=0-99");
    for (name, value) in &target.headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let response = request.send().await.unwrap();
    assert_eq!(response.status().as_u16(), 206);
    response.bytes().await.unwrap().len()
}

fn ticks(data: &Value) -> i64 {
    data["PlaybackPositionTicks"].as_i64().unwrap_or(0)
}

fn played(data: &Value) -> bool {
    data["Played"].as_bool().unwrap_or(false)
}

/// The server's API, asked directly, to check what Xorvio did there.
struct ServerApi {
    kind: SourceKind,
    base: String,
    token: String,
    user_id: String,
}

impl ServerApi {
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let header = format!(r#"MediaBrowser Client="Xorvio tests", Device="tests", DeviceId="xorvio-tests", Version="1", Token="{}""#, self.token);
        reqwest::Client::new().request(method, format!("{}{path}", self.base)).header("Authorization", header)
    }

    fn jellyfin(&self) -> bool {
        self.kind == SourceKind::Jellyfin
    }

    async fn find(&self, item_type: &str, name: &str) -> String {
        let path = if self.jellyfin() { "/Items".to_string() } else { format!("/Users/{}/Items", self.user_id) };
        let reply: Value = self
            .request(reqwest::Method::GET, &format!("{path}?UserId={}&Recursive=true&IncludeItemTypes={item_type}", self.user_id))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let items = reply["Items"].as_array().cloned().unwrap_or_default();
        let item = items.iter().find(|item| item["Name"] == name).unwrap_or_else(|| panic!("{name} is on the server"));
        item["Id"].as_str().unwrap().to_string()
    }

    async fn user_data(&self, key: &str) -> Value {
        let path = if self.jellyfin() {
            format!("/Items/{key}?UserId={}", self.user_id)
        } else {
            format!("/Users/{}/Items/{key}", self.user_id)
        };
        let item: Value = self.request(reqwest::Method::GET, &path).send().await.unwrap().json().await.unwrap();
        item["UserData"].clone()
    }

    async fn set_played(&self, key: &str, played: bool) {
        let path = if self.jellyfin() {
            format!("/UserPlayedItems/{key}?UserId={}", self.user_id)
        } else {
            format!("/Users/{}/PlayedItems/{key}", self.user_id)
        };
        let method = if played { reqwest::Method::POST } else { reqwest::Method::DELETE };
        assert!(self.request(method, &path).send().await.unwrap().status().is_success());
    }

    async fn set_position(&self, key: &str, seconds: f64) {
        let path = if self.jellyfin() {
            format!("/UserItems/{key}/UserData?UserId={}", self.user_id)
        } else {
            format!("/Users/{}/Items/{key}/UserData", self.user_id)
        };
        let played_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let body = json!({ "PlaybackPositionTicks": (seconds * 10_000_000.0) as i64, "LastPlayedDate": played_at });
        assert!(self.request(reqwest::Method::POST, &path).json(&body).send().await.unwrap().status().is_success());
    }

    /// Waits for Xorvio's report (sent in the background) to show on the server.
    async fn wait_for(&self, key: &str, check: impl Fn(&Value) -> bool, what: &str) {
        let start = Instant::now();
        loop {
            let data = self.user_data(key).await;
            if check(&data) {
                return;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "{what}: the server has {data}");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}
