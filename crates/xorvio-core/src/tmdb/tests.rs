//! TMDB end to end, through [`Core`]'s public calls, against a stand-in TMDB
//! (which serves the playlist too) on 127.0.0.1. Nothing here reaches
//! api.themoviedb.org, the keychain or anyone's own data.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use xorvio_model::*;

use super::{fill_episodes, is_numbered_only, overlay, renamed_by_tmdb};
use crate::details::DetailsRecord;
use crate::tmdb::api::{TmdbEpisode, TmdbTitle};
use crate::{BrowseQuery, Core, MemorySecrets, SecretStore};

const API_KEY: &str = "0123456789abcdef0123456789abcdef";
const TOKEN: &str = "eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiJ0ZXN0In0.c2lnbmF0dXJl";

// MARK: The stand-in

#[derive(Debug, Clone)]
struct Call {
    path: String,
    query: HashMap<String, String>,
    authorization: Option<String>,
}

struct StandIn {
    base: String,
    calls: Arc<Mutex<Vec<Call>>>,
    /// A status to answer every API request with instead (401, 500), or 0.
    failing: Arc<AtomicU16>,
}

impl StandIn {
    async fn start() -> StandIn {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let calls: Arc<Mutex<Vec<Call>>> = Arc::default();
        let failing = Arc::new(AtomicU16::new(0));
        let (log, fail, playlist_base) = (calls.clone(), failing.clone(), base.clone());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (log, fail, playlist_base) = (log.clone(), fail.clone(), playlist_base.clone());
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
                    let target = text.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let parsed = url::Url::parse(&format!("http://test{target}")).unwrap();
                    let authorization = text.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("authorization").then(|| value.trim().to_string())
                    });
                    let call = Call {
                        path: parsed.path().to_string(),
                        query: parsed.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect(),
                        authorization,
                    };
                    let (status, body) = if call.path == "/list.m3u" {
                        (200, playlist(&playlist_base).into_bytes())
                    } else {
                        log.lock().unwrap().push(call.clone());
                        let (status, body) = match fail.load(Ordering::Relaxed) {
                            0 => answer(&call),
                            status => (status, json!({"status_message": "no"})),
                        };
                        (status, body.to_string().into_bytes())
                    };
                    let header = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        StandIn { base, calls, failing }
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    fn count(&self, path: &str) -> usize {
        self.calls().iter().filter(|call| call.path == path).count()
    }

    fn fail_with(&self, status: u16) {
        self.failing.store(status, Ordering::Relaxed);
    }
}

fn playlist(base: &str) -> String {
    format!(
        "#EXTM3U\n\
         #EXTINF:-1 group-title=\"Films\" tmdb-id=\"603\",The Matrix (1999)\n{base}/movie/matrix.mp4\n\
         #EXTINF:-1 group-title=\"Films\",Paper Boats (2024)\n{base}/movie/paper.mp4\n\
         #EXTINF:-1 group-title=\"Films\",The Matrix Reloaded (2003)\n{base}/movie/reloaded.mp4\n\
         #EXTINF:-1 group-title=\"Films\",John Wick (2014)\n{base}/movie/wick.mp4\n\
         #EXTINF:-1 group-title=\"Films\",Unknown Picture (2011)\n{base}/movie/unknown.mp4\n\
         #EXTINF:-1 group-title=\"Series\",Harbor Lights S01E01\n{base}/series/harbor/1.mp4\n\
         #EXTINF:-1 group-title=\"Series\",Harbor Lights S01E02\n{base}/series/harbor/2.mp4\n"
    )
}

/// What the stand-in TMDB says to each request.
fn answer(call: &Call) -> (u16, Value) {
    let authorized = call.query.get("api_key").map(String::as_str) == Some(API_KEY)
        || call.authorization.as_deref() == Some(&format!("Bearer {TOKEN}"));
    if !authorized {
        return (401, json!({"status_code": 7, "status_message": "Invalid API key"}));
    }
    let query = call.query.get("query").map(String::as_str).unwrap_or_default();
    let appended = call.query.get("append_to_response").map(String::as_str).unwrap_or_default();
    let body = match call.path.as_str() {
        "/3/configuration" => json!({"images": {}}),
        "/3/movie/603" => json!({
            "id": 603, "title": "The Matrix", "tagline": "Welcome to the Real World.", "runtime": 136,
            "release_date": "1999-03-30", "overview": "A hacker learns the truth.",
            "images": {"backdrops": [{"file_path": "/matrix.jpg", "iso_639_1": null}], "logos": [{"file_path": "/matrix-logo.png", "iso_639_1": "en"}]},
            "credits": {"cast": [{"id": 6384, "name": "Keanu Reeves", "character": "Neo", "profile_path": "/keanu.jpg"}]},
            "vote_average": 8.217, "vote_count": 26000, "poster_path": "/matrix-poster.jpg",
            "videos": {"results": [
                {"site": "YouTube", "type": "Featurette", "key": "making-of", "name": "Making Of", "official": true},
                {"site": "Vimeo", "type": "Trailer", "key": "elsewhere"},
                {"site": "YouTube", "type": "Trailer", "key": "fan-cut", "name": "Fan Trailer", "official": false},
                {"site": "YouTube", "type": "Trailer", "key": "official", "name": "Official Trailer", "official": true}
            ]},
            "recommendations": {"results": [
                {"id": 999, "title": "Not in the Library", "release_date": "2001-01-01"},
                {"id": 604, "title": "The Matrix Reloaded", "release_date": "2003-05-15"}
            ]}
        }),
        "/3/movie/700" => json!({"id": 700, "title": "Paper Boats", "poster_path": "/paper-poster.jpg", "vote_count": 3, "vote_average": 9.9, "images": {"logos": [{"file_path": "/paper-logo.png", "iso_639_1": "en"}]}}),
        "/3/search/movie" => match (query, call.query.get("year").map(String::as_str)) {
            ("Paper Boats", Some("2024")) => json!({"results": [{"id": 700, "title": "Paper Boats", "release_date": "2024-05-01"}]}),
            _ => json!({"results": []}),
        },
        "/3/search/tv" if query == "Harbor Lights" => json!({"results": [{"id": 1399, "name": "Harbor Lights", "first_air_date": "2021-01-10"}]}),
        "/3/search/tv" => json!({"results": []}),
        "/3/tv/1399" if appended.starts_with("season/") => json!({
            "id": 1399,
            "season/1": {"episodes": [
                {"season_number": 1, "episode_number": 1, "name": "Launch", "overview": "It begins.", "still_path": "/e1.jpg", "runtime": 50, "air_date": "2021-01-10"},
                {"season_number": 1, "episode_number": 2, "name": "Undertow", "overview": "It goes on.", "still_path": "/e2.jpg"}
            ]}
        }),
        "/3/tv/1399" => json!({
            "id": 1399, "name": "Harbor Lights", "first_air_date": "2021-01-10", "networks": [{"name": "Harbor"}],
            "aggregate_credits": {"cast": [{"id": 9, "name": "Mara Ellison", "roles": [{"character": "Ada"}]}]},
            "seasons": [{"season_number": 1, "name": "Season 1", "episode_count": 2, "poster_path": "/s1.jpg"}]
        }),
        "/3/person/6384" => json!({
            "id": 6384, "name": "Keanu Reeves", "known_for_department": "Acting", "biography": "An actor.", "profile_path": "/keanu.jpg",
            "combined_credits": {"cast": [
                {"media_type": "movie", "id": 603, "title": "The Matrix", "release_date": "1999-03-30", "popularity": 80.0},
                {"media_type": "movie", "id": 245891, "title": "John Wick", "release_date": "2014-10-22", "popularity": 90.0},
                {"media_type": "tv", "id": 22, "name": "Harbor Lights", "character": "Himself", "popularity": 99.0}
            ]}
        }),
        "/3/search/person" if query == "Keanu Reeves" => json!({"results": [
            {"id": 1, "name": "Keanu Reeves", "known_for": [{"media_type": "movie", "id": 5}]},
            {"id": 6384, "name": "Keanu Reeves", "known_for": [{"media_type": "movie", "id": 603}]}
        ]}),
        "/3/search/person" => json!({"results": []}),
        "/3/find/tt0133093" => json!({"movie_results": [{"id": 603}], "tv_results": []}),
        "/3/find/tt5555555" => json!({"movie_results": [], "tv_results": [{"id": 1399}]}),
        _ => return (404, json!({"status_code": 34})),
    };
    (200, body)
}

// MARK: Helpers

async fn open_with_playlist(stand_in: &StandIn) -> (tempfile::TempDir, Arc<Core>, Arc<MemorySecrets>) {
    let folder = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let core = Core::open(folder.path(), secrets.clone()).unwrap();
    core.set_tmdb_base(&stand_in.base);
    // TVmaze is asked only when TMDB can't help, but never from tests.
    core.set_tvmaze_enabled(false);
    let source = core
        .add_source(NewSource {
            name: "My List".into(),
            kind: SourceKind::M3uUrl,
            secrets: SourceSecrets { playlist_url: Some(format!("{}/list.m3u", stand_in.base)), ..Default::default() },
            file_path: None,
            live_format: LiveStreamFormat::Hls,
            user_agent: None,
        })
        .await
        .unwrap();
    let status = core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert_eq!((status.movie_count, status.series_count), (5, 1), "{status:?}");
    (folder, core, secrets)
}

fn movie(core: &Core, title: &str) -> Movie {
    core.movies(&BrowseQuery::default()).unwrap().into_iter().find(|movie| movie.title == title).unwrap()
}

fn series(core: &Core, title: &str) -> Series {
    core.series_list(&BrowseQuery::default()).unwrap().into_iter().find(|series| series.title == title).unwrap()
}

// MARK: Tests

#[tokio::test]
async fn without_a_key_tmdb_is_never_asked() {
    let stand_in = StandIn::start().await;
    let (_folder, core, _) = open_with_playlist(&stand_in).await;
    assert_eq!(core.tmdb_status(), TmdbStatus::default());
    let matrix = movie(&core, "The Matrix");
    let details = core.details(&matrix.id).await.unwrap();
    assert_eq!((details.logo_url, details.attribution), (None, None));
    assert!(core.related_movies(&matrix.id).await.unwrap().is_empty());
    assert_eq!(core.person("Keanu Reeves", Some("6384"), None).await.unwrap(), None);
    let harbor = series(&core, "Harbor Lights");
    assert_eq!(core.episodes(&harbor.id).await.unwrap()[0].title, "Harbor Lights S01E01");
    assert!(stand_in.calls().is_empty());
}

#[tokio::test]
async fn switched_off_tmdb_is_never_asked_even_with_a_key() {
    let stand_in = StandIn::start().await;
    let (_folder, core, _) = open_with_playlist(&stand_in).await;
    core.tmdb_set_key(API_KEY).await.unwrap();
    let checked = stand_in.calls().len();
    core.set_tmdb_enabled(false);
    let matrix = movie(&core, "The Matrix");
    let details = core.details(&matrix.id).await.unwrap();
    assert_eq!((details.logo_url, details.attribution), (None, None));
    assert!(core.related_movies(&matrix.id).await.unwrap().is_empty());
    assert_eq!(core.person("Keanu Reeves", Some("6384"), None).await.unwrap(), None);
    let harbor = series(&core, "Harbor Lights");
    assert_eq!(core.episodes(&harbor.id).await.unwrap()[0].title, "Harbor Lights S01E01");
    assert_eq!(stand_in.calls().len(), checked, "nothing after the key was checked");

    core.set_tmdb_enabled(true);
    assert!(core.details(&matrix.id).await.unwrap().logo_url.is_some(), "back on, TMDB is asked again");
}

#[tokio::test]
async fn add_on_titles_are_found_by_their_imdb_id() {
    let stand_in = StandIn::start().await;
    let (_folder, core, _) = open_with_playlist(&stand_in).await;
    core.tmdb_set_key(API_KEY).await.unwrap();

    // An add-on's copy of a film: not in the catalog, and known by its IMDb id.
    let mut film = movie(&core, "The Matrix");
    (film.id, film.source_id, film.tmdb_id, film.imdb_id) = ("addon|movie|tt0133093".into(), "addon".into(), None, Some("tt0133093".into()));
    let own = MediaDetails { overview: Some("The add-on's.".into()), genres: vec!["Sci-Fi".into()], ..Default::default() };
    let details = core.addon_movie_details(&film, own).await;
    assert!(details.logo_url.is_some_and(|logo| logo.ends_with("/matrix-logo.png")));
    assert_eq!(details.overview.as_deref(), Some("A hacker learns the truth."));
    assert_eq!(details.genres, ["Sci-Fi"], "the add-on's own stay where TMDB has nothing");
    assert_eq!(details.people[0].tmdb_id.as_deref(), Some("6384"));
    let related = core.movies_related_to(&film).await.unwrap();
    assert_eq!(related.iter().map(|movie| movie.title.as_str()).collect::<Vec<_>>(), ["The Matrix Reloaded"]);

    // Someone from its cast is told from namesakes by the add-on title they're in.
    let keanu = core.person("Keanu Reeves", None, Some("addon|movie|tt0133093")).await.unwrap().unwrap();
    assert_eq!(keanu.tmdb_id, "6384");
    assert_eq!(stand_in.count("/3/find/tt0133093"), 1, "asked once, then kept");

    let mut show = series(&core, "Harbor Lights");
    (show.id, show.source_id, show.imdb_id) = ("addon|series|tt5555555".into(), "addon".into(), Some("tt5555555".into()));
    let details = core.addon_series_details(&show, MediaDetails::default()).await;
    assert_eq!((details.network.as_deref(), details.seasons.len()), (Some("Harbor"), 1));
}

#[tokio::test]
async fn a_key_is_kept_only_once_tmdb_takes_it() {
    let stand_in = StandIn::start().await;
    let (folder, core, secrets) = open_with_playlist(&stand_in).await;

    let refused = core.tmdb_set_key("ffffffffffffffffffffffffffffffff").await.unwrap_err().to_string();
    assert!(refused.contains("didn't accept"), "{refused}");
    assert_eq!(secrets.get("tmdb:key").unwrap(), None);
    assert!(core.tmdb_set_key("two words").await.unwrap_err().to_string().contains("isn't a TMDB key"));

    // An API key goes in the address; it's the only way TMDB takes one.
    let status = core.tmdb_set_key(&format!("  {API_KEY}\n")).await.unwrap();
    assert!(status.configured && status.custom_key && !status.rejected);
    assert_eq!(secrets.get("tmdb:key").unwrap().as_deref(), Some(API_KEY));
    let checked = stand_in.calls().pop().unwrap();
    assert_eq!((checked.query.get("api_key").map(String::as_str), checked.authorization), (Some(API_KEY), None));

    // A read access token goes in a header and never in the address.
    core.tmdb_set_key(TOKEN).await.unwrap();
    let checked = stand_in.calls().pop().unwrap();
    assert_eq!((checked.query.get("api_key"), checked.authorization), (None, Some(format!("Bearer {TOKEN}"))));

    // TMDB being down isn't the key's fault.
    stand_in.fail_with(503);
    let unreachable = core.tmdb_set_key(API_KEY).await.unwrap_err().to_string();
    assert!(unreachable.contains("couldn't be reached"), "{unreachable}");
    assert_eq!(secrets.get("tmdb:key").unwrap().as_deref(), Some(TOKEN), "the working key stays");

    let status = core.tmdb_set_key("").await.unwrap();
    assert_eq!(status, TmdbStatus::default());
    assert_eq!(secrets.get("tmdb:key").unwrap(), None);

    // A key saved earlier is used after a restart.
    secrets.set("tmdb:key", API_KEY).unwrap();
    let reopened = Core::open(folder.path(), secrets.clone()).unwrap();
    assert!(reopened.tmdb_status().custom_key);
}

#[tokio::test]
async fn films_series_episodes_and_people_from_tmdb() {
    let stand_in = StandIn::start().await;
    let (_folder, core, _) = open_with_playlist(&stand_in).await;
    core.tmdb_set_key(API_KEY).await.unwrap();

    // A film with a TMDB id from its source.
    let matrix = movie(&core, "The Matrix");
    let details = core.details(&matrix.id).await.unwrap();
    assert_eq!(details.logo_url.as_deref(), Some("https://image.tmdb.org/t/p/w500/matrix-logo.png"));
    assert_eq!(details.backdrop_urls, ["https://image.tmdb.org/t/p/w1280/matrix.jpg"]);
    assert_eq!(details.people[0].tmdb_id.as_deref(), Some("6384"));
    assert_eq!(details.attribution.as_deref(), Some("TMDB"));
    assert_eq!(details.runtime_minutes, Some(136));
    // Kept: opening it again doesn't ask again.
    core.details(&matrix.id).await.unwrap();
    assert_eq!(stand_in.count("/3/movie/603"), 1);

    // A film found by its name and year.
    let paper = movie(&core, "Paper Boats");
    assert_eq!(core.details(&paper.id).await.unwrap().logo_url.as_deref(), Some("https://image.tmdb.org/t/p/w500/paper-logo.png"));
    let search = stand_in.calls().into_iter().find(|call| call.path == "/3/search/movie").unwrap();
    assert_eq!((search.query["query"].as_str(), search.query["year"].as_str()), ("Paper Boats", "2024"));
    core.details(&paper.id).await.unwrap();
    assert_eq!(stand_in.count("/3/search/movie"), 1, "the match is kept");

    // Nothing convincing: the miss is kept too, and the film keeps its own details.
    let unknown = movie(&core, "Unknown Picture");
    let searches = stand_in.count("/3/search/movie");
    assert_eq!(core.details(&unknown.id).await.unwrap().attribution, None);
    assert_eq!(stand_in.count("/3/search/movie"), searches + 2, "with the year, then without");
    core.details(&unknown.id).await.unwrap();
    assert_eq!(stand_in.count("/3/search/movie"), searches + 2);

    // Recommendations the sources carry, found by name and year.
    let related = core.related_movies(&matrix.id).await.unwrap();
    assert_eq!(related.iter().map(|movie| movie.title.as_str()).collect::<Vec<_>>(), ["The Matrix Reloaded"]);
    // Hidden categories stay hidden here too.
    core.set_hidden(related[0].category_id.as_deref().unwrap(), true).unwrap();
    assert!(core.related_movies(&matrix.id).await.unwrap().is_empty());
    core.set_hidden(related[0].category_id.as_deref().unwrap(), false).unwrap();

    // A series: its seasons, and episodes named and pictured by TMDB.
    let harbor = series(&core, "Harbor Lights");
    let (details, episodes) = tokio::join!(core.details(&harbor.id), core.episodes(&harbor.id));
    let (details, episodes) = (details.unwrap(), episodes.unwrap());
    assert_eq!(details.network.as_deref(), Some("Harbor"));
    assert_eq!(details.seasons[0].cover_url.as_deref(), Some("https://image.tmdb.org/t/p/w500/s1.jpg"));
    assert_eq!(episodes.iter().map(|episode| episode.title.as_str()).collect::<Vec<_>>(), ["Launch", "Undertow"]);
    assert_eq!(episodes[0].still_url.as_deref(), Some("https://image.tmdb.org/t/p/w780/e1.jpg"));
    assert_eq!(episodes[0].duration_seconds, Some(3000));
    assert!(episodes[0].air_date.is_some());
    assert_eq!((stand_in.count("/3/search/tv"), stand_in.count("/3/tv/1399")), (1, 2), "asked once, though both asked at once");

    // Someone from the cast, with what of theirs is here, best known first;
    // appearing as themselves doesn't count.
    let keanu = core.person("Keanu Reeves", Some("6384"), Some(&matrix.id)).await.unwrap().unwrap();
    assert_eq!(keanu.photo_url.as_deref(), Some("https://image.tmdb.org/t/p/h632/keanu.jpg"));
    assert_eq!(keanu.movies.iter().map(|movie| movie.title.as_str()).collect::<Vec<_>>(), ["John Wick", "The Matrix"]);
    assert!(keanu.series.is_empty());

    // By name alone, the person known for the title they're from wins.
    let by_name = core.person("Keanu Reeves", None, Some(&matrix.id)).await.unwrap().unwrap();
    assert_eq!(by_name.tmdb_id, "6384");
    assert_eq!(stand_in.count("/3/person/6384"), 1);
    assert_eq!(core.person("Nobody Known", None, None).await.unwrap(), None);
}

#[tokio::test]
async fn a_key_turned_down_later_stops_every_request() {
    let stand_in = StandIn::start().await;
    let (_folder, core, _) = open_with_playlist(&stand_in).await;
    core.tmdb_set_key(API_KEY).await.unwrap();
    stand_in.fail_with(401);
    let matrix = movie(&core, "The Matrix");
    assert_eq!(core.details(&matrix.id).await.unwrap().attribution, None, "the source's own details still show");
    assert!(core.tmdb_status().rejected);
    let asked = stand_in.calls().len();
    let paper = movie(&core, "Paper Boats");
    core.details(&paper.id).await.unwrap();
    assert_eq!(stand_in.calls().len(), asked);

    stand_in.fail_with(0);
    assert!(!core.tmdb_set_key(API_KEY).await.unwrap().rejected);
    assert_eq!(core.details(&matrix.id).await.unwrap().attribution.as_deref(), Some("TMDB"));
}

#[tokio::test]
async fn when_tmdb_is_down_it_waits_before_asking_again() {
    let stand_in = StandIn::start().await;
    let (_folder, core, _) = open_with_playlist(&stand_in).await;
    core.tmdb_set_key(API_KEY).await.unwrap();
    stand_in.fail_with(500);
    let paper = movie(&core, "Paper Boats");
    core.details(&paper.id).await.unwrap();
    let asked = stand_in.count("/3/search/movie");
    assert_eq!(asked, 1, "a server error isn't retried");
    stand_in.fail_with(0);
    core.details(&paper.id).await.unwrap();
    assert_eq!(stand_in.count("/3/search/movie"), asked, "not again for a while");
    assert!(!core.tmdb_status().rejected);
}

#[tokio::test]
async fn the_demo_library_is_never_sent() {
    let stand_in = StandIn::start().await;
    let folder = tempfile::tempdir().unwrap();
    let core = Core::open(folder.path(), Arc::new(MemorySecrets::new())).unwrap();
    core.set_tmdb_base(&stand_in.base);
    core.tmdb_set_key(API_KEY).await.unwrap();
    core.add_demo_source().unwrap();
    let asked = stand_in.calls().len();
    for movie in core.movies(&BrowseQuery::default()).unwrap().into_iter().take(3) {
        core.details(&movie.id).await.unwrap();
        core.related_movies(&movie.id).await.unwrap();
    }
    for series in core.series_list(&BrowseQuery::default()).unwrap().into_iter().take(2) {
        core.episodes(&series.id).await.unwrap();
    }
    assert_eq!(stand_in.calls().len(), asked);
}

#[tokio::test]
async fn ratings_and_videos_come_with_a_films_details() {
    let stand_in = StandIn::start().await;
    let (_folder, core, _) = open_with_playlist(&stand_in).await;
    core.tmdb_set_key(API_KEY).await.unwrap();
    let matrix = movie(&core, "The Matrix");
    let details = core.details(&matrix.id).await.unwrap();
    assert_eq!(details.ratings, [TitleRating { source: RatingSource::Tmdb, score: 8.2, votes: Some(26000) }]);
    let videos: Vec<(&str, Option<&str>)> = details.videos.iter().map(|video| (video.youtube_id.as_str(), video.kind.as_deref())).collect();
    assert_eq!(videos, [("official", Some("Trailer")), ("fan-cut", Some("Trailer")), ("making-of", Some("Featurette"))], "YouTube only, trailers first, official ones first");
    assert_eq!(details.videos[0].title, "Official Trailer");
    // A score only a few have voted on means little.
    let paper = movie(&core, "Paper Boats");
    assert!(core.details(&paper.id).await.unwrap().ratings.is_empty());

    // Details kept before ratings and videos were are asked for again once.
    let older = serde_json::json!({"details": {"attribution": "TMDB", "overview": "Kept before."}});
    core.catalog.save_tmdb_cached("movie", 603, &older.to_string(), crate::now()).unwrap();
    assert_eq!(core.details(&matrix.id).await.unwrap().ratings.len(), 1);
    assert_eq!(stand_in.count("/3/movie/603"), 2);
    core.details(&matrix.id).await.unwrap();
    assert_eq!(stand_in.count("/3/movie/603"), 2);
}

#[tokio::test]
async fn lists_fill_missing_posters_from_tmdb_once() {
    let stand_in = StandIn::start().await;
    let (_folder, core, _) = open_with_playlist(&stand_in).await;
    let ids: Vec<String> = core.movies(&BrowseQuery::default()).unwrap().into_iter().map(|movie| movie.id).collect();
    assert!(core.fill_posters(&ids).await.unwrap().is_empty(), "nothing without a key");
    assert!(stand_in.calls().is_empty());

    core.tmdb_set_key(API_KEY).await.unwrap();
    let paper = movie(&core, "Paper Boats");
    let unknown = movie(&core, "Unknown Picture");
    let asked = [paper.id.clone(), unknown.id.clone()];
    let found = core.fill_posters(&asked).await.unwrap();
    assert_eq!(found, [(paper.id.clone(), "https://image.tmdb.org/t/p/w342/paper-poster.jpg".to_string())]);
    let poster_call = stand_in.calls().into_iter().find(|call| call.path == "/3/movie/700").unwrap();
    assert!(!poster_call.query.contains_key("append_to_response"), "only the poster is asked for");

    // Kept: lists show it at once, and nothing is asked again, found or not.
    assert_eq!(movie(&core, "Paper Boats").poster_url.as_deref(), Some("https://image.tmdb.org/t/p/w342/paper-poster.jpg"));
    let calls = stand_in.calls().len();
    assert_eq!(core.fill_posters(&asked).await.unwrap().len(), 1);
    assert_eq!(stand_in.calls().len(), calls);

    // A film opened before uses TMDB's reply kept for its page.
    let matrix = movie(&core, "The Matrix");
    core.details(&matrix.id).await.unwrap();
    let calls = stand_in.calls().len();
    let found = core.fill_posters(std::slice::from_ref(&matrix.id)).await.unwrap();
    assert_eq!(found[0].1, "https://image.tmdb.org/t/p/w342/matrix-poster.jpg");
    assert_eq!(stand_in.calls().len(), calls);

    // When TMDB can't be reached, nothing is kept and the rest wait.
    stand_in.fail_with(500);
    let wick = movie(&core, "John Wick");
    assert!(core.fill_posters(std::slice::from_ref(&wick.id)).await.unwrap().is_empty());
    assert_eq!(core.catalog.poster_lookup(&wick.id).unwrap(), None);
}

#[tokio::test]
async fn the_demo_library_never_has_its_posters_looked_for() {
    let stand_in = StandIn::start().await;
    let folder = tempfile::tempdir().unwrap();
    let core = Core::open(folder.path(), Arc::new(MemorySecrets::new())).unwrap();
    core.set_tmdb_base(&stand_in.base);
    core.tmdb_set_key(API_KEY).await.unwrap();
    core.add_demo_source().unwrap();
    let asked = stand_in.calls().len();
    let ids: Vec<String> = core.movies(&BrowseQuery::default()).unwrap().into_iter().map(|movie| movie.id).collect();
    core.fill_posters(&ids).await.unwrap();
    assert_eq!(stand_in.calls().len(), asked);
}

#[test]
fn media_servers_keep_their_own_details() {
    let stored = DetailsRecord { director: Some("The Server's".into()), ..Default::default() };
    let tmdb = TmdbTitle {
        details: DetailsRecord { director: Some("TMDB's".into()), logo_url: Some("https://image.tmdb.org/l.png".into()), ..Default::default() },
        ..Default::default()
    };
    let server = overlay(stored.clone(), &tmdb, true);
    assert_eq!((server.director.as_deref(), server.logo_url.is_some()), (Some("The Server's"), true));
    assert_eq!(overlay(stored, &tmdb, false).director.as_deref(), Some("TMDB's"));
}

#[test]
fn only_numbered_episode_titles_are_renamed() {
    for numbered in ["Episode 4", "S01E04", "Paper Boats S01 E04", "Paper Boats - Episode 4", "Paper Boats", "4", ""] {
        assert!(is_numbered_only(numbered, "Paper Boats"), "{numbered}");
    }
    for named in ["Pilot", "The Long Night", "Paper Boats Return", "1984 Revisited"] {
        assert!(!is_numbered_only(named, "Paper Boats"), "{named}");
    }
    // TMDB's name dressed in a provider's tags and numbering is tidied too.
    let series = "A+ - Severance (2022) (US)";
    assert!(renamed_by_tmdb("A+ - Severance (2022) (US) - S01E01 - Good News About Hell", series, "Good News About Hell"));
    assert!(renamed_by_tmdb("Severance - Good News About Hell", "Severance", "Good News About Hell"));
    assert!(!renamed_by_tmdb("Good News About Hell", series, "Good News About Hell"), "already TMDB's name");
    assert!(!renamed_by_tmdb("The Director's Cut of Hell", series, "Hell"), "a different title that ends the same way");
    assert!(!renamed_by_tmdb("Pilot", series, "Good News About Hell"));

    let episode = |title: &str, plot: Option<&str>| Episode {
        id: "s|se|1|ep|1".into(),
        series_id: "s|se|1".into(),
        source_id: "s".into(),
        season: 1,
        number: 1,
        title: title.into(),
        plot: plot.map(str::to_string),
        duration_seconds: None,
        still_url: Some("https://provider.example/still.jpg".into()),
        stream: StreamLocator::Url { url: "http://provider.example/1.mp4".into() },
        air_date: None,
    };
    let tmdb = [TmdbEpisode { season: 1, number: 1, name: Some("Launch".into()), overview: Some("It begins.".into()), still_url: Some("https://image.tmdb.org/e.jpg".into()), ..Default::default() }];
    let filled = fill_episodes("Paper Boats", vec![episode("Pilot", Some("The provider's."))], &tmdb);
    assert_eq!((filled[0].title.as_str(), filled[0].plot.as_deref()), ("Pilot", Some("The provider's.")));
    assert_eq!(filled[0].still_url.as_deref(), Some("https://provider.example/still.jpg"), "the source's own still stays");
    let filled = fill_episodes("Paper Boats", vec![episode("Episode 1", None)], &tmdb);
    assert_eq!((filled[0].title.as_str(), filled[0].plot.as_deref()), ("Launch", Some("It begins.")));
}
