//! Streams for a TV, against a local server serving generated media. The
//! "TV" is this test, asking from 127.0.0.1, so the listener opens on the
//! loopback; nothing is opened on the network and no device is contacted.

mod common;

use std::net::IpAddr;
use std::time::Duration;

use axum::Router;
use common::*;
use xorvio_media::{Gateway, PlayHint};

const TV: [IpAddr; 1] = [IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)];

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// Test pictures and tone into `name`, with these codec arguments.
fn make(tools: &xorvio_media::Tools, dir: &tempfile::TempDir, name: &str, seconds: u32, codecs: &[&str]) -> std::path::PathBuf {
    let file = dir.path().join(name);
    let path = file.to_string_lossy().into_owned();
    let duration = seconds.to_string();
    let mut args: Vec<&str> = VIDEO_SOURCE.to_vec();
    args.extend(AUDIO_SOURCE);
    args.extend(["-t", &duration]);
    args.extend(codecs);
    args.push(&path);
    ffmpeg(tools, &args);
    file
}

fn has_libx264(tools: &xorvio_media::Tools) -> bool {
    let found = has_encoder(tools, "libx264");
    if !found {
        eprintln!("ffmpeg has no libx264; skipping");
    }
    found
}

#[tokio::test]
async fn web_files_go_to_the_tv_as_they_are() {
    let Some(tools) = tools() else { return };
    if !has_libx264(&tools) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = make(&tools, &dir, "film.mp4", 3, &["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac", "-movflags", "+faststart"]);
    let bytes = std::fs::read(&file).unwrap();
    let origin = serve(dir.path(), Router::new()).await;
    let gateway = Gateway::start(Some(tools)).await.unwrap();

    let media = gateway.cast(target(&format!("{origin}/files/film.mp4"), false), PlayHint::default(), 1.5, &TV).await.unwrap();
    assert_eq!(media.content_type, "video/mp4");
    assert!(!media.live && !media.seeks_by_restart);
    assert_eq!((media.start_seconds, media.offset_seconds), (1.5, 0.0), "the TV starts where the film was");
    assert!((media.duration_seconds.unwrap() - 3.0).abs() < 0.5);
    let listener = gateway.cast_origin().unwrap();
    assert!(media.url.starts_with(&listener), "{} on {listener}", media.url);
    assert!(!media.url.contains("files/film") && !media.url.contains(&origin), "the provider's address stays here");

    let response = client().get(&media.url).header("range", "bytes=0-99").send().await.unwrap();
    assert_eq!(response.status(), 206, "the TV seeks with ranges");
    assert_eq!(response.headers()["content-type"], "video/mp4");
    assert_eq!(response.headers()["access-control-allow-origin"], "*", "the receiver's page reads it");
    assert_eq!(response.bytes().await.unwrap().as_ref(), &bytes[..100]);
    assert_eq!(gateway.cast_requests(), 1);

    gateway.cast_end();
    assert!(gateway.cast_origin().is_none());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(client().get(&media.url).send().await.is_err(), "the listener is closed once casting stops");
}

#[tokio::test]
async fn only_the_tvs_streams_are_served_and_only_to_the_tv() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("film.mp4"), b"not really a film").unwrap();
    let origin = serve(dir.path(), Router::new()).await;
    let gateway = Gateway::start(Some(tools)).await.unwrap();
    let film = target(&format!("{origin}/files/film.mp4"), false);

    // A stream the player has, before anything is cast.
    let playing = gateway.prepare(film.clone(), PlayHint { extension: Some("mp4".into()), ..PlayHint::default() }).await.unwrap();
    assert!(gateway.cast_origin().is_none(), "nothing listens on the network until something is cast");

    let media = gateway.cast(film.clone(), PlayHint { extension: Some("mp4".into()), ..PlayHint::default() }, 0.0, &TV).await.unwrap();
    let url = url::Url::parse(&media.url).unwrap();
    let parts: Vec<&str> = url.path_segments().unwrap().collect();
    let (token, kind, id) = (parts[0], parts[1], parts[2]);
    let listener = gateway.cast_origin().unwrap();
    let status = |path: String| {
        let url = format!("{listener}/{path}");
        async move { client().get(url).send().await.unwrap().status().as_u16() }
    };

    assert_eq!(status(format!("{token}/{kind}/{id}/video.mp4")).await, 200);
    assert_eq!(status(format!("wrong{token}/{kind}/{id}/video.mp4")).await, 404, "a token is needed");
    assert_eq!(status(format!("{token}/{kind}/{}/video.mp4", "0".repeat(32))).await, 404, "an unknown stream");
    assert_eq!(status(format!("{token}/m/{id}/video.mp4")).await, 404, "the stream's own kind only");
    assert_eq!(status(format!("{token}/{kind}/{id}")).await, 404, "nothing else is routed");
    assert_eq!(status("".to_string()).await, 404);

    // The player's own stream isn't reachable on the network, with either token.
    let player = url::Url::parse(&playing.url).unwrap();
    let player_parts: Vec<&str> = player.path_segments().unwrap().collect();
    assert_eq!(status(format!("{token}/s/{}/stream.mp4", player_parts[2])).await, 404);
    assert_eq!(status(format!("{}/s/{}/stream.mp4", player_parts[0], player_parts[2])).await, 404);
    // And the TV's token means nothing to the player's gateway.
    let local_origin = format!("http://127.0.0.1:{}", player.port().unwrap());
    let response = client().get(format!("{local_origin}/{token}/{kind}/{id}/video.mp4")).send().await.unwrap();
    assert_eq!(response.status(), 404);

    let posted = client().post(&media.url).send().await.unwrap();
    assert_eq!(posted.status(), 405);
    let preflight = client().request(reqwest::Method::OPTIONS, &media.url).send().await.unwrap();
    assert_eq!(preflight.status(), 204);
    assert_eq!(preflight.headers()["access-control-allow-private-network"], "true");

    // Casting to another TV: this computer (127.0.0.1) isn't it, so it's refused.
    let elsewhere: [IpAddr; 1] = ["127.0.0.2".parse().unwrap()];
    let before = gateway.cast_requests();
    let media = gateway.cast(film, PlayHint { extension: Some("mp4".into()), ..PlayHint::default() }, 0.0, &elsewhere).await.unwrap();
    assert_eq!(client().get(&media.url).send().await.unwrap().status(), 404, "only the TV is answered");
    assert_eq!(gateway.cast_requests(), before, "a refused request isn't the TV's");
    gateway.cast_end();
}

#[tokio::test]
async fn other_films_are_remuxed_to_fragmented_mp4_and_seek_by_restarting() {
    let Some(tools) = tools() else { return };
    if !has_libx264(&tools) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    make(&tools, &dir, "film.mkv", 6, &["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-g", "25", "-c:a", "ac3", "-ac", "2"]);
    let origin = serve(dir.path(), Router::new()).await;
    let gateway = Gateway::start(Some(tools.clone())).await.unwrap();

    // Copied video starts at the keyframe before (one a second here), so
    // seeks fall between them: Ubuntu 22.04's ffmpeg puts them 5 ms late,
    // and a seek to 2.0 would start from the one at 1.005.
    let media = gateway.cast(target(&format!("{origin}/files/film.mkv"), false), PlayHint::default(), 2.5, &TV).await.unwrap();
    assert_eq!(media.content_type, "video/mp4");
    assert!(media.seeks_by_restart);
    assert_eq!((media.offset_seconds, media.start_seconds), (2.5, 0.0), "ffmpeg starts where the film was");
    assert!((media.duration_seconds.unwrap() - 3.5).abs() < 0.5, "{media:?}");
    assert_eq!(media.notes, ["Audio converted to AAC stereo"]);

    let fetch = |url: String, name: &'static str| {
        let dir = dir.path().to_path_buf();
        async move {
            let response = client().get(&url).send().await.unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()["content-type"], "video/mp4");
            assert_eq!(response.headers()["accept-ranges"], "none");
            let out = dir.join(name);
            std::fs::write(&out, response.bytes().await.unwrap()).unwrap();
            out
        }
    };
    let rest = fetch(media.url.clone(), "rest.mp4").await;
    let summary = probe(&tools, &rest.to_string_lossy());
    assert!(summary.format.contains("mp4"), "{summary:?}");
    assert_eq!(summary.video.as_deref(), Some("h264"), "H.264 is copied");
    assert_eq!(summary.audio.as_deref(), Some("aac"), "AC-3 becomes AAC");
    assert!(summary.duration > 3.0 && summary.duration < 4.6, "{summary:?}");

    let seeked = gateway.cast_seek(&media.url, 4.5).await.unwrap();
    assert_eq!(seeked.offset_seconds, 4.5);
    assert_ne!(seeked.url, media.url);
    assert_eq!(client().get(&media.url).send().await.unwrap().status(), 404, "the old address stops");
    let end = fetch(seeked.url.clone(), "end.mp4").await;
    let summary = probe(&tools, &end.to_string_lossy());
    assert!(summary.duration > 1.0 && summary.duration < 2.6, "{summary:?}");

    // Stopping mid-stream ends the response, and ffmpeg with it.
    let mut response = client().get(&seeked.url).send().await.unwrap();
    gateway.cast_end();
    let ended = tokio::time::timeout(Duration::from_secs(5), async { while let Ok(Some(_)) = response.chunk().await {} }).await;
    assert!(ended.is_ok(), "the stream ends once casting stops");
    assert!(gateway.cast_seek(&seeked.url, 1.0).await.is_err());
}

#[tokio::test]
async fn live_channels_go_through_a_buffer_as_hls() {
    let Some(tools) = tools() else { return };
    if !has_libx264(&tools) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ts = dir.path().join("channel.ts");
    make_ts(&tools, &ts, 30);
    let paced = Paced::new(&ts, 30.0, None);
    let origin = serve(dir.path(), paced.clone().route("/live/1.ts")).await;
    let gateway = Gateway::start(Some(tools)).await.unwrap();
    let channel = target(&format!("{origin}/live/1.ts"), true);

    // The player had the channel open; the provider allows one connection.
    let playing = gateway.prepare(channel.clone(), PlayHint::default()).await.unwrap();
    let media = gateway.cast(channel, PlayHint::default(), 600.0, &TV).await.unwrap();
    assert_eq!(client().get(&playing.url).send().await.unwrap().status(), 404, "the player's stream of it is released");
    assert!(media.live && media.hls_fmp4);
    assert_eq!(media.content_type, "application/x-mpegurl");
    assert_eq!(media.start_seconds, 0.0, "live starts at live");
    assert!(media.url.ends_with("/index.m3u8"), "{}", media.url);
    assert!(media.notes.contains(&"Video encoded to H.264".to_string()), "MPEG-2 isn't for every TV: {:?}", media.notes);

    let response = client().get(&media.url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    let playlist = response.text().await.unwrap();
    assert!(playlist.starts_with("#EXTM3U") && playlist.contains("init.mp4"), "{playlist}");
    let segment = playlist.lines().find(|line| line.ends_with(".m4s")).expect("a segment");
    let base = media.url.trim_end_matches("index.m3u8");
    for part in ["init.mp4", segment] {
        let response = client().get(format!("{base}{part}")).send().await.unwrap();
        assert_eq!(response.status(), 200, "{part}");
        assert_eq!(response.headers()["content-type"], "video/mp4");
    }
    assert_eq!(client().get(format!("{base}../../etc/passwd")).send().await.unwrap().status(), 404);
    assert_eq!(paced.requests.load(std::sync::atomic::Ordering::SeqCst), 1, "one connection to the provider");
    gateway.cast_end();
}

#[tokio::test]
async fn what_cant_be_cast_says_so_before_anything_opens() {
    let with = Gateway::start(tools()).await.unwrap();
    let without = Gateway::start(None).await.unwrap();
    let hint = PlayHint::default();
    let live = target("http://provider.example/live/u/p/1.ts", true);
    let film = target("http://provider.example/movie/u/p/1.mkv", false);
    let mp4 = target("http://provider.example/movie/u/p/2.mp4", false);
    let growing = target(if cfg!(windows) { r"C:\Recordings\News.ts" } else { "/recordings/News.ts" }, true);
    let rtmp = target("rtmp://provider.example/live/1", true);

    assert_eq!(without.cast_check(&live, &hint).as_deref(), Some("Xorvio needs ffmpeg to send live channels to a TV."));
    assert_eq!(without.cast_check(&film, &hint).as_deref(), Some("Xorvio needs ffmpeg to send this to a TV."));
    assert_eq!(without.cast_check(&mp4, &hint), None, "a web file needs no ffmpeg");
    assert_eq!(with.cast_check(&growing, &hint).as_deref(), Some("A recording still being made can go to a TV once it's finished."));
    assert_eq!(with.cast_check(&rtmp, &hint).as_deref(), Some("Xorvio can't send this kind of stream to a TV."));
    let error = without.cast(film, hint, 0.0, &TV).await.unwrap_err();
    assert_eq!(error.to_string(), "Xorvio needs ffmpeg to send this to a TV.");
    assert!(without.cast_origin().is_none(), "nothing opened for a stream that can't go");
}
