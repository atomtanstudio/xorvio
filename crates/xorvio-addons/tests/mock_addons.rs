//! The add-on client against local mock add-ons: nothing here reaches the internet.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use xorvio_addons::{AddonError, Addons, MemorySecrets, SecretStore};
use xorvio_model::StreamLocator;

const KEY: &str = "secret-key-123456789";

/// Serves fixed JSON by exact path; anything under `/broken/` but its
/// manifest is a server error, anything else unknown is a 404.
async fn serve(State(routes): State<Arc<HashMap<String, Value>>>, uri: Uri) -> Response {
    match routes.get(uri.path()) {
        Some(body) => axum::Json(body.clone()).into_response(),
        None if uri.path().starts_with("/broken/") => {
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

struct Mock {
    base: String,
    secrets: Arc<MemorySecrets>,
    dir: tempfile::TempDir,
}

impl Mock {
    fn link(&self, path: &str) -> String {
        format!("{}/{path}/manifest.json", self.base)
    }

    fn open(&self) -> Arc<Addons> {
        Addons::open(self.dir.path(), self.secrets.clone()).unwrap()
    }

    async fn with_addons(&self, paths: &[&str]) -> Arc<Addons> {
        let addons = self.open();
        for path in paths {
            addons.install(&self.link(path)).await.unwrap();
        }
        addons
    }
}

async fn mock() -> Mock {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let streams = format!("/streams/{KEY}");
    let mut routes: HashMap<String, Value> = HashMap::new();
    let mut add = |path: String, body: Value| {
        routes.insert(path, body);
    };

    // A catalog and metadata add-on.
    add(
        "/meta/manifest.json".into(),
        json!({
            "id": "org.test.meta", "name": "Test Meta", "version": "1.0.0", "types": ["movie", "series"],
            "resources": ["catalog", { "name": "meta", "types": ["movie", "series"], "idPrefixes": ["tt"] }],
            "catalogs": [
                { "type": "movie", "id": "popular", "name": "Popular", "extra": [{ "name": "genre", "options": ["Drama", "Comedy"] }, { "name": "skip" }, { "name": "search" }] },
                { "type": "series", "id": "popular", "name": "Popular", "extra": [{ "name": "search" }] },
                { "type": "movie", "id": "find", "extra": [{ "name": "search", "isRequired": true }] }
            ]
        }),
    );
    add(
        "/meta/catalog/movie/popular.json".into(),
        json!({ "metas": [
            { "id": "tt0000001", "type": "movie", "name": "Night Train", "year": 1999, "poster": "https://img.example/1.jpg" },
            { "id": "tt0000002", "type": "movie", "name": "Day Boat", "releaseInfo": "2001" },
            { "id": "kitsu:5", "type": "movie", "name": "Nothing can play this" },
            { "name": "no id" }
        ]}),
    );
    add(
        "/meta/catalog/movie/popular/genre=Drama&skip=20.json".into(),
        json!({ "metas": [
            { "id": "tt0000003", "type": "movie", "name": "Drama Film" }
        ]}),
    );
    add(
        "/meta/catalog/series/popular.json".into(),
        json!({ "metas": [
            { "id": "tt0000010", "type": "series", "name": "Night Shift", "releaseInfo": "2010–2014" }
        ]}),
    );
    add(
        "/meta/catalog/movie/popular/search=night.json".into(),
        json!({ "metas": [
            { "id": "tt0000001", "type": "movie", "name": "Night Train" },
            { "id": "tt0000004", "type": "movie", "name": "Unrelated" },
            { "id": "tt0000005", "type": "movie", "name": "Night" }
        ]}),
    );
    add(
        "/meta/catalog/movie/find/search=night.json".into(),
        json!({ "metas": [
            { "id": "tt0000001", "type": "movie", "name": "Night Train" }
        ]}),
    );
    add(
        "/meta/catalog/series/popular/search=night.json".into(),
        json!({ "metas": [
            { "id": "tt0000010", "type": "series", "name": "Night Shift" }
        ]}),
    );
    add(
        "/meta/meta/movie/tt0000001.json".into(),
        json!({ "meta": {
            "id": "tt0000001", "type": "movie", "name": "Night Train", "year": "1999", "runtime": "1h 50min",
            "background": "https://img.example/bg.jpg", "description": "A train at night.", "imdbRating": "7.1",
            "cast": ["Ann Actor", "Ben Actor"], "director": "Dee Rector", "genres": ["Drama"],
            "released": "1999-05-01T00:00:00.000Z", "trailers": [{ "source": "yt123", "type": "Trailer" }]
        }}),
    );
    add(
        "/meta/meta/series/tt0000010.json".into(),
        json!({ "meta": {
            "id": "tt0000010", "type": "series", "name": "Night Shift", "writer": ["Wri Ter"],
            "videos": [
                { "id": "tt0000010:1:2", "title": "Second", "season": 1, "episode": 2, "released": "2010-01-08T00:00:00.000Z" },
                { "id": "tt0000010:1:1", "title": "First", "season": 1, "episode": 1, "released": "2010-01-01T00:00:00.000Z" },
                { "id": "tt0000010:9:1", "season": 9, "episode": 1, "released": "2999-01-01T00:00:00.000Z" }
            ]
        }}),
    );

    // A stream and subtitle add-on whose link carries a key.
    add(
        format!("{streams}/manifest.json"),
        json!({
            "id": "org.test.streams", "name": "Test Streams", "types": ["movie", "series"], "catalogs": [],
            "resources": [{ "name": "stream", "types": ["movie", "series"], "idPrefixes": ["tt"] }, "subtitles"]
        }),
    );
    add(
        format!("{streams}/stream/movie/tt0000001.json"),
        json!({ "streams": [
            { "name": "Test\n1080p", "description": "Night.Train.1999.1080p.BluRay.x264.DTS\n💾 2.5 GB",
              "url": format!("{base}/media/night.mp4"),
              "behaviorHints": { "proxyHeaders": { "request": { "User-Agent": "TestPlayer/1", "Referer": "http://ref.example" } } } },
            { "name": "Test\n4K", "title": "Night.Train.2160p.HDR", "infoHash": "abcdef" },
            { "name": "Trailer", "ytId": "xyz" },
            { "name": "Test\n1080p again", "url": format!("{base}/media/night.mp4") }
        ]}),
    );
    add(
        format!("{streams}/stream/series/tt0000010%3A1%3A1.json"),
        json!({ "streams": [
            { "name": "Test 720p", "url": format!("{base}/media/ep1.mkv") }
        ]}),
    );
    add(
        format!("{streams}/subtitles/movie/tt0000001.json"),
        json!({ "subtitles": [
            { "url": format!("{base}/subs/en.srt"), "lang": "eng" },
            { "lang": "fre" }
        ]}),
    );

    // An add-on that fails everything after installing.
    add(
        "/broken/manifest.json".into(),
        json!({
            "id": "org.test.broken", "name": "Broken", "types": ["movie"], "resources": ["stream", "meta"], "idPrefixes": ["tt"],
            "catalogs": [{ "type": "movie", "id": "top", "name": "Top" }]
        }),
    );

    // An add-on that answers with an explanation instead of streams.
    add(
        "/notice/manifest.json".into(),
        json!({
            "id": "org.test.notice", "name": "Notice", "types": ["movie"], "resources": ["stream"]
        }),
    );
    add(
        "/notice/stream/movie/tt0000099.json".into(),
        json!({ "streams": [
            { "name": "[Error] Notice", "description": "Your key expired." }
        ]}),
    );

    // An add-on of files that carry their own streams.
    add(
        "/files/manifest.json".into(),
        json!({
            "id": "org.test.files", "name": "Files", "types": ["other"], "resources": ["catalog", "meta"],
            "catalogs": [{ "type": "other", "id": "files", "name": "My Files" }]
        }),
    );
    add(
        "/files/catalog/other/files.json".into(),
        json!({ "metas": [
            { "id": "f1", "type": "other", "name": "Heat.1995.2160p.UHD.mkv" }
        ]}),
    );
    add(
        "/files/meta/other/f1.json".into(),
        json!({ "meta": {
            "id": "f1", "type": "other", "name": "Heat.1995.2160p.UHD.mkv",
            "videos": [{ "id": "f1:1", "title": "Heat.1995.2160p.UHD.mkv", "streams": [{ "url": format!("{base}/media/heat.mkv") }] }]
        }}),
    );

    // An add-on with nothing Xorvio can use.
    add(
        "/junk/manifest.json".into(),
        json!({
            "id": "org.test.junk", "name": "Junk", "types": ["tv"], "resources": [], "catalogs": [{ "type": "tv", "id": "x" }]
        }),
    );

    let app = Router::new().fallback(serve).with_state(Arc::new(routes));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Mock {
        base,
        secrets: Arc::new(MemorySecrets::new()),
        dir: tempfile::tempdir().unwrap(),
    }
}

fn addon_error(error: anyhow::Error) -> AddonError {
    error.downcast::<AddonError>().expect("an add-on error")
}

#[tokio::test]
async fn installs_persists_and_keeps_links_secret() {
    let mock = mock().await;
    let addons = mock.with_addons(&["meta", &format!("streams/{KEY}")]).await;
    let list = addons.list();
    assert_eq!(
        list.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["org.test.meta", "org.test.streams"]
    );
    let host = mock.base.trim_start_matches("http://");
    assert!(list.iter().all(|a| a.host == host && a.enabled));
    assert_eq!(list[0].catalogs.len(), 3);
    assert!(list[0].catalogs[0].browsable && !list[0].catalogs[2].browsable);
    assert_eq!(list[1].resources, ["stream", "subtitles"]);

    // The link is only in the secret store, never in addons.json.
    let saved = std::fs::read_to_string(mock.dir.path().join("addons.json")).unwrap();
    assert!(!saved.contains(KEY) && !saved.contains("/streams"));
    let link = mock
        .secrets
        .get("addon-link:org.test.streams")
        .unwrap()
        .unwrap();
    assert!(link.ends_with(KEY));

    // Reopening reads the same list and can still reach the add-ons.
    addons.move_addon("org.test.streams", 0).unwrap();
    addons.set_enabled("org.test.meta", false).unwrap();
    let reopened = mock.open();
    let list = reopened.list();
    assert_eq!(
        list.iter()
            .map(|a| (a.id.as_str(), a.enabled))
            .collect::<Vec<_>>(),
        [("org.test.streams", true), ("org.test.meta", false)]
    );
    assert_eq!(
        reopened.streams("movie", "tt0000001").await.unwrap().len(),
        2
    );
    reopened.refresh_manifests().await.unwrap();

    // Installing again updates in place; removing forgets the link.
    reopened
        .install(&mock.link(&format!("streams/{KEY}")))
        .await
        .unwrap();
    assert_eq!(reopened.list().len(), 2);
    reopened.remove("org.test.streams").unwrap();
    assert_eq!(reopened.list().len(), 1);
    assert!(
        mock.secrets
            .get("addon-link:org.test.streams")
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        addon_error(reopened.remove("org.test.streams").unwrap_err()),
        AddonError::NotInstalled
    ));
}

#[tokio::test]
async fn rejects_bad_and_useless_addons_without_leaking_links() {
    let mock = mock().await;
    let addons = mock.open();
    assert!(matches!(
        addon_error(addons.install("not a link").await.unwrap_err()),
        AddonError::BadAddress
    ));
    assert!(matches!(
        addon_error(addons.install(&mock.link("junk")).await.unwrap_err()),
        AddonError::NothingToUse(_)
    ));
    let missing = addons
        .install(&mock.link(&format!("nothing/{KEY}")))
        .await
        .unwrap_err();
    assert!(matches!(addon_error(missing), AddonError::NotFound(_)));

    // A port nothing listens on: the error names the host, never the path.
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let error = addons
        .install(&format!("http://{closed}/{KEY}/manifest.json"))
        .await
        .unwrap_err();
    let text = error.to_string();
    assert!(
        text.contains(&closed.to_string()) && !text.contains(KEY),
        "{text}"
    );
    assert!(addons.list().is_empty());
}

#[tokio::test]
async fn discovers_rows_and_pages_catalogs() {
    let mock = mock().await;
    let addons = mock
        .with_addons(&["meta", &format!("streams/{KEY}"), "broken", "files"])
        .await;
    let rows = addons.discover_rows().await.unwrap();
    let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
    // The broken add-on's row fails quietly; the search-only catalog isn't a row.
    assert_eq!(titles, ["Popular Films", "Popular Series", "My Files"]);
    let films: Vec<&str> = rows[0].movies.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(
        films,
        ["Night Train", "Day Boat"],
        "titles nothing can play are left out"
    );
    assert_eq!(rows[0].movies[0].id, "addon|movie|tt0000001");
    assert_eq!(rows[0].movies[1].year, Some(2001));
    assert_eq!(rows[1].series[0].remote_id.as_deref(), Some("tt0000010"));
    assert_eq!(rows[2].movies[0].title, "Heat");
    assert_eq!(rows[2].movies[0].year, Some(1995));
    assert_eq!(
        rows[2].movies[0].stream,
        StreamLocator::Addon {
            media_type: "other".into(),
            media_id: "f1".into()
        }
    );

    let page = addons
        .catalog("org.test.meta", "movie", "popular", Some("Drama"), 20)
        .await
        .unwrap();
    assert_eq!(page.title, "Popular Films · Drama");
    assert_eq!(page.movies[0].title, "Drama Film");
    assert!(
        addons
            .catalog("org.test.broken", "movie", "top", None, 0)
            .await
            .is_err()
    );
    assert!(matches!(
        addon_error(
            addons
                .catalog("nope", "movie", "x", None, 0)
                .await
                .unwrap_err()
        ),
        AddonError::NotInstalled
    ));
}

#[tokio::test]
async fn searches_every_catalog_that_can() {
    let mock = mock().await;
    let addons = mock.with_addons(&["meta", "broken"]).await;
    let (movies, series) = addons.search("  night ").await.unwrap();
    let films: Vec<&str> = movies.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(
        films,
        ["Night", "Night Train"],
        "exact match first, unrelated and duplicate titles dropped"
    );
    assert_eq!(series.len(), 1);
    assert!(addons.search("n").await.unwrap().0.is_empty());
}

#[tokio::test]
async fn maps_details_and_episodes() {
    let mock = mock().await;
    let addons = mock.with_addons(&["broken", "meta"]).await;
    let film = addons.meta("movie", "tt0000001").await.unwrap();
    let movie = film.movie.unwrap();
    assert_eq!(movie.id, "addon|movie|tt0000001");
    assert_eq!(movie.duration_seconds, Some(110 * 60));
    assert_eq!(movie.rating, Some(7.1));
    let details = film.details;
    assert_eq!(details.backdrop_urls, ["https://img.example/bg.jpg"]);
    assert_eq!(details.people.len(), 2);
    assert_eq!(details.director.as_deref(), Some("Dee Rector"));
    assert_eq!(details.trailer_youtube_id.as_deref(), Some("yt123"));
    assert_eq!(details.release_date.as_deref(), Some("1999-05-01"));
    assert_eq!(details.imdb_id.as_deref(), Some("tt0000001"));

    let show = addons
        .meta("series", "addon|series|tt0000010")
        .await
        .unwrap();
    assert_eq!(show.series.unwrap().id, "addon|series|tt0000010");
    assert_eq!(show.details.creators, ["Wri Ter"]);
    let episodes: Vec<(i64, i64, &str)> = show
        .episodes
        .iter()
        .map(|e| (e.season, e.number, e.title.as_str()))
        .collect();
    assert_eq!(episodes, [(1, 1, "First"), (1, 2, "Second")]);
    assert_eq!(
        show.episodes[0].id,
        "addon|series|tt0000010|ep|tt0000010:1:1"
    );

    assert!(matches!(
        addon_error(addons.meta("movie", "kitsu:1").await.unwrap_err()),
        AddonError::NoAddonFor(_)
    ));
}

#[tokio::test]
async fn lists_copies_from_every_stream_addon() {
    let mock = mock().await;
    let addons = mock
        .with_addons(&[&format!("streams/{KEY}"), "broken", "notice"])
        .await;
    let copies = addons.streams("movie", "tt0000001").await.unwrap();
    assert_eq!(
        copies.len(),
        2,
        "the YouTube entry and the duplicate are dropped"
    );
    let direct = &copies[0];
    assert_eq!(direct.addon_id, "org.test.streams");
    assert_eq!(direct.addon_name, "Test Streams");
    assert_eq!(direct.name, "Test");
    assert_eq!(direct.quality.as_deref(), Some("1080p"));
    assert_eq!(direct.size_bytes, Some((2.5 * 1024f64.powi(3)) as i64));
    assert_eq!(direct.tags, ["Blu-ray", "H.264", "DTS"]);
    let target = direct.target.as_ref().unwrap();
    assert_eq!(target.url, format!("{}/media/night.mp4", mock.base));
    assert_eq!(target.user_agent.as_deref(), Some("TestPlayer/1"));
    assert_eq!(
        target.headers,
        [("Referer".to_string(), "http://ref.example".to_string())]
    );
    assert!(!target.is_live);
    let torrent = &copies[1];
    assert!(torrent.target.is_none());
    assert_eq!(torrent.quality.as_deref(), Some("4K"));
    assert_eq!(torrent.tags, ["HDR"]);

    let episode = addons.streams("series", "tt0000010:1:1").await.unwrap();
    assert_eq!(episode[0].quality.as_deref(), Some("720p"));

    // Only a notice and a failure: the notice is the error.
    let error = addon_error(addons.streams("movie", "tt0000099").await.unwrap_err());
    assert_eq!(error, AddonError::Notice("Your key expired.".into()));
}

#[tokio::test]
async fn files_play_from_their_own_streams() {
    let mock = mock().await;
    let addons = mock.with_addons(&["files"]).await;
    let copies = addons.streams("other", "f1").await.unwrap();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].name, "Heat.1995.2160p.UHD.mkv");
    assert_eq!(copies[0].quality.as_deref(), Some("4K"));
    assert!(
        copies[0]
            .target
            .as_ref()
            .unwrap()
            .url
            .ends_with("/media/heat.mkv")
    );
}

#[tokio::test]
async fn subtitles_and_switched_off_addons() {
    let mock = mock().await;
    let addons = mock
        .with_addons(&[&format!("streams/{KEY}"), "broken"])
        .await;
    let subtitles = addons.subtitles("movie", "tt0000001").await.unwrap();
    assert_eq!(subtitles.len(), 1);
    assert_eq!(subtitles[0].language, "eng");
    assert_eq!(subtitles[0].addon_name, "Test Streams");

    addons.set_enabled("org.test.streams", false).unwrap();
    addons.set_enabled("org.test.broken", false).unwrap();
    assert!(
        addons
            .subtitles("movie", "tt0000001")
            .await
            .unwrap()
            .is_empty()
    );
    let error = addon_error(addons.streams("movie", "tt0000001").await.unwrap_err());
    assert!(matches!(error, AddonError::NoAddonFor("streams")));
    assert!(matches!(
        addon_error(
            addons
                .catalog("org.test.broken", "movie", "top", None, 0)
                .await
                .unwrap_err()
        ),
        AddonError::Disabled
    ));
}
