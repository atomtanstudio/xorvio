//! A media server's live TV through Core: its channels and guide read with
//! the catalog, played through the tuner relay, kept through a hiccup, left
//! out on request, and tunings a crash left behind let go.

use std::time::Duration;

use super::*;
use crate::sources::server::Release;
use crate::tuners::{HELD_PREFERENCE, Held};

fn fixture(name: &str) -> Value {
    let path = format!("{}/src/sources/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The test guide's programmes moved to start now, so the window keeps them.
fn programmes_from_now() -> Value {
    let mut page = fixture("jellyfin-livetv-programs");
    let start = this_minute();
    for (index, item) in page["Items"].as_array_mut().unwrap().iter_mut().enumerate() {
        let at = |offset: i64| chrono::DateTime::from_timestamp(offset, 0).unwrap().to_rfc3339();
        let from = start + (index as i64 / 3) * 1800;
        item["StartDate"] = json!(at(from));
        item["EndDate"] = json!(at(from + 1800));
    }
    page
}

/// A Jellyfin server with no libraries and the test servers' live TV:
/// three channels, a two-tuner M3U tuner, and a stream of a few packets.
fn jellyfin(request: &Request) -> (u16, Vec<u8>) {
    match request.path.as_str() {
        "/System/Info/Public" => ok_json(json!({"Id": "srv", "Version": "10.11.0", "ProductName": "Jellyfin Server", "ServerName": "Den"})),
        "/Users/AuthenticateByName" => ok_json(json!({"AccessToken": "jf-token-1", "User": {"Id": "u1"}})),
        "/UserViews" => ok_json(json!({"Items": []})),
        "/LiveTv/Channels" => ok_json(fixture("jellyfin-livetv-channels")),
        "/LiveTv/Programs" => ok_json(programmes_from_now()),
        "/System/Configuration/livetv" => ok_json(fixture("jellyfin-livetv-config")),
        path if path.ends_with("/PlaybackInfo") => ok_json(fixture("jellyfin-livetv-playbackinfo")),
        path if path.starts_with("/Videos/") => (200, [0x47u8; 188 * 20].to_vec()),
        "/LiveStreams/Close" => (204, Vec::new()),
        _ => (404, Vec::new()),
    }
}

async fn add_jellyfin(core: &Core, server: &Server) -> SourceConfig {
    let secrets = SourceSecrets {
        server_url: Some(server.base.clone()),
        username: Some("ann".into()),
        password: Some("pw".into()),
        ..Default::default()
    };
    let new = NewSource { name: String::new(), kind: SourceKind::Jellyfin, secrets, file_path: None, live_format: LiveStreamFormat::Hls, user_agent: None };
    core.add_source(new).await.unwrap()
}

fn status(core: &Core, id: &str) -> SourceStatus {
    core.list_sources().unwrap().into_iter().find(|source| source.config.id == id).unwrap().status
}

fn closes(server: &Server) -> Vec<Request> {
    server.requests().into_iter().filter(|request| request.path == "/LiveStreams/Close").collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_servers_channels_and_guide_play_through_the_relay() {
    let server = Server::start(jellyfin).await;
    let (_folder, core, _) = open_core();
    let config = add_jellyfin(&core, &server).await;
    assert!(config.include_live_tv, "on unless turned off");
    let refreshed = core.refresh_source(&config.id, &|_| {}).await.unwrap();
    assert_eq!((refreshed.last_error.as_deref(), refreshed.catalog_warning.as_deref()), (None, None));
    assert_eq!((refreshed.channel_count, refreshed.program_count, refreshed.max_connections), (3, 9, Some(2)));
    assert!(refreshed.capabilities.live && refreshed.capabilities.guide);

    let filter = ChannelFilter { source_id: Some(config.id.clone()), ..Default::default() };
    assert_eq!(channel_names(&core, filter.clone()), ["North Test News", "Harbor Test One", "Meadow Test Radio"]);
    let channels = core.channels(&filter).unwrap();
    let ids: Vec<String> = channels.iter().map(|channel| channel.id.clone()).collect();
    let rows = core.guide(&ids, now() - 600, now() + 3 * 3600).unwrap();
    assert_eq!(rows.iter().map(|row| row.programs.len()).collect::<Vec<_>>(), [3, 3, 3]);
    assert_eq!(rows[1].programs[0].title, "Harbor Lights");
    assert_eq!(rows[1].programs[0].episode.as_deref(), Some("S2 E1"));

    // Played through the relay, which tunes on the server.
    let target = core.stream_target(&config.id, &channels[0].stream, true).unwrap();
    assert!(target.url.starts_with("http://127.0.0.1:") && target.headers.is_empty() && target.is_live);
    assert!(!target.url.contains("jf-token-1"));
    let bytes = reqwest::get(&target.url).await.unwrap().bytes().await.unwrap();
    assert_eq!(bytes.len(), 188 * 20);
    let opened = server.requests().into_iter().filter(|request| request.path.ends_with("/PlaybackInfo")).count();
    assert_eq!(opened, 1);
    let read = server.requests().into_iter().find(|request| request.path.starts_with("/Videos/")).unwrap();
    assert_eq!(read.query.get("static").map(String::as_str), Some("true"));
    assert!(read.query["LiveStreamId"].ends_with("_e6c96939e3d045dda738acd61cbed83a"));

    // Quitting lets the tuner go there and then.
    core.let_go_of_tuners().await;
    assert_eq!(closes(&server).len(), 1);
    assert_eq!(core.library.preference(HELD_PREFERENCE).unwrap(), None, "nothing left noted as held");

    // A hiccup reading live TV keeps the channels and their guide, saying so.
    server.set(|request| match request.path.as_str() {
        "/LiveTv/Channels" => (500, Vec::new()),
        _ => jellyfin(request),
    });
    let kept = core.refresh_source(&config.id, &|_| {}).await.unwrap();
    assert_eq!((kept.channel_count, kept.program_count), (3, 9));
    assert!(kept.capabilities.live && kept.capabilities.guide);
    assert_eq!(
        kept.catalog_warning.as_deref(),
        Some("Live TV couldn't be read (The server responded with HTTP 500), so the channels from the last refresh are still shown.")
    );
    assert_eq!(core.guide(&ids, now() - 600, now() + 3 * 3600).unwrap()[0].programs.len(), 3);

    // Left out: the channels and the guide go.
    server.set(jellyfin);
    core.update_source(SourceConfig { include_live_tv: false, ..config.clone() }, None).unwrap();
    let without = core.refresh_source(&config.id, &|_| {}).await.unwrap();
    assert_eq!((without.channel_count, without.program_count, without.max_connections), (0, 0, None));
    assert!(!without.capabilities.live && !without.capabilities.guide);
    assert!(core.channels(&filter).unwrap().is_empty());

    // Back on, and the guide alone read again.
    core.update_source(SourceConfig { include_live_tv: true, ..config.clone() }, None).unwrap();
    core.refresh_source(&config.id, &|_| {}).await.unwrap();
    let guide_only = core.refresh_guide(&config.id).await.unwrap();
    assert_eq!((guide_only.program_count, guide_only.guide_last_error.as_deref()), (9, None));
    assert_eq!(status(&core, &config.id).channel_count, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn tunings_left_by_a_crash_are_let_go_once() {
    let server = Server::start(jellyfin).await;
    let (_folder, core, _) = open_core();
    let config = add_jellyfin(&core, &server).await;
    let left = |id: &str| {
        let held = vec![Held {
            source_id: config.id.clone(),
            release: Release::LiveStream { live_stream_id: id.into(), play_session: Some("ps-9".into()) },
        }];
        core.library.set_preference(HELD_PREFERENCE, Some(&serde_json::to_string(&held).unwrap())).unwrap();
    };

    left("ls-9");
    core.let_go_of_abandoned_tuners().await;
    let closed = closes(&server);
    assert_eq!(closed.len(), 1);
    assert_eq!((closed[0].query["LiveStreamId"].as_str(), closed[0].query["PlaySessionId"].as_str()), ("ls-9", "ps-9"));
    core.let_go_of_abandoned_tuners().await;
    assert_eq!(closes(&server).len(), 1, "never twice: the server counts each");

    // When a channel plays before that runs, the relay lets them go as it starts.
    left("ls-10");
    let channel = core.channels(&ChannelFilter::default()).unwrap().into_iter().next();
    let locator = channel.map_or(StreamLocator::ServerChannel { key: "k".into(), dvr: None }, |channel| channel.stream);
    core.stream_target(&config.id, &locator, true).unwrap();
    for _ in 0..50 {
        if closes(&server).len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(closes(&server).last().map(|request| request.query["LiveStreamId"].clone()).as_deref(), Some("ls-10"));
    core.let_go_of_abandoned_tuners().await;
    assert_eq!(closes(&server).len(), 2);
}
