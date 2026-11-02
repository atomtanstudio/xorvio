//! Xorvio against a real Plex Media Server: the one `Scripts/test-servers/plex.sh start`
//! runs in Docker, with two films and a two-season show. Only Xorvio's public
//! API is used, as the app uses it. Ignored unless asked for, and skipped
//! when no server address is given:
//!
//!   XORVIO_PLEX_TEST_URL=http://127.0.0.1:32401 cargo test -p xorvio-core --test plex_docker -- --ignored
//!
//! The server is unclaimed and lets any address in without a token, so
//! nothing here signs in to plex.tv. The test leaves every title unwatched.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use xorvio_core::{BrowseQuery, Core, MemorySecrets, parse_artwork_path};
use xorvio_model::*;

/// A request straight to the server, as another Plex app would make it.
async fn plex(client: &reqwest::Client, base: &str, method: reqwest::Method, path: &str) -> Value {
    let response = client
        .request(method, format!("{base}{path}"))
        .header("Accept", "application/json")
        .header("X-Plex-Client-Identifier", "xorvio-docker-test")
        .header("X-Plex-Product", "Xorvio Test")
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{path}: {}", response.status());
    let text = response.text().await.unwrap();
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// (viewOffset in ms, viewCount) of an item on the server.
async fn watch_state(client: &reqwest::Client, base: &str, key: &str) -> (i64, i64) {
    let reply = plex(client, base, reqwest::Method::GET, &format!("/library/metadata/{key}")).await;
    let item = &reply["MediaContainer"]["Metadata"][0];
    (item["viewOffset"].as_i64().unwrap_or(0), item["viewCount"].as_i64().unwrap_or(0))
}

/// Waits for Xorvio's report (sent in the background) to reach the server.
async fn eventually(client: &reqwest::Client, base: &str, key: &str, wanted: impl Fn((i64, i64)) -> bool) -> (i64, i64) {
    let mut state = (0, 0);
    for _ in 0..50 {
        state = watch_state(client, base, key).await;
        if wanted(state) {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("the server never got there: {state:?}");
}

fn server_key(stream: &StreamLocator) -> String {
    match stream {
        StreamLocator::Server { key, .. } => key.clone(),
        other => panic!("not a server title: {other:?}"),
    }
}

fn played(item_id: &str, kind: MediaKind, source_id: &str, series_id: Option<&str>, position: f64, finished: bool) -> PlaybackProgress {
    PlaybackProgress {
        item_id: item_id.into(),
        kind,
        source_id: source_id.into(),
        series_id: series_id.map(str::to_string),
        position_seconds: position,
        duration_seconds: if kind == MediaKind::Movie { 360.0 } else { 180.0 },
        finished,
        updated_at: 0,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the Plex test server: Scripts/test-servers/plex.sh start"]
async fn a_plex_server_through_xorvio() {
    let Ok(address) = std::env::var("XORVIO_PLEX_TEST_URL") else {
        eprintln!("XORVIO_PLEX_TEST_URL isn't set, so the Plex server test was skipped");
        return;
    };
    let base = address.trim_end_matches('/').to_string();
    let client = reqwest::Client::new();
    let folder = tempfile::tempdir().unwrap();
    let core = Core::open(folder.path(), Arc::new(MemorySecrets::default())).unwrap();

    // Added by address, with no token: the server lets this network in.
    let added = core
        .add_source(NewSource {
            name: String::new(),
            kind: SourceKind::Plex,
            secrets: SourceSecrets { server_url: Some(address.clone()), ..Default::default() },
            file_path: None,
            live_format: LiveStreamFormat::default(),
            user_agent: None,
        })
        .await
        .unwrap();
    assert_eq!(added.name, "Xorvio Test Plex", "named after the server");
    let source = added.id.clone();
    let status = core.refresh_source(&source, &|_| {}).await.unwrap();
    assert_eq!(status.last_error, None);
    assert_eq!((status.movie_count, status.series_count), (2, 1));
    assert!(status.capabilities.movies && status.capabilities.series && !status.capabilities.live);

    // Start from nothing watched, whatever an earlier run left.
    let movies = core.movies(&BrowseQuery::default()).unwrap();
    let shows = core.series_list(&BrowseQuery::default()).unwrap();
    let episodes = core.episodes(&shows[0].id).await.unwrap();
    for stream in movies.iter().map(|movie| &movie.stream).chain(episodes.iter().map(|episode| &episode.stream)) {
        plex(&client, &base, reqwest::Method::PUT, &format!("/:/unscrobble?key={}&identifier=com.plexapp.plugins.library", server_key(stream))).await;
    }

    // MARK: The catalog
    let film_libraries = core.categories(CategoryKind::Movie).unwrap();
    let show_libraries = core.categories(CategoryKind::Series).unwrap();
    assert_eq!(film_libraries.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Films"]);
    assert_eq!(show_libraries.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Shows"]);
    let one = movies.iter().find(|movie| movie.title == "Test Film One").expect("film one");
    let two = movies.iter().find(|movie| movie.title == "Test Film Two").expect("film two");
    assert_eq!((one.year, one.rating, one.duration_seconds), (Some(2020), Some(7.4), Some(360)));
    assert_eq!(one.genre.as_deref(), Some("Drama, Science Fiction"));
    assert_eq!(one.plot.as_deref(), Some("A test pattern drifts across a field of colour."));
    assert_eq!(one.category_id.as_deref(), Some(film_libraries[0].id.as_str()));
    assert!(one.added_at.is_some());
    let StreamLocator::Server { part, extension, .. } = &one.stream else { panic!("{:?}", one.stream) };
    assert!(part.as_deref().is_some_and(|part| part.starts_with("/library/parts/")), "{part:?}");
    assert_eq!(extension.as_deref(), Some("mkv"));
    for artwork in [&one.poster_url, &one.backdrop_url] {
        assert!(artwork.as_deref().is_some_and(|url| url.starts_with("xorvio-server://") || url.starts_with("http://xorvio-server.")), "{artwork:?}");
    }

    let show = &shows[0];
    assert_eq!((show.title.as_str(), show.year), ("Test Show", Some(2019)));
    assert_eq!(show.category_id.as_deref(), Some(show_libraries[0].id.as_str()));
    assert!(show.remote_id.is_some());
    assert_eq!(episodes.iter().map(|e| (e.season, e.number)).collect::<Vec<_>>(), [(1, 1), (1, 2), (2, 1), (2, 2)]);
    assert!(episodes.iter().all(|e| e.duration_seconds == Some(180) && e.still_url.is_some()));

    // Artwork comes through Xorvio, sized by the server.
    let poster = url::Url::parse(one.poster_url.as_deref().unwrap()).unwrap();
    let (artwork_source, artwork_path) = parse_artwork_path(poster.path()).unwrap();
    assert_eq!(artwork_source, source);
    let (bytes, content_type) = core.artwork(&source, &artwork_path).await.unwrap();
    assert_eq!(content_type.as_deref(), Some("image/jpeg"));
    assert!(bytes.starts_with(&[0xFF, 0xD8]), "a JPEG");

    // MARK: Details
    let details = core.details(&one.id).await.unwrap();
    assert_eq!(details.director.as_deref(), Some("Ada Test"));
    assert_eq!(details.creators, ["Bea Script"]);
    let cast: Vec<(&str, Option<&str>)> = details.people.iter().map(|p| (p.name.as_str(), p.role.as_deref())).collect();
    assert_eq!(cast, [("Cam Lens", Some("The Pattern")), ("Dee Frame", Some("The Tone"))]);
    assert_eq!(details.genres, ["Drama", "Science Fiction"]);
    assert_eq!(details.release_date.as_deref(), Some("2020-05-01"));
    assert_eq!(details.age_rating.as_deref(), Some("PG"));
    assert_eq!(details.country.as_deref(), Some("Ireland"));
    assert_eq!(details.tagline.as_deref(), Some("Every pixel in its place."));
    assert_eq!(details.runtime_minutes, Some(6));
    assert_eq!(details.backdrop_urls.len(), 1);
    assert_eq!(details.video, Some(VideoFormat { width: 1280, height: 720, codec: Some("h264".into()), is_hdr: false }));
    assert_eq!(details.audio_channels, Some(2));
    let details = core.details(&two.id).await.unwrap();
    assert_eq!(details.video, Some(VideoFormat { width: 1920, height: 1080, codec: Some("hevc".into()), is_hdr: true }));
    assert_eq!(details.audio_channels, Some(6));
    let details = core.details(&show.id).await.unwrap();
    assert_eq!(details.network.as_deref(), Some("Xorvio Test Network"));
    assert_eq!(details.age_rating.as_deref(), Some("TV-PG"));
    assert_eq!(details.people.iter().map(|p| p.role.as_deref()).collect::<Vec<_>>(), [Some("Herself")]);
    let seasons: Vec<(i64, Option<i64>)> = details.seasons.iter().map(|s| (s.number, s.episode_count)).collect();
    assert_eq!(seasons, [(1, Some(2)), (2, Some(2))]);

    // MARK: Playing
    let target = core.stream_target(&source, &two.stream, false).unwrap();
    assert!(target.url.starts_with(&base) && target.url.contains("/library/parts/"), "{}", target.url);
    assert!(!target.url.contains("X-Plex-Token"));
    assert!(target.headers.iter().any(|(name, _)| name == "X-Plex-Client-Identifier"));
    // The file reads with its headers, as the gateway's ffmpeg reads it…
    let headers: String = target.headers.iter().map(|(name, value)| format!("{name}: {value}\r\n")).collect();
    match std::process::Command::new("ffprobe")
        .args(["-v", "error", "-headers", &headers, "-show_entries", "stream=codec_name", "-of", "csv=p=0", &target.url])
        .output()
    {
        Ok(probe) => {
            assert!(probe.status.success(), "ffprobe: {}", String::from_utf8_lossy(&probe.stderr));
            let codecs = String::from_utf8_lossy(&probe.stdout).split_whitespace().map(str::to_string).collect::<Vec<_>>();
            assert_eq!(codecs, ["hevc", "ac3"]);
        }
        Err(_) => eprintln!("ffprobe isn't installed, so the file wasn't probed"),
    }
    // …and in ranges, so seeking works.
    let mut ranged = client.get(&target.url).header("Range", "bytes=1000-1999");
    for (name, value) in &target.headers {
        ranged = ranged.header(name.as_str(), value.as_str());
    }
    let ranged = ranged.send().await.unwrap();
    assert_eq!(ranged.status().as_u16(), 206);
    assert!(ranged.headers().get("content-range").is_some());
    assert_eq!(ranged.bytes().await.unwrap().len(), 1000);

    // MARK: What's been watched, sent to the server
    let one_key = server_key(&one.stream);
    core.save_progress(played(&one.id, MediaKind::Movie, &source, None, 125.0, false)).unwrap();
    assert_eq!(eventually(&client, &base, &one_key, |(offset, _)| offset == 125_000).await, (125_000, 0));
    // Finished: watched once, however many times the credits report it.
    core.save_progress(played(&one.id, MediaKind::Movie, &source, None, 340.0, true)).unwrap();
    eventually(&client, &base, &one_key, |(offset, count)| offset == 0 && count == 1).await;
    core.save_progress(played(&one.id, MediaKind::Movie, &source, None, 350.0, true)).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(watch_state(&client, &base, &one_key).await, (0, 1), "counted once");

    let first = &episodes[0];
    let first_key = server_key(&first.stream);
    let first_played = played(&first.id, MediaKind::Episode, &source, Some(&show.id), 0.0, false);
    core.set_watched(std::slice::from_ref(&first_played), true).unwrap();
    eventually(&client, &base, &first_key, |(_, count)| count >= 1).await;
    core.set_watched(std::slice::from_ref(&first_played), false).unwrap();
    eventually(&client, &base, &first_key, |state| state == (0, 0)).await;

    // MARK: …and taken from it
    let third = &episodes[2];
    let third_key = server_key(&third.stream);
    plex(
        &client,
        &base,
        reqwest::Method::PUT,
        &format!("/:/timeline?ratingKey={third_key}&key=%2Flibrary%2Fmetadata%2F{third_key}&identifier=com.plexapp.plugins.library&state=stopped&time=95000&duration=180021"),
    )
    .await;
    assert_eq!(watch_state(&client, &base, &third_key).await, (95_000, 0));
    plex(&client, &base, reqwest::Method::PUT, &format!("/:/scrobble?key={}&identifier=com.plexapp.plugins.library", server_key(&two.stream))).await;
    assert!(core.progress(&third.id).unwrap().is_none());
    let status = core.refresh_source(&source, &|_| {}).await.unwrap();
    assert_eq!(status.last_error, None);
    let taken = core.progress(&third.id).unwrap().expect("the server's position");
    assert_eq!((taken.position_seconds, taken.finished, taken.series_id.as_deref()), (95.0, false, Some(show.id.as_str())));
    assert!(core.progress(&two.id).unwrap().is_some_and(|progress| progress.finished), "watched on the server");
    let continuing = core.continue_watching(20).unwrap();
    assert!(continuing.iter().any(|item| item.item_id == third.id), "{continuing:?}");
    // The refresh's lists (which name only the first few genres and actors)
    // leave the details fetched before as they were.
    let details = core.details(&one.id).await.unwrap();
    assert_eq!((details.people.len(), details.genres.len()), (2, 2));
    assert_eq!(details.director.as_deref(), Some("Ada Test"));
    assert!(details.video.is_some());

    // Leave the server as it was.
    for stream in movies.iter().map(|movie| &movie.stream).chain(episodes.iter().map(|episode| &episode.stream)) {
        plex(&client, &base, reqwest::Method::PUT, &format!("/:/unscrobble?key={}&identifier=com.plexapp.plugins.library", server_key(stream))).await;
    }
}

/// A server that wants a sign-in (`plex.sh lock`, as a claimed server does)
/// turns an address-only source down, saying plainly to sign in with Plex.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the locked Plex test server: Scripts/test-servers/plex.sh start, then lock"]
async fn a_plex_server_that_wants_a_sign_in() {
    let Ok(address) = std::env::var("XORVIO_PLEX_LOCKED_TEST_URL") else {
        eprintln!("XORVIO_PLEX_LOCKED_TEST_URL isn't set, so the locked Plex server test was skipped");
        return;
    };
    let folder = tempfile::tempdir().unwrap();
    let core = Core::open(folder.path(), Arc::new(MemorySecrets::default())).unwrap();
    let new = |token: Option<&str>| NewSource {
        name: String::new(),
        kind: SourceKind::Plex,
        secrets: SourceSecrets { server_url: Some(address.clone()), token: token.map(str::to_string), ..Default::default() },
        file_path: None,
        live_format: LiveStreamFormat::default(),
        user_agent: None,
    };
    let error = core.add_source(new(None)).await.unwrap_err().to_string();
    assert!(error.contains("Choose Sign in with Plex"), "{error}");
    let error = core.add_source(new(Some("not-a-real-token"))).await.unwrap_err().to_string();
    assert!(error.contains("didn't accept the sign-in") && !error.contains("not-a-real-token"), "{error}");
    assert!(core.list_sources().unwrap().is_empty(), "nothing was added");
}
