//! The media gateway against a local server serving generated media.

mod common;

use std::time::Duration;

use axum::Router;
use common::*;
use xorvio_media::{Gateway, PlayHint, StreamOptions};
use xorvio_model::{LiveStreamFormat, PlaybackMode};

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn text(url: &str) -> String {
    let response = client().get(url).send().await.unwrap();
    assert!(
        response.status().is_success(),
        "{} for {url}",
        response.status()
    );
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    response.text().await.unwrap()
}

/// The addresses in a playlist (its URI lines and URI="…" attributes).
fn addresses(playlist: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in playlist.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with('#') {
            if let Some((_, rest)) = line.split_once("URI=\"") {
                found.push(rest.split('"').next().unwrap().to_string());
            }
        } else {
            found.push(line.to_string());
        }
    }
    found
}

#[tokio::test]
async fn hls_playlists_and_segments_come_through_the_gateway() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("v0")).unwrap();
    let segments = dir.path().join("v0/seg_%03d.ts");
    let media = dir.path().join("v0/index.m3u8");
    let mut args: Vec<&str> = VIDEO_SOURCE.to_vec();
    args.extend(AUDIO_SOURCE);
    let (segments, media) = (segments.to_string_lossy(), media.to_string_lossy());
    args.extend([
        "-t",
        "4",
        "-c:v",
        "mpeg2video",
        "-g",
        "25",
        "-c:a",
        "aac",
        "-f",
        "hls",
        "-hls_time",
        "1",
    ]);
    args.extend([
        "-hls_list_size",
        "0",
        "-hls_segment_filename",
        &segments,
        &media,
    ]);
    ffmpeg(&tools, &args);
    let origin = serve(dir.path(), Router::new()).await;
    // One variant relative, one absolute, as providers mix them.
    std::fs::write(
        dir.path().join("master.m3u8"),
        format!(
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nv0/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=400000\n{origin}/files/v0/index.m3u8\n"
        ),
    )
    .unwrap();

    let gateway = Gateway::start(Some(tools.clone())).await.unwrap();
    let source = format!("{origin}/files/master.m3u8?token={KEY}");
    let stream = gateway
        .prepare(target(&source, false), PlayHint::default())
        .await
        .unwrap();
    assert_eq!(stream.mode, PlaybackMode::Hls);
    let port = origin.rsplit(':').next().unwrap();
    assert!(
        stream.url.starts_with("http://127.0.0.1:")
            && !stream.url.contains(KEY)
            && !stream.url.contains("/files/")
    );
    assert!(!stream.url.contains(&format!(":{port}/")));

    let master = text(&stream.url).await;
    let variants = addresses(&master);
    assert_eq!(variants.len(), 2);
    let gateway_prefix = stream.url.trim_end_matches("index.m3u8");
    assert!(
        variants
            .iter()
            .all(|v| v.starts_with(gateway_prefix) && v.ends_with(".m3u8")),
        "{master}"
    );
    assert_eq!(
        variants[0], variants[1],
        "the same playlist gets the same gateway address"
    );

    let playlist = text(&variants[0]).await;
    let segments = addresses(&playlist);
    assert!(segments.len() >= 3, "{playlist}");
    assert!(
        segments
            .iter()
            .all(|s| s.starts_with(gateway_prefix) && s.ends_with(".ts"))
    );
    let first = client()
        .get(&segments[0])
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(
        first.as_ref(),
        std::fs::read(dir.path().join("v0/seg_000.ts")).unwrap()
    );

    // A player that knows nothing of the provider's headers reads the whole stream.
    let probe = tokio::task::spawn_blocking({
        let (tools, url) = (tools.clone(), stream.url.clone());
        move || common::probe(&tools, &url)
    })
    .await
    .unwrap();
    assert!(probe.format.contains("hls"));
    assert!(
        (probe.duration - 4.0).abs() < 1.0,
        "duration {}",
        probe.duration
    );

    // Wrong token, and a released stream, are gone.
    let token = gateway_prefix.split('/').nth(3).unwrap();
    let wrong = stream.url.replace(token, "nope");
    assert_eq!(client().get(&wrong).send().await.unwrap().status(), 404);
    gateway.release(&stream.url);
    assert_eq!(
        client().get(&stream.url).send().await.unwrap().status(),
        404
    );
    assert_eq!(
        client().get(&segments[0]).send().await.unwrap().status(),
        404
    );
}

#[tokio::test]
async fn web_ready_files_play_natively_with_ranges() {
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx264") {
        eprintln!("ffmpeg has no libx264; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("film.mp4");
    let mut args: Vec<&str> = VIDEO_SOURCE.to_vec();
    args.extend(AUDIO_SOURCE);
    let path = file.to_string_lossy();
    args.extend([
        "-t",
        "3",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-c:a",
        "aac",
        "-movflags",
        "+faststart",
        &path,
    ]);
    ffmpeg(&tools, &args);
    let bytes = std::fs::read(&file).unwrap();
    let origin = serve(dir.path(), Router::new()).await;
    let gateway = Gateway::start(Some(tools)).await.unwrap();

    let stream = gateway
        .prepare(
            target(&format!("{origin}/files/film.mp4"), false),
            PlayHint::default(),
        )
        .await
        .unwrap();
    assert_eq!(stream.mode, PlaybackMode::Native);
    assert!((stream.duration_seconds.unwrap() - 3.0).abs() < 0.5);
    let response = client()
        .get(&stream.url)
        .header("range", "bytes=0-99")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 206);
    assert_eq!(
        response.headers()["content-range"],
        format!("bytes 0-99/{}", bytes.len())
    );
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    assert_eq!(response.bytes().await.unwrap().as_ref(), &bytes[..100]);

    // The same file from disk, by file:// address.
    let local = url::Url::from_file_path(&file).unwrap().to_string();
    let stream = gateway
        .prepare(target(&local, false), PlayHint::default())
        .await
        .unwrap();
    assert_eq!(stream.mode, PlaybackMode::Native);
    let response = client()
        .get(&stream.url)
        .header("range", "bytes=100-199")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 206);
    assert_eq!(response.bytes().await.unwrap().as_ref(), &bytes[100..200]);
}

#[tokio::test]
async fn live_streams_are_passed_through() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let ts = dir.path().join("channel.ts");
    make_ts(&tools, &ts, 2);
    std::fs::copy(&ts, dir.path().join("1234")).unwrap();
    std::fs::write(dir.path().join("5678"), "#EXTM3U\n#EXTINF:1,\nchannel.ts\n").unwrap();
    let origin = serve(dir.path(), Router::new()).await;
    let gateway = Gateway::start(Some(tools)).await.unwrap();

    let by_extension = gateway
        .prepare(
            target(&format!("{origin}/files/channel.ts"), true),
            PlayHint::default(),
        )
        .await
        .unwrap();
    assert_eq!(by_extension.mode, PlaybackMode::Mpegts);
    assert!(by_extension.is_live && by_extension.url.ends_with("/stream.ts"));
    let body = client()
        .get(&by_extension.url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(body.as_ref(), std::fs::read(&ts).unwrap());

    let hinted = PlayHint {
        live_format: Some(LiveStreamFormat::Hls),
        ..PlayHint::default()
    };
    let by_format = gateway
        .prepare(target(&format!("{origin}/files/1234"), true), hinted)
        .await
        .unwrap();
    assert_eq!(by_format.mode, PlaybackMode::Hls);

    // No extension and no hint: the first bytes decide.
    let sniffed_ts = gateway
        .prepare(
            target(&format!("{origin}/files/1234"), true),
            PlayHint::default(),
        )
        .await
        .unwrap();
    assert_eq!(sniffed_ts.mode, PlaybackMode::Mpegts);
    let sniffed_hls = gateway
        .prepare(
            target(&format!("{origin}/files/5678"), true),
            PlayHint::default(),
        )
        .await
        .unwrap();
    assert_eq!(sniffed_hls.mode, PlaybackMode::Hls);

    // Without the provider's headers the server refuses; through the gateway it doesn't.
    assert_eq!(
        client()
            .get(format!("{origin}/files/channel.ts"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
}

#[tokio::test]
async fn mkv_with_ac3_is_remuxed_to_mpegts_and_seeks() {
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx264") {
        eprintln!("ffmpeg has no libx264; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("film.mkv");
    let mut args: Vec<&str> = VIDEO_SOURCE.to_vec();
    args.extend(AUDIO_SOURCE);
    let path = file.to_string_lossy();
    args.extend([
        "-t",
        "6",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-g",
        "25",
        "-c:a",
        "ac3",
        "-ac",
        "2",
        &path,
    ]);
    ffmpeg(&tools, &args);
    let origin = serve(dir.path(), Router::new()).await;
    let gateway = Gateway::start(Some(tools.clone())).await.unwrap();

    let stream = gateway
        .prepare(
            target(&format!("{origin}/files/film.mkv"), false),
            PlayHint::default(),
        )
        .await
        .unwrap();
    assert_eq!(stream.mode, PlaybackMode::Remux);
    assert!((stream.duration_seconds.unwrap() - 6.0).abs() < 0.5);
    assert_eq!(stream.start_offset_seconds, 0.0);
    assert!(stream.url.ends_with(".ts"), "{}", stream.url);

    let fetch = |url: String, name: &'static str| {
        let dir = dir.path().to_path_buf();
        async move {
            let response = client().get(&url).send().await.unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()["content-type"], "video/mp2t");
            let out = dir.join(name);
            std::fs::write(&out, response.bytes().await.unwrap()).unwrap();
            out
        }
    };
    let whole = fetch(stream.url.clone(), "whole.ts").await;
    let summary = probe(&tools, &whole.to_string_lossy());
    assert_eq!(summary.format, "mpegts", "{summary:?}");
    assert_eq!(summary.video.as_deref(), Some("h264"));
    assert_eq!(summary.audio.as_deref(), Some("aac"), "AC-3 becomes AAC");
    assert!((summary.duration - 6.0).abs() < 0.6, "{summary:?}");

    let seeked = gateway.seek(&stream.url, 3.0).await.unwrap();
    assert_eq!(seeked.start_offset_seconds, 3.0);
    assert_ne!(seeked.url, stream.url);
    assert_eq!(
        client().get(&stream.url).send().await.unwrap().status(),
        404,
        "the old address stops"
    );
    let rest = fetch(seeked.url.clone(), "rest.ts").await;
    let summary = probe(&tools, &rest.to_string_lossy());
    assert_eq!(summary.format, "mpegts", "{summary:?}");
    // Copied video starts at a keyframe at or before 3 s; ffmpeg 7 (Debian's)
    // takes the one a second earlier, ffmpeg 9 the one at 3 s.
    assert!(
        summary.duration > 2.0 && summary.duration < 4.1,
        "{summary:?}"
    );

    // Releasing mid-stream ends the response (and ffmpeg with it).
    let mut response = client().get(&seeked.url).send().await.unwrap();
    gateway.release(&seeked.url);
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while let Ok(Some(_)) = response.chunk().await {}
    })
    .await;
    assert!(ended.is_ok(), "the stream should end once released");
    assert!(gateway.seek(&seeked.url, 1.0).await.is_err());
}

/// Reads a response until nothing more comes for `quiet` (false) or it ends (true).
async fn read_until_quiet(response: &mut reqwest::Response, into: &mut Vec<u8>, quiet: Duration) -> bool {
    loop {
        match tokio::time::timeout(quiet, response.chunk()).await {
            Ok(Ok(Some(chunk))) => into.extend_from_slice(&chunk),
            Ok(Ok(None)) => return true,
            Ok(Err(error)) => panic!("the stream broke off: {error}"),
            Err(_) => return false,
        }
    }
}

#[tokio::test]
async fn a_remuxed_film_keeps_only_a_minute_ahead_of_the_player() {
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx264") {
        eprintln!("ffmpeg has no libx264; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("film.mkv");
    let mut args: Vec<&str> = VIDEO_SOURCE.to_vec();
    args.extend(AUDIO_SOURCE);
    let path = file.to_string_lossy();
    args.extend(["-t", "150", "-c:v", "libx264", "-preset", "ultrafast", "-g", "25", "-c:a", "aac", &path]);
    ffmpeg(&tools, &args);
    let origin = serve(dir.path(), Router::new()).await;
    let gateway = Gateway::start(Some(tools.clone())).await.unwrap();
    let stream = gateway
        .prepare(target(&format!("{origin}/files/film.mkv"), false), PlayHint::default())
        .await
        .unwrap();
    assert_eq!(stream.mode, PlaybackMode::Remux);
    let sent = dir.path().join("sent.ts");
    let seconds = |bytes: &[u8]| {
        std::fs::write(&sent, bytes).unwrap();
        probe(&tools, &sent.to_string_lossy()).duration
    };

    // ffmpeg copies the whole film in moments; a minute of it comes, then
    // the gateway waits to hear where the player is.
    let mut response = client().get(&stream.url).send().await.unwrap();
    let mut got = Vec::new();
    assert!(!read_until_quiet(&mut response, &mut got, Duration::from_secs(2)).await, "it waits for the player");
    let first = seconds(&got);
    assert!((58.0..68.0).contains(&first), "a minute ahead of the start: {first}");

    gateway.report_position(&stream.url, 50.0);
    assert!(!read_until_quiet(&mut response, &mut got, Duration::from_secs(2)).await);
    let second = seconds(&got);
    assert!((108.0..118.0).contains(&second), "a minute ahead of 50 s: {second}");

    // A position past what was sent isn't this player's.
    gateway.report_position(&stream.url, 1000.0);
    assert!(!read_until_quiet(&mut response, &mut got, Duration::from_secs(1)).await);
    assert!((seconds(&got) - second).abs() < 0.5);

    gateway.report_position(&stream.url, 100.0);
    assert!(read_until_quiet(&mut response, &mut got, Duration::from_secs(10)).await, "the rest comes, to the end");
    let whole = seconds(&got);
    assert!((whole - 150.0).abs() < 1.0, "{whole}");
}

/// A one-caption PGS (Blu-ray subtitle) file: a white 400×80 box at
/// (760, 900) on a 1920×1080 canvas, shown from 1 s to 3 s. ffmpeg can read
/// PGS but not write it.
fn pgs_caption() -> Vec<u8> {
    fn segment(seconds: u32, kind: u8, data: &[u8]) -> Vec<u8> {
        let mut out = b"PG".to_vec();
        out.extend((seconds * 90_000).to_be_bytes());
        out.extend(0u32.to_be_bytes());
        out.push(kind);
        out.extend((data.len() as u16).to_be_bytes());
        out.extend(data);
        out
    }
    let be = |n: u16| n.to_be_bytes();
    let (x, y, width, height) = (760u16, 900u16, 400u16, 80u16);
    let composition = |number: u16, objects: &[u8]| [&be(1920)[..], &be(1080), &[0x10], &be(number), objects].concat();
    let shown = [&[0x80, 0, 0, 1][..], &be(0), &[0, 0], &be(x), &be(y)].concat();
    let window = [&[1, 0][..], &be(x), &be(y), &be(width), &be(height)].concat();
    let palette = [0, 0, 1, 235, 128, 128, 255];
    // One run of colour 1 per line, then the end of the line.
    let line = [&[0x00, 0xC0 | (width >> 8) as u8, width as u8, 1][..], &[0, 0]].concat();
    let picture = [&be(width)[..], &be(height), &line.repeat(height as usize)].concat();
    let object = [&be(0)[..], &[0, 0xC0], &(picture.len() as u32).to_be_bytes()[1..], &picture].concat();
    [
        segment(1, 0x16, &composition(0, &shown)),
        segment(1, 0x17, &window),
        segment(1, 0x14, &palette),
        segment(1, 0x15, &object),
        segment(1, 0x80, &[]),
        segment(3, 0x16, &composition(1, &[0x00, 0, 0, 0])),
        segment(3, 0x17, &window),
        segment(3, 0x80, &[]),
    ]
    .concat()
}

#[tokio::test]
async fn picture_subtitles_are_drawn_into_the_video() {
    let Some(tools) = tools() else { return };
    if !has_encoder(&tools, "libx264") {
        eprintln!("ffmpeg has no libx264; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let captions = dir.path().join("captions.sup");
    std::fs::write(&captions, pgs_caption()).unwrap();
    let file = dir.path().join("film.mkv");
    // Black 720p video with 1080p subtitles, as some releases have.
    ffmpeg(
        &tools,
        &[
            "-f", "lavfi", "-i", "color=c=black:size=1280x720:rate=25", "-f", "lavfi", "-i", "sine=frequency=440",
            "-i", &captions.to_string_lossy(), "-t", "5", "-map", "0:v", "-map", "1:a", "-map", "2:s",
            "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac", "-c:s", "copy", &file.to_string_lossy(),
        ],
    );
    let origin = serve(dir.path(), Router::new()).await;
    let gateway = Gateway::start(Some(tools.clone())).await.unwrap();
    let stream = gateway.prepare(target(&format!("{origin}/files/film.mkv"), false), PlayHint::default()).await.unwrap();
    let pictures = stream.subtitle_tracks.iter().find(|track| track.codec == "hdmv_pgs_subtitle").expect("the PGS track is listed");
    assert_eq!((pictures.url.as_deref(), stream.burned_subtitle), (None, None));

    let burned = gateway.restream(&stream.url, 0.0, StreamOptions { burn_subtitle: Some(pictures.index), ..Default::default() }).await.unwrap();
    assert_eq!(burned.burned_subtitle, Some(pictures.index));
    assert!(burned.info.as_ref().unwrap().notes.iter().any(|note| note == "Subtitles drawn into the picture"));
    let out = dir.path().join("burned.ts");
    std::fs::write(&out, client().get(&burned.url).send().await.unwrap().bytes().await.unwrap()).unwrap();

    // The box, scaled to 720p, is at about (507, 600)–(773, 653): white
    // while the caption shows, black once it's gone.
    let brightness = |frame: u32| {
        let output = std::process::Command::new(maker(&tools))
            .args(["-hide_banner", "-loglevel", "error", "-i", &out.to_string_lossy()])
            .args(["-vf", &format!("select=eq(n\\,{frame}),crop=200:30:540:610"), "-frames:v", "1"])
            .args(["-f", "rawvideo", "-pix_fmt", "gray", "-"])
            .output()
            .unwrap();
        assert!(!output.stdout.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
        output.stdout.iter().map(|&value| value as u64).sum::<u64>() / output.stdout.len() as u64
    };
    assert!(brightness(50) > 230, "the caption is drawn at 2 s");
    assert!(brightness(100) < 30, "and gone at 4 s");

    let plain = gateway.restream(&burned.url, 1.0, StreamOptions { burn_subtitle: Some(-1), ..Default::default() }).await.unwrap();
    assert_eq!(plain.burned_subtitle, None);
    let seeked = gateway.seek(&burned.url, 2.0).await;
    assert!(seeked.is_err(), "the old address stopped");
}

#[tokio::test]
async fn a_recording_still_being_written_is_followed() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.ts");
    let second = dir.path().join("second.ts");
    make_ts(&tools, &first, 2);
    make_ts(&tools, &second, 2);
    let growing = dir.path().join("Channel - Show - 2026-10-30 2000.ts");
    std::fs::copy(&first, &growing).unwrap();

    let gateway = Gateway::start(Some(tools.clone())).await.unwrap();
    let local = url::Url::from_file_path(&growing).unwrap().to_string();
    let stream = gateway
        .prepare(target(&local, false), PlayHint::default())
        .await
        .unwrap();
    assert_eq!(stream.mode, PlaybackMode::Remux);
    assert_eq!(
        stream.duration_seconds, None,
        "still growing: no length yet"
    );

    // More arrives while it plays.
    let (more, path) = (std::fs::read(&second).unwrap(), growing.clone());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(&more)
            .unwrap();
    });
    let body = client()
        .get(&stream.url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let out = dir.path().join("followed.ts");
    std::fs::write(&out, body).unwrap();
    let summary = probe(&tools, &out.to_string_lossy());
    assert_eq!(summary.format, "mpegts", "{summary:?}");
    assert_eq!(summary.audio.as_deref(), Some("aac"), "{summary:?}");
    assert!(
        summary.duration > 3.0,
        "the appended part was read too: {summary:?}"
    );
}

#[tokio::test]
async fn other_players_get_lasting_addresses_for_the_original_stream() {
    // A web-ready file plays natively, and without ffmpeg it isn't probed,
    // so nothing here needs ffmpeg.
    let dir = tempfile::tempdir().unwrap();
    let bytes: Vec<u8> = (0..20_000u32).map(|n| (n % 251) as u8).collect();
    std::fs::write(dir.path().join("film.mp4"), &bytes).unwrap();
    std::fs::write(dir.path().join("other.mkv"), &bytes).unwrap();
    let origin = serve(dir.path(), Router::new()).await;
    let gateway = Gateway::start(None).await.unwrap();
    let source = format!("{origin}/files/film.mp4?token={KEY}");

    let stream = gateway
        .prepare(target(&source, false), PlayHint::default())
        .await
        .unwrap();
    assert_eq!(stream.mode, PlaybackMode::Native);
    let external = gateway.external(&stream.url, "The Film (2024)").unwrap();
    assert_ne!(external, stream.url);
    assert!(
        external.starts_with("http://127.0.0.1:") && external.ends_with("/The%20Film%20(2024).mp4"),
        "{external}"
    );
    assert!(!external.contains(KEY) && !external.contains("/files/"));

    // The player letting its stream go leaves the other player's alone.
    gateway.release(&stream.url);
    assert!(gateway.external(&stream.url, "Gone").is_err());
    let response = client()
        .get(&external)
        .header("range", "bytes=100-199")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 206, "the provider's headers are added and ranges pass");
    assert_eq!(response.bytes().await.unwrap().as_ref(), &bytes[100..200]);

    // One Xorvio isn't playing, from a menu.
    let unopened = gateway
        .external_target(
            target(&format!("{origin}/files/other.mkv"), false),
            &PlayHint::default(),
            "Other",
        )
        .unwrap();
    assert!(unopened.ends_with("/Other.mkv"), "{unopened}");
    let body = client().get(&unopened).send().await.unwrap().bytes().await.unwrap();
    assert_eq!(body.as_ref(), &bytes[..]);

    // Zapping through many streams in Xorvio doesn't push them out…
    for _ in 0..80 {
        let played = gateway
            .prepare(target(&source, false), PlayHint::default())
            .await
            .unwrap();
        gateway.release(&played.url);
    }
    assert_eq!(client().get(&external).send().await.unwrap().status(), 200);
    // …but only so many are kept for other players.
    for n in 0..16 {
        gateway
            .external_target(target(&source, false), &PlayHint::default(), &format!("Copy {n}"))
            .unwrap();
    }
    assert_eq!(client().get(&external).send().await.unwrap().status(), 404);
}

#[tokio::test]
async fn other_players_get_rewritten_playlists_for_hls() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("live.m3u8"),
        "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nseg0.ts\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("seg0.ts"), b"segment bytes").unwrap();
    let origin = serve(dir.path(), Router::new()).await;
    let gateway = Gateway::start(None).await.unwrap();
    let channel = target(&format!("{origin}/files/live.m3u8"), true);

    let stream = gateway.prepare(channel, PlayHint::default()).await.unwrap();
    assert_eq!(stream.mode, PlaybackMode::Hls);
    let external = gateway.external(&stream.url, "Channel One").unwrap();
    gateway.release(&stream.url);
    assert!(external.ends_with("/index.m3u8"), "{external}");
    let playlist = text(&external).await;
    let segments = addresses(&playlist);
    assert_eq!(segments.len(), 1, "{playlist}");
    assert!(segments[0].starts_with(external.trim_end_matches("index.m3u8")));
    assert_eq!(text(&segments[0]).await, "segment bytes");

    // A channel whose source says it's HLS, though its address doesn't show it.
    let hinted = PlayHint {
        live_format: Some(LiveStreamFormat::Hls),
        ..PlayHint::default()
    };
    let unopened = gateway
        .external_target(target(&format!("{origin}/files/live.m3u8?x=1"), true), &hinted, "Channel One")
        .unwrap();
    assert!(unopened.ends_with("/index.m3u8"), "{unopened}");
    assert!(text(&unopened).await.starts_with("#EXTM3U"));
}

#[tokio::test]
async fn other_players_get_the_original_of_a_remuxed_stream() {
    let Some(tools) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("recording.ts");
    make_ts(&tools, &file, 2);
    let origin = serve(dir.path(), Router::new()).await;
    let gateway = Gateway::start(Some(tools)).await.unwrap();

    let stream = gateway
        .prepare(
            target(&format!("{origin}/files/recording.ts"), false),
            PlayHint::default(),
        )
        .await
        .unwrap();
    assert_eq!(stream.mode, PlaybackMode::Remux);
    let external = gateway.external(&stream.url, "A Recording").unwrap();
    assert!(external.ends_with("/A%20Recording.ts"), "{external}");
    let body = client().get(&external).send().await.unwrap().bytes().await.unwrap();
    assert_eq!(body.as_ref(), std::fs::read(&file).unwrap(), "the file as it is, not ffmpeg's remux");
}

#[tokio::test]
async fn a_refused_stream_says_how_it_was_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("channel.ts"), [0x47u8; 188 * 4]).unwrap();
    let origin = serve(dir.path(), Router::new()).await;
    let gateway = Gateway::start(None).await.unwrap();
    let live = |url: String, headers: bool| {
        let mut target = target(&url, true);
        if !headers {
            target.headers.clear();
        }
        target
    };

    let fine = gateway.prepare(live(format!("{origin}/files/channel.ts"), true), PlayHint::default()).await.unwrap();
    assert!(client().get(&fine.url).send().await.unwrap().status().is_success());
    assert_eq!(gateway.failure_status(&fine.url), None);

    // The provider turning the login down: its 403 comes through, and is kept.
    let refused = gateway.prepare(live(format!("{origin}/files/channel.ts"), false), PlayHint::default()).await.unwrap();
    assert_eq!(client().get(&refused.url).send().await.unwrap().status(), 403);
    assert_eq!(gateway.failure_status(&refused.url), Some(403));

    let gone = gateway.prepare(live(format!("{origin}/files/gone.ts"), true), PlayHint::default()).await.unwrap();
    assert_eq!(client().get(&gone.url).send().await.unwrap().status(), 404);
    assert_eq!(gateway.failure_status(&gone.url), Some(404));

    // Nothing listening: the gateway's own 502.
    let unreachable = gateway.prepare(live("http://127.0.0.1:9/live/1.ts".into(), true), PlayHint::default()).await.unwrap();
    assert_eq!(client().get(&unreachable.url).send().await.unwrap().status(), 502);
    assert_eq!(gateway.failure_status(&unreachable.url), Some(502));

    // Released, there's nothing to say.
    gateway.release(&refused.url);
    assert_eq!(gateway.failure_status(&refused.url), None);
}

#[tokio::test]
async fn trailers_play_from_a_page_the_gateway_serves() {
    let gateway = Gateway::start(None).await.unwrap();
    let page = gateway.trailer_page("dQw4w9WgXcQ").unwrap();
    assert!(page.starts_with("http://127.0.0.1:") && page.ends_with("/y/trailer/player.html?v=dQw4w9WgXcQ"), "{page}");
    let reply = client().get(&page).send().await.unwrap();
    assert_eq!(reply.status(), 200);
    assert_eq!(reply.headers()["content-type"], "text/html; charset=utf-8");
    // YouTube hears the gateway's origin, never the token in the path.
    assert_eq!(reply.headers()["referrer-policy"], "strict-origin-when-cross-origin");
    assert!(reply.text().await.unwrap().contains("https://www.youtube.com/iframe_api"));

    assert_eq!(gateway.trailer_page("../../etc"), None);
    assert_eq!(gateway.trailer_page("<script>"), None);
    // Without the gateway's token, there's nothing.
    let wrong = page.split('/').enumerate().map(|(i, part)| if i == 3 { "nope" } else { part }).collect::<Vec<_>>().join("/");
    assert_eq!(client().get(&wrong).send().await.unwrap().status(), 404);
}
