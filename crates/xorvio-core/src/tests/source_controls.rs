//! What the Sources screen and its sheets do besides adding and refreshing:
//! putting sources in order, stopping a refresh, trying an Xtream login,
//! keeping a dropped playlist, and the PIN locking again by itself.

use std::time::Duration;

use super::*;

/// A playlist of channels, one per (group, name).
fn playlist(base: &str, channels: &[(&str, &str)]) -> String {
    let mut text = String::from("#EXTM3U\n");
    for (group, name) in channels {
        let file = name.to_lowercase().replace(' ', "-");
        text.push_str(&format!("#EXTINF:-1 tvg-id=\"{file}\" group-title=\"{group}\",{name}\n{base}/live/{file}.m3u8\n"));
    }
    text
}

/// An M3U file source holding `text`, refreshed.
async fn file_source(core: &Core, folder: &std::path::Path, name: &str, text: &str) -> SourceConfig {
    let path = folder.join(format!("{name}.m3u"));
    std::fs::write(&path, text).unwrap();
    let source = core
        .add_source(NewSource {
            name: name.into(),
            kind: SourceKind::M3uFile,
            secrets: SourceSecrets::default(),
            file_path: Some(path.to_string_lossy().into_owned()),
            live_format: LiveStreamFormat::Hls,
            user_agent: None,
        })
        .await
        .unwrap();
    core.refresh_source(&source.id, &|_| {}).await.unwrap();
    source
}

fn source_names(core: &Core) -> Vec<String> {
    core.list_sources().unwrap().into_iter().map(|summary| summary.config.name).collect()
}

#[tokio::test]
async fn sources_keep_the_order_they_are_put_in() {
    let (folder, core, secrets) = open_core();
    let lists = tempfile::tempdir().unwrap();
    let first = file_source(&core, lists.path(), "First", &playlist("http://a.example", &[("News", "Alpha One"), ("News", "Alpha Two")])).await;
    let second = file_source(&core, lists.path(), "Second", &playlist("http://b.example", &[("Sport", "Bravo One")])).await;
    let third = file_source(&core, lists.path(), "Third", &playlist("http://c.example", &[("Films", "Charlie One")])).await;
    assert_eq!(channel_names(&core, ChannelFilter::default()), ["Alpha One", "Alpha Two", "Bravo One", "Charlie One"]);

    // Live TV lists channels source by source, in the sources' order.
    core.reorder_sources(&[third.id.clone(), first.id.clone(), second.id.clone()]).unwrap();
    assert_eq!(source_names(&core), ["Third", "First", "Second"]);
    assert_eq!(channel_names(&core, ChannelFilter::default()), ["Charlie One", "Alpha One", "Alpha Two", "Bravo One"]);
    let indexes: Vec<i64> = core.list_sources().unwrap().iter().map(|summary| summary.config.sort_index).collect();
    assert_eq!(indexes, [0, 1, 2]);

    // Ones left out keep their order after the ones given; unknown ids change nothing.
    core.reorder_sources(&["gone".into(), second.id.clone()]).unwrap();
    assert_eq!(source_names(&core), ["Second", "Third", "First"]);

    // A new source goes last, and the order lasts through a restart.
    let fourth = file_source(&core, lists.path(), "Fourth", &playlist("http://d.example", &[("Kids", "Delta One")])).await;
    assert_eq!(source_names(&core), ["Second", "Third", "First", "Fourth"]);
    drop(core);
    let core = Core::open(folder.path(), secrets).unwrap();
    assert_eq!(source_names(&core), ["Second", "Third", "First", "Fourth"]);
    assert_eq!(channel_names(&core, ChannelFilter::default()), ["Bravo One", "Charlie One", "Alpha One", "Alpha Two", "Delta One"]);
    assert_eq!(channel_names(&core, ChannelFilter { source_id: Some(fourth.id), ..Default::default() }), ["Delta One"]);
}

/// A server that takes connections and never answers, standing in for a
/// provider that's slow to send a big catalog. Resolves once it's asked.
async fn silent_server() -> (String, tokio::sync::oneshot::Receiver<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (asked, heard) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let mut asked = Some(asked);
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
            if let Some(asked) = asked.take() {
                let _ = asked.send(());
            }
        }
    });
    (base, heard)
}

/// Starts a refresh, waits until it's waiting on `asked`, stops it, and
/// returns what the refresh returned.
async fn stop_refresh_while_waiting(core: &Arc<Core>, id: &str, asked: tokio::sync::oneshot::Receiver<()>) -> SourceStatus {
    let running = {
        let (core, id) = (core.clone(), id.to_string());
        tokio::spawn(async move { core.refresh_source(&id, &|_| {}).await })
    };
    tokio::time::timeout(Duration::from_secs(10), asked).await.expect("the refresh asked the server").unwrap();
    assert!(core.list_sources().unwrap()[0].refreshing);
    assert!(core.cancel_refresh(id));
    let status = tokio::time::timeout(Duration::from_secs(5), running).await.expect("stopping is quick").unwrap().unwrap();
    assert!(!core.list_sources().unwrap()[0].refreshing);
    assert!(!core.cancel_refresh(id), "nothing left to stop");
    status
}

#[tokio::test]
async fn a_cancelled_refresh_keeps_the_catalog() {
    let server = Server::start(|_| (404, Vec::new())).await;
    let list = playlist(&server.base, &[("News", "Harbor News"), ("Sport", "Summit Sports")]);
    let listings = guide(this_minute(), &[("harbor-news", "Harbor News")], &[("harbor-news", "Morning Brief", -30, "News"), ("harbor-news", "Late Edition", 30, "News")]);
    server.set(move |request| match request.path.as_str() {
        "/list.m3u" => (200, list.clone().into_bytes()),
        "/guide.xml" => (200, listings.clone().into_bytes()),
        _ => (404, Vec::new()),
    });
    let (_folder, core, _) = open_core();
    let mut secrets = SourceSecrets {
        playlist_url: Some(format!("{}/list.m3u", server.base)),
        epg_url: Some(format!("{}/guide.xml", server.base)),
        ..Default::default()
    };
    let source = core
        .add_source(NewSource {
            name: "Provider".into(),
            kind: SourceKind::M3uUrl,
            secrets: secrets.clone(),
            file_path: None,
            live_format: LiveStreamFormat::Hls,
            user_agent: None,
        })
        .await
        .unwrap();
    let before = core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert_eq!((before.channel_count, before.program_count), (2, 2));
    assert!(!core.cancel_refresh(&source.id), "nothing to stop between refreshes");

    // Stopped while the playlist downloads: nothing changes, and it isn't a failure.
    let (silent, asked) = silent_server().await;
    secrets.playlist_url = Some(format!("{silent}/list.m3u"));
    core.update_source(source.clone(), Some(secrets.clone())).unwrap();
    let returned = stop_refresh_while_waiting(&core, &source.id, asked).await;
    assert_eq!(returned, before);
    assert_eq!(core.list_sources().unwrap()[0].status, before, "the status isn't touched either");
    assert_eq!(channel_names(&core, ChannelFilter::default()), ["Harbor News", "Summit Sports"]);
    let harbor = core.channels(&ChannelFilter::default()).unwrap()[0].id.clone();
    assert_eq!(core.now_next(std::slice::from_ref(&harbor), now()).unwrap()[0].programs.len(), 2);

    // Stopped while the guide downloads: the new catalog is kept, the guide stays as it was.
    let (silent, asked) = silent_server().await;
    secrets.playlist_url = Some(format!("{}/list.m3u", server.base));
    secrets.epg_url = Some(format!("{silent}/guide.xml"));
    core.update_source(source.clone(), Some(secrets.clone())).unwrap();
    let returned = stop_refresh_while_waiting(&core, &source.id, asked).await;
    assert!(returned.last_success >= before.last_success);
    assert_eq!((returned.last_error, returned.guide_last_error), (None, None));
    assert_eq!((returned.channel_count, returned.program_count, returned.guide_last_success), (2, 2, before.guide_last_success));
    assert_eq!(core.now_next(&[harbor], now()).unwrap()[0].programs.len(), 2);

    // A refresh after a stop runs as usual.
    secrets.epg_url = Some(format!("{}/guide.xml", server.base));
    core.update_source(source.clone(), Some(secrets)).unwrap();
    let after = core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert_eq!((after.last_error, after.program_count), (None, 2));
}

#[tokio::test]
async fn test_connection_says_only_what_the_provider_reports() {
    let server = Server::start(panel_handler(Panel::default())).await;
    let (_folder, core, _) = open_core();
    let login = |password: &str| SourceSecrets {
        server_url: Some(format!("{}/get.php?username=user&password={password}", server.base)),
        username: Some("user".into()),
        password: Some(password.into()),
        ..Default::default()
    };
    let check = core.test_xtream_login(&login("pass"), Some(" TestAgent/1 ")).await.unwrap();
    assert_eq!(check.max_connections, Some(2));
    assert!(check.expires.is_some_and(|expires| expires > now()));
    assert_eq!(server.requests().last().unwrap().user_agent.as_deref(), Some("TestAgent/1"));
    let json = serde_json::to_string(&check).unwrap();
    assert!(!json.contains("user") && !json.contains("pass"), "{json}");
    assert!(core.list_sources().unwrap().is_empty(), "trying a login saves nothing");

    server.set(panel_handler(Panel { authorised: false, ..Panel::default() }));
    let error = core.test_xtream_login(&login("hunter2"), None).await.unwrap_err().to_string();
    assert_eq!(error, "The provider rejected the username or password.");
    let error = core.test_xtream_login(&SourceSecrets { username: Some("user".into()), ..Default::default() }, None).await.unwrap_err();
    assert_eq!(error.to_string(), "Enter a server, username and password first.");
}

#[tokio::test]
async fn a_dropped_playlist_is_kept_until_its_source_goes() {
    let (folder, core, _) = open_core();
    let text = playlist("http://a.example", &[("News", "Alpha One")]);
    assert!(core.keep_playlist_copy("notes.txt", b"just some notes").is_err(), "only playlists are kept");
    let kept = core.keep_playlist_copy("../My List.m3u8", text.as_bytes()).unwrap();
    assert_eq!(std::path::Path::new(&kept), folder.path().join("Playlists").join("My List.m3u8"));
    let again = core.keep_playlist_copy("My List.m3u8", text.as_bytes()).unwrap();
    assert!(again.ends_with("My List 2.m3u8"));

    let source = core
        .add_source(NewSource {
            name: String::new(),
            kind: SourceKind::M3uFile,
            secrets: SourceSecrets::default(),
            file_path: Some(kept.clone()),
            live_format: LiveStreamFormat::Hls,
            user_agent: None,
        })
        .await
        .unwrap();
    core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert_eq!(channel_names(&core, ChannelFilter::default()), ["Alpha One"]);
    core.remove_source(&source.id).unwrap();
    assert!(!std::path::Path::new(&kept).exists(), "Xorvio's own copy goes with its source");
    assert!(std::path::Path::new(&again).exists());

    // Or when another playlist replaces it.
    let source = core
        .add_source(NewSource {
            name: String::new(),
            kind: SourceKind::M3uFile,
            secrets: SourceSecrets::default(),
            file_path: Some(again.clone()),
            live_format: LiveStreamFormat::Hls,
            user_agent: None,
        })
        .await
        .unwrap();
    let newer = core.keep_playlist_copy("Newer.m3u", text.as_bytes()).unwrap();
    core.update_source(SourceConfig { file_path: Some(newer.clone()), ..source.clone() }, None).unwrap();
    assert!(!std::path::Path::new(&again).exists());
    core.update_source(SourceConfig { name: "Renamed".into(), ..core.list_sources().unwrap()[0].config.clone() }, None).unwrap();
    assert!(std::path::Path::new(&newer).exists(), "the copy in use stays");
    core.remove_source(&source.id).unwrap();

    // A playlist chosen from elsewhere is never deleted.
    let elsewhere = tempfile::tempdir().unwrap();
    let source = file_source(&core, elsewhere.path(), "Mine", &text).await;
    core.remove_source(&source.id).unwrap();
    assert!(elsewhere.path().join("Mine.m3u").exists());
}

#[tokio::test]
async fn the_pin_locks_again_when_the_time_is_up() {
    let (_folder, core, _) = open_core();
    let lists = tempfile::tempdir().unwrap();
    file_source(&core, lists.path(), "Family", &playlist("http://a.example", &[("News", "Harbor News"), ("XXX | Adults", "Late Channel")])).await;
    core.set_pin(None, Some("1234")).unwrap();
    assert_eq!(channel_names(&core, ChannelFilter::default()), ["Harbor News"]);

    // Until Xorvio quits, as before, unless a time is chosen.
    assert!(core.unlock("1234").unwrap());
    let status = core.parental_status().unwrap();
    assert_eq!((status.unlocked, status.locks_at), (true, None));

    // Choosing a time applies to the PIN already entered, counted from when it was.
    core.set_relock_after(Some(Duration::from_secs(15 * 60)));
    let status = core.parental_status().unwrap();
    assert!(status.unlocked);
    assert!(status.locks_at.is_some_and(|at| (now() + 15 * 60 - 2..=now() + 15 * 60 + 1).contains(&at)), "{status:?}");

    core.set_relock_after(Some(Duration::from_millis(150)));
    assert!(core.unlock("1234").unwrap());
    assert_eq!(channel_names(&core, ChannelFilter::default()), ["Harbor News", "Late Channel"]);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(channel_names(&core, ChannelFilter::default()), ["Harbor News"], "locked again by itself");
    let status = core.parental_status().unwrap();
    assert_eq!((status.unlocked, status.locks_at), (false, None));

    // Lock Now still locks at once.
    core.set_relock_after(None);
    assert!(core.unlock("1234").unwrap());
    core.lock();
    assert!(!core.parental_status().unwrap().unlocked);
}
