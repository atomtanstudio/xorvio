//! End-to-end tests of [`Core`] against a small HTTP server on 127.0.0.1.
//! Nothing here reaches the internet, the keychain or the user's own data:
//! every test uses a temporary folder and [`MemorySecrets`].

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use xorvio_model::*;

use crate::{BrowseQuery, BrowseSort, ChannelFilter, Core, MemorySecrets, now};

mod home;
mod profiles;
mod search_terms;
mod titles;
mod source_controls;

// MARK: Test server

#[derive(Debug, Clone)]
struct Request {
    path: String,
    query: HashMap<String, String>,
    user_agent: Option<String>,
}

type Handler = Box<dyn Fn(&Request) -> (u16, Vec<u8>) + Send>;

struct Server {
    base: String,
    requests: Arc<Mutex<Vec<Request>>>,
    handler: Arc<Mutex<Handler>>,
}

impl Server {
    async fn start(handler: impl Fn(&Request) -> (u16, Vec<u8>) + Send + 'static) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<Request>>> = Arc::default();
        let handler: Arc<Mutex<Handler>> = Arc::new(Mutex::new(Box::new(handler)));
        let (log, routes) = (requests.clone(), handler.clone());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (log, routes) = (log.clone(), routes.clone());
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
                    let user_agent = text
                        .lines()
                        .find_map(|line| line.to_lowercase().strip_prefix("user-agent:").map(|_| line[11..].trim().to_string()));
                    let request = Request {
                        path: parsed.path().to_string(),
                        query: parsed.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect(),
                        user_agent,
                    };
                    log.lock().unwrap().push(request.clone());
                    let (status, body) = (routes.lock().unwrap())(&request);
                    let header =
                        format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Server { base, requests, handler }
    }

    fn set(&self, handler: impl Fn(&Request) -> (u16, Vec<u8>) + Send + 'static) {
        *self.handler.lock().unwrap() = Box::new(handler);
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    fn count(&self, path: &str, action: Option<&str>) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.path == path && request.query.get("action").map(String::as_str) == action)
            .count()
    }
}

fn ok_json(value: Value) -> (u16, Vec<u8>) {
    (200, value.to_string().into_bytes())
}

fn gzip(text: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(text.as_bytes()).unwrap();
    encoder.finish().unwrap()
}

fn xmltv_time(moment: Timestamp) -> String {
    chrono::DateTime::from_timestamp(moment, 0).unwrap().format("%Y%m%d%H%M%S +0000").to_string()
}

/// The minute a test's guide is laid out from.
fn this_minute() -> Timestamp {
    now() - now() % 60
}

/// An hour-long programme per (channel, title, start in minutes from `base`, category).
fn guide(base: Timestamp, channels: &[(&str, &str)], programmes: &[(&str, &str, i64, &str)]) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tv>\n");
    for (id, name) in channels {
        xml.push_str(&format!("<channel id=\"{id}\"><display-name>{name}</display-name></channel>\n"));
    }
    for (channel, title, offset, category) in programmes {
        let start = base + offset * 60;
        xml.push_str(&format!(
            "<programme channel=\"{channel}\" start=\"{}\" stop=\"{}\"><title>{title}</title><category>{category}</category></programme>\n",
            xmltv_time(start),
            xmltv_time(start + 3600)
        ));
    }
    xml.push_str("</tv>\n");
    xml
}

fn open_core() -> (tempfile::TempDir, Arc<Core>, Arc<MemorySecrets>) {
    let folder = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let core = Core::open(folder.path(), secrets.clone()).unwrap();
    (folder, core, secrets)
}

fn phases() -> (Arc<Mutex<Vec<String>>>, impl Fn(RefreshProgress) + Send + Sync) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = seen.clone();
    (seen, move |progress: RefreshProgress| log.lock().unwrap().push(progress.phase))
}

// MARK: Xtream

struct Panel {
    live_streams: (u16, Value),
    vod_streams: (u16, Value),
    authorised: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Panel {
            live_streams: (
                200,
                json!([
                    {"stream_id": 1, "name": "UK | Harbor News HD", "epg_channel_id": "harbor.uk", "category_id": "1",
                     "tv_archive": 1, "tv_archive_duration": "3", "num": "1", "stream_icon": "http://img.example/h.png"},
                    {"stream_id": "2", "name": "Summit Sports", "epg_channel_id": "", "category_id": 1, "tv_archive": "0"}
                ]),
            ),
            vod_streams: (
                200,
                json!([{"stream_id": "20", "name": "EN - The Glass Valley (2018)", "rating": "8", "container_extension": "mkv",
                        "stream_icon": "http://img.example/g.jpg", "category_id": "5", "added": "1700000000"}]),
            ),
            authorised: true,
        }
    }
}

fn panel_handler(panel: Panel) -> impl Fn(&Request) -> (u16, Vec<u8>) + Send + 'static {
    move |request| match request.path.as_str() {
        "/player_api.php" => match request.query.get("action").map(String::as_str) {
            None if panel.authorised => ok_json(json!({
                "user_info": {"auth": 1, "status": "Active", "exp_date": (now() + 86_400 * 365).to_string(), "max_connections": "2"},
                "server_info": {"timezone": "Europe/London"}
            })),
            None => ok_json(json!({"user_info": {"auth": 0}})),
            Some("get_live_categories") => ok_json(json!([{"category_id": "1", "category_name": "UK | News"}])),
            Some("get_vod_categories") => ok_json(json!([{"category_id": "5", "category_name": "Films"}])),
            // An empty object where a list belongs.
            Some("get_series_categories") => ok_json(json!({})),
            Some("get_live_streams") => (panel.live_streams.0, panel.live_streams.1.to_string().into_bytes()),
            Some("get_vod_streams") => (panel.vod_streams.0, panel.vod_streams.1.to_string().into_bytes()),
            Some("get_series") => ok_json(json!([{"series_id": 30, "name": "Tidewater", "cover": "http://img.example/s.jpg",
                                                   "category_id": "7", "releaseDate": "2025-01-01"}])),
            Some("get_series_info") => ok_json(json!({
                "info": {"cast": "Mara Ellison, Joon Park", "backdrop_path": ["http://img.example/b.jpg"]},
                "seasons": [{"season_number": 1, "name": "Season 1"}],
                "episodes": {"1": [{"id": "301", "episode_num": 1, "title": "Low Tide", "container_extension": "mp4",
                                    "info": {"duration_secs": "2640"}}]}
            })),
            Some("get_vod_info") => ok_json(json!({"info": {"plot": "Surveyors map a canyon.", "duration_secs": 5400,
                                                             "genre": "Science Fiction", "video": {"width": 3840, "height": 2160}}})),
            Some(_) => ok_json(json!([])),
        },
        "/xmltv.php" => (
            200,
            gzip(&guide(
                this_minute(),
                &[("harbor.uk", "Harbor News"), ("summit", "Summit Sports")],
                &[("harbor.uk", "Morning Brief", -30, "News"), ("harbor.uk", "Late Edition", 30, "News"),
                  ("summit", "Live: Matchday", -10, "Sports"), ("harbor.uk", "Old Story", -120, "News")],
            )),
        ),
        "/singlesearch/shows" => (404, b"{}".to_vec()),
        _ => (404, Vec::new()),
    }
}

async fn add_panel(core: &Core, server: &Server) -> SourceConfig {
    core.add_source(NewSource {
        name: String::new(),
        kind: SourceKind::Xtream,
        secrets: SourceSecrets {
            server_url: Some(format!("{}/", server.base)),
            username: Some("user".into()),
            password: Some("pass".into()),
            ..Default::default()
        },
        file_path: None,
        live_format: LiveStreamFormat::Hls,
        user_agent: Some("TestAgent/1".into()),
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn xtream_refresh_end_to_end() {
    let server = Server::start(panel_handler(Panel::default())).await;
    let (_folder, core, _) = open_core();
    core.set_tvmaze_base(&server.base);
    let source = add_panel(&core, &server).await;
    assert_eq!(source.display_host.as_deref(), Some(server.base.trim_start_matches("http://")));
    assert!(!source.name.contains("user"));

    let (seen, report) = phases();
    let status = core.refresh_source(&source.id, &report).await.unwrap();
    assert_eq!(status.last_error, None, "{status:?}");
    assert!(status.last_success.is_some());
    assert_eq!((status.channel_count, status.movie_count, status.series_count), (2, 1, 1));
    assert_eq!(status.program_count, 4);
    assert_eq!(status.max_connections, Some(2));
    assert_eq!(status.server_time_zone.as_deref(), Some("Europe/London"));
    assert!(status.capabilities.live && status.capabilities.guide && status.capabilities.replay);
    assert_eq!(*seen.lock().unwrap(), ["Connecting", "Reading catalog", "Organizing content", "Loading guide"]);
    // Every request to the provider names the source's player.
    assert!(server.requests().iter().all(|request| request.user_agent.as_deref() == Some("TestAgent/1")));

    let channels = core.channels(&ChannelFilter::default()).unwrap();
    assert_eq!(channels.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Harbor News", "Summit Sports"]);
    assert_eq!(channels[1].epg_id.as_deref(), Some("summit"), "matched to the guide by name");
    assert_eq!(core.categories(CategoryKind::Live).unwrap()[0].name, "News");
    let rows = core.now_next(&channels.iter().map(|c| c.id.clone()).collect::<Vec<_>>(), now()).unwrap();
    assert_eq!(rows[0].programs.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(), ["Morning Brief", "Late Edition"]);
    assert_eq!(rows[1].programs[0].title, "Matchday");
    assert!(rows[1].programs[0].flags.is_live, "“Live:” became the live flag");

    let movie = core.movies(&BrowseQuery::default()).unwrap().pop().unwrap();
    assert_eq!((movie.title.as_str(), movie.year), ("The Glass Valley", Some(2018)));

    // Playback links carry the login; nothing stored does.
    let live = core.stream_target(&source.id, &channels[0].stream, true).unwrap();
    assert_eq!(live.url, format!("{}/live/user/pass/1.m3u8", server.base));
    assert_eq!(live.user_agent.as_deref(), Some("TestAgent/1"));
    let film = core.stream_target(&source.id, &movie.stream, false).unwrap();
    assert_eq!(film.url, format!("{}/movie/user/pass/20.mkv", server.base));
    let replay = core.replay_target(&channels[0].id, now() - 3600, now() - 1800).unwrap();
    assert!(replay.url.starts_with(&format!("{}/timeshift/user/pass/30/", server.base)), "{}", replay.url);
    assert!(core.replay_target(&channels[1].id, now() - 3600, now()).is_err(), "no replay advertised");
    assert!(core.replay_target(&channels[0].id, now() - 5 * 86_400, now() - 5 * 86_400 + 60).is_err(), "outside the window");
    let stored = std::fs::read_to_string(_folder.path().join("sources.json")).unwrap();
    assert!(!stored.contains("pass") && !stored.contains("\"user\""));

    // Episodes and details are fetched once.
    let series = core.series_list(&BrowseQuery::default()).unwrap().pop().unwrap();
    let episodes = core.episodes(&series.id).await.unwrap();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].duration_seconds, Some(2640));
    core.episodes(&series.id).await.unwrap();
    let details = core.details(&series.id).await.unwrap();
    assert_eq!(server.count("/player_api.php", Some("get_series_info")), 1);
    assert_eq!(details.people.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Mara Ellison", "Joon Park"]);
    assert_eq!(details.seasons.len(), 1);
    // TVmaze was asked (people had no photos) with nothing but the title.
    let lookups: Vec<Request> = server.requests().into_iter().filter(|r| r.path == "/singlesearch/shows").collect();
    assert_eq!(lookups.len(), 1);
    assert_eq!(lookups[0].query.get("q").map(String::as_str), Some("Tidewater"));
    assert_ne!(lookups[0].user_agent.as_deref(), Some("TestAgent/1"));
    core.details(&series.id).await.unwrap();
    assert_eq!(server.requests().iter().filter(|r| r.path == "/singlesearch/shows").count(), 1);

    let film_details = core.details(&movie.id).await.unwrap();
    core.details(&movie.id).await.unwrap();
    assert_eq!(server.count("/player_api.php", Some("get_vod_info")), 1);
    assert_eq!(film_details.video.map(|v| v.width), Some(3840));
    let movie = core.movie(&movie.id).unwrap().unwrap();
    assert_eq!(movie.plot.as_deref(), Some("Surveyors map a canyon."));
    assert_eq!(movie.duration_seconds, Some(5400));

    let results = core.search("glass val", 10).unwrap();
    assert_eq!(results.movies.len(), 1);
    assert_eq!(core.search("harbor", 10).unwrap().channels.len(), 1);
    let programmes = core.search("matchday", 10).unwrap().programs;
    assert_eq!(programmes.len(), 1);
    assert_eq!(programmes[0].channel.name, "Summit Sports");
}

#[tokio::test]
async fn a_failed_refresh_keeps_the_last_good_catalog() {
    let server = Server::start(panel_handler(Panel::default())).await;
    let (_folder, core, _) = open_core();
    let source = add_panel(&core, &server).await;
    let first = core.refresh_source(&source.id, &|_| {}).await.unwrap();

    server.set(panel_handler(Panel { live_streams: (500, json!("down")), ..Panel::default() }));
    let status = core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert_eq!(status.last_error.as_deref(), Some("The server responded with HTTP 500."));
    assert_eq!(status.last_success, first.last_success);
    assert_eq!(core.channels(&ChannelFilter::default()).unwrap().len(), 2);
    assert_eq!(core.movies(&BrowseQuery::default()).unwrap().len(), 1);

    server.set(panel_handler(Panel { authorised: false, ..Panel::default() }));
    let status = core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert!(status.authentication_expired);
    assert_eq!(status.last_error.as_deref(), Some("The provider rejected the username or password."));
    assert_eq!(core.list_sources().unwrap()[0].status, status, "the status is kept");

    // A guide that comes back as an error page leaves the old guide in place.
    server.set(|request: &Request| match request.path.as_str() {
        "/xmltv.php" => (200, b"<html><body>Service unavailable</body></html>".to_vec()),
        _ => panel_handler(Panel::default())(request),
    });
    let status = core.refresh_guide(&source.id).await.unwrap();
    assert!(status.guide_last_error.as_deref().is_some_and(|error| error.starts_with("The guide could not be read")));
    assert_eq!(status.program_count, 4);
    let channels = core.channels(&ChannelFilter::default()).unwrap();
    assert_eq!(core.now_next(&[channels[0].id.clone()], now()).unwrap()[0].programs.len(), 2);
}

#[tokio::test]
async fn empty_or_missing_film_lists_keep_the_previous_ones() {
    let many: Vec<Value> = (0..60).map(|n| json!({"stream_id": n, "name": format!("Film {n}"), "category_id": "5"})).collect();
    let server = Server::start(panel_handler(Panel { vod_streams: (200, Value::Array(many.clone())), ..Panel::default() })).await;
    let (_folder, core, _) = open_core();
    let source = add_panel(&core, &server).await;
    core.refresh_source(&source.id, &|_| {}).await.unwrap();
    let all = BrowseQuery { limit: 500, ..BrowseQuery::default() };
    assert_eq!(core.movies(&all).unwrap().len(), 60);

    server.set(panel_handler(Panel { vod_streams: (200, json!([])), ..Panel::default() }));
    let status = core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert_eq!(core.movies(&all).unwrap().len(), 60);
    assert_eq!(status.movie_count, 60);
    assert!(status.catalog_warning.as_deref().is_some_and(|w| w.contains("empty film list")), "{status:?}");
    assert!(status.capabilities.movies);
    assert_eq!(core.categories(CategoryKind::Movie).unwrap().len(), 1, "their category stays too");

    server.set(panel_handler(Panel { vod_streams: (503, json!(null)), ..Panel::default() }));
    let status = core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert_eq!(status.last_error, None);
    assert_eq!(core.movies(&all).unwrap().len(), 60);
    assert!(status.catalog_warning.as_deref().is_some_and(|w| w.starts_with("Films couldn't be downloaded")));

    // A real, smaller list replaces it.
    server.set(panel_handler(Panel::default()));
    core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert_eq!(core.movies(&all).unwrap().len(), 1);
}

// MARK: M3U

fn playlist(base: &str) -> String {
    format!(
        "#EXTM3U url-tvg=\"{base}/guide.xml.gz\"\n\
         #EXTINF:-1 tvg-id=\"one.uk\" group-title=\"News\" catchup=\"default\" catchup-days=\"2\",UK | Channel One FHD\n\
         {base}/live/one.m3u8\n\
         #EXTINF:-1 group-title=\"Sports\",Sports Two\n\
         #EXTVLCOPT:http-user-agent=SpecialPlayer/2\n\
         #EXTVLCOPT:http-referrer=https://ref.example/\n\
         {base}/live/two.ts\n\
         #EXTINF:-1 group-title=\"Movies\",Paper Boats (2024)\n\
         {base}/movie/paper.mp4\n"
    )
}

#[tokio::test]
async fn playlists_with_guides_and_player_options() {
    let server = Server::start(|_| (404, Vec::new())).await;
    let base = server.base.clone();
    let minute = this_minute();
    let guide_text = Arc::new(Mutex::new(guide(
        minute,
        &[("one.uk", "Channel One"), ("two", "Sports Two")],
        &[("one.uk", "Morning", -20, "News"), ("two", "Final Whistle", 120, "Sports"), ("one.uk", "Evening Film", 180, "Movie")],
    )));
    let shared = guide_text.clone();
    let list = playlist(&base);
    server.set(move |request| match request.path.as_str() {
        "/list.m3u" => (200, list.clone().into_bytes()),
        "/guide.xml.gz" => (200, gzip(&shared.lock().unwrap())),
        _ => (404, Vec::new()),
    });
    let (_folder, core, _) = open_core();
    let source = core
        .add_source(NewSource {
            name: "My List".into(),
            kind: SourceKind::M3uUrl,
            secrets: SourceSecrets { playlist_url: Some(format!("{base}/list.m3u")), ..Default::default() },
            file_path: None,
            live_format: LiveStreamFormat::Hls,
            user_agent: Some("SourceAgent/1".into()),
        })
        .await
        .unwrap();
    let status = core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert_eq!(status.last_error, None);
    assert_eq!((status.channel_count, status.movie_count, status.program_count), (2, 1, 3));

    let channels = core.channels(&ChannelFilter::default()).unwrap();
    assert_eq!(channels[0].name, "Channel One");
    assert_eq!(channels[1].epg_id.as_deref(), Some("two"), "no tvg-id, matched by name");
    let one = core.stream_target(&source.id, &channels[0].stream, true).unwrap();
    assert_eq!(one.user_agent.as_deref(), Some("SourceAgent/1"));
    let two = core.stream_target(&source.id, &channels[1].stream, true).unwrap();
    assert_eq!(two.user_agent.as_deref(), Some("SpecialPlayer/2"));
    assert_eq!(two.headers, [("Referer".to_string(), "https://ref.example/".to_string())]);
    let start = now() - 3600;
    let replay = core.replay_target(&channels[0].id, start, start + 1800).unwrap();
    assert!(replay.url.starts_with(&format!("{base}/live/one.m3u8?utc={start}&lutc=")), "{}", replay.url);
    assert!(!replay.is_live);

    // Tonight on TV: events first, news left out, ordered by start.
    let tonight = core.upcoming_programs(now(), now() + 6 * 3600, 10).unwrap();
    assert_eq!(tonight.iter().map(|hit| hit.program.title.as_str()).collect::<Vec<_>>(), ["Final Whistle", "Evening Film"]);

    // A saved programme follows its listing when the guide moves it.
    let final_whistle = core.program(&tonight[0].program.id).unwrap().unwrap();
    let entry = crate::watchlist::entry_for_program(&final_whistle.program, &final_whistle.channel.id, true, now());
    core.set_watchlisted(entry.clone(), true).unwrap();
    *guide_text.lock().unwrap() =
        guide(minute, &[("one.uk", "Channel One"), ("two", "Sports Two")], &[("two", "Final Whistle", 150, "Sports")]);
    let status = core.refresh_guide(&source.id).await.unwrap();
    assert_eq!(status.guide_last_error, None);
    assert_eq!(status.program_count, 1);
    let watchlist = core.watchlist().unwrap();
    assert_eq!(watchlist.len(), 1);
    assert_eq!(watchlist[0].start, entry.start.map(|start| start + 30 * 60));
    assert_ne!(watchlist[0].id, entry.id, "a programme's id carries its start time");
    assert!(watchlist[0].remind);
}

// MARK: Added guides and matching by hand

#[tokio::test]
async fn added_guides_offsets_and_guide_channels_chosen_by_hand() {
    let server = Server::start(|_| (404, Vec::new())).await;
    let base = server.base.clone();
    let minute = this_minute();
    // A playlist with no guide of its own: one channel with a guide id, one
    // without, one whose id no guide has.
    let list = "#EXTM3U\n\
        #EXTINF:-1 tvg-id=\"news1.uk\",News One\n/live/news.m3u8\n\
        #EXTINF:-1,Sport Two\n/live/sport.m3u8\n\
        #EXTINF:-1 tvg-id=\"wrong.id\",Film Three\n/live/film.m3u8\n"
        .replace("/live", &format!("{base}/live"));
    let extra = Arc::new(Mutex::new(guide(
        minute,
        &[("news1.uk", "News One"), ("sport2", "Sport Two"), ("films3", "Film Channel 3")],
        &[
            ("news1.uk", "Headlines", -30, "News"),
            ("news1.uk", "Old News", -24 * 60, "News"),
            ("sport2", "Cup Final", 60, "Sports"),
            ("films3", "The Long Night", 120, "Movie"),
        ],
    )));
    let own = guide(minute, &[("news1.uk", "News One")], &[("news1.uk", "Own Bulletin", 0, "News")]);
    let shared = extra.clone();
    server.set(move |request| match request.path.as_str() {
        "/list.m3u" => (200, list.clone().into_bytes()),
        "/extra.xml" => (200, shared.lock().unwrap().clone().into_bytes()),
        "/own.xml.gz" => (200, gzip(&own)),
        _ => (404, Vec::new()),
    });
    let (_folder, core, secrets) = open_core();
    let source = core
        .add_source(NewSource {
            name: "Playlist".into(),
            kind: SourceKind::M3uUrl,
            secrets: SourceSecrets { playlist_url: Some(format!("{base}/list.m3u")), ..Default::default() },
            file_path: None,
            live_format: LiveStreamFormat::Hls,
            user_agent: None,
        })
        .await
        .unwrap();
    core.refresh_source(&source.id, &|_| {}).await.unwrap();
    let channels = core.channels(&ChannelFilter::default()).unwrap();
    let ids: Vec<String> = channels.iter().map(|c| c.id.clone()).collect();
    let (news, sport, film) = (&ids[0], &ids[1], &ids[2]);
    let titles = |core: &Core, channel_id: &str| -> Vec<(String, Timestamp)> {
        core.guide(&[channel_id.to_string()], now() - 2 * 86_400, now() + 86_400).unwrap()[0]
            .programs
            .iter()
            .map(|p| (p.title.clone(), p.start))
            .collect()
    };
    assert!(titles(&core, news).is_empty(), "the playlist has no guide");

    // An added guide, an hour late: its link goes to the keychain, not the library.
    assert!(core.add_guide_source(NewGuideSource { name: "Bad".into(), url: "not a link".into(), offset_minutes: 0 }).is_err());
    let added = core
        .add_guide_source(NewGuideSource { name: " ".into(), url: format!("{base}/extra.xml"), offset_minutes: 60 })
        .unwrap();
    assert_eq!(added.name, base.trim_start_matches("http://"), "named after its host");
    assert_eq!(core.guide_source_link(&added.id).unwrap(), Some(format!("{base}/extra.xml")));
    assert!(crate::SecretStore::get(secrets.as_ref(), &added.id).unwrap().is_some());
    let status = core.refresh_guide_source(&added.id).await.unwrap();
    assert_eq!(status.guide_last_error, None);
    assert_eq!(status.program_count, 4);
    assert_eq!(core.guide_sources().unwrap()[0].status.program_count, 4);
    assert!(core.guides_due_for_refresh(now()).unwrap().is_empty(), "just loaded");
    assert_eq!(core.guides_due_for_refresh(now() + 2 * 86_400).unwrap(), std::slice::from_ref(&added.id));

    // Channels find it by their guide id, then by name; times move by the offset.
    assert_eq!(titles(&core, news), [("Old News".to_string(), minute - 23 * 3600), ("Headlines".to_string(), minute + 30 * 60)]);
    assert_eq!(titles(&core, sport), [("Cup Final".to_string(), minute + 120 * 60)]);
    assert!(titles(&core, film).is_empty());
    let unmatched = core.guide_match(film).unwrap();
    assert_eq!((unmatched.current, unmatched.chosen), (None, false));

    // Global search, Tonight on TV and programme ids all reach the added guide.
    let hits = core.search("cup", 10).unwrap().programs;
    assert_eq!(hits.iter().map(|h| (h.program.title.as_str(), h.channel.id.as_str())).collect::<Vec<_>>(), [("Cup Final", sport.as_str())]);
    assert_eq!(core.program(&hits[0].program.id).unwrap().unwrap().channel.id, *sport);
    let tonight = core.upcoming_programs(now(), now() + 6 * 3600, 10).unwrap();
    assert!(tonight.iter().any(|hit| hit.program.title == "Cup Final"));

    // Choosing a guide channel by hand.
    let offered = core.guide_channels("film", 10).unwrap();
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].epg_id, "films3");
    assert_eq!(offered[0].name.as_deref(), Some("Film Channel 3"));
    assert_eq!(offered[0].guide_name, added.name);
    assert_eq!(core.guide_channels("news1", 10).unwrap()[0].now_title, None, "Headlines starts in half an hour");
    core.match_guide(film, &added.id, "films3").unwrap();
    assert_eq!(titles(&core, film), [("The Long Night".to_string(), minute + 180 * 60)]);
    let chosen = core.guide_match(film).unwrap();
    assert!(chosen.chosen);
    assert_eq!(chosen.current.as_ref().map(|c| c.epg_id.as_str()), Some("films3"));
    let night = core.search_guide("night", now() - 86_400, 10).unwrap();
    assert_eq!(night.len(), 1);
    assert_eq!(core.program(&night[0].program.id).unwrap().unwrap().channel.id, *film);
    // It wins over a channel's own guide id, and survives refreshes of both.
    core.match_guide(news, &added.id, "sport2").unwrap();
    assert_eq!(titles(&core, news)[0].0, "Cup Final");
    core.reset_guide_match(news).unwrap();
    core.refresh_source(&source.id, &|_| {}).await.unwrap();
    core.refresh_guide_source(&added.id).await.unwrap();
    assert_eq!(titles(&core, film)[0].0, "The Long Night");
    assert_eq!(titles(&core, news).last().unwrap().0, "Headlines");

    // Programme search for the guide: what's to come, then what aired, latest first.
    let found = core.search_guide("news", now() - 2 * 86_400, 10).unwrap();
    assert_eq!(found.iter().map(|h| h.program.title.as_str()).collect::<Vec<_>>(), ["Old News"]);
    let found = core.search_guide("e", now() - 2 * 86_400, 10).unwrap();
    assert!(found.is_empty(), "one letter finds nothing");
    let found = core.search_guide("ne", now() - 2 * 86_400, 10).unwrap();
    assert_eq!(found.iter().map(|h| h.program.title.as_str()).collect::<Vec<_>>(), ["Headlines", "Old News"]);
    assert!(core.search_guide("old", now(), 10).unwrap().is_empty(), "nothing that ended before `from`");

    // A saved programme follows its listing when the added guide moves it.
    let cup = core.search_guide("cup", now(), 1).unwrap().remove(0);
    let entry = crate::watchlist::entry_for_program(&cup.program, &cup.channel.id, true, now());
    core.set_watchlisted(entry.clone(), true).unwrap();
    *extra.lock().unwrap() = guide(minute, &[("sport2", "Sport Two"), ("films3", "Film Channel 3")], &[("sport2", "Cup Final", 90, "Sports")]);
    core.refresh_guide_source(&added.id).await.unwrap();
    assert_eq!(core.watchlist().unwrap()[0].start, entry.start.map(|start| start + 30 * 60));

    // The source's own guide comes first, with its own offset.
    let mut config = core.list_sources().unwrap()[0].config.clone();
    config.guide_offset_minutes = -60;
    let own_link = SourceSecrets { playlist_url: Some(format!("{base}/list.m3u")), epg_url: Some(format!("{base}/own.xml.gz")), ..Default::default() };
    core.update_source(config, Some(own_link)).unwrap();
    let status = core.refresh_guide(&source.id).await.unwrap();
    assert_eq!(status.guide_last_error, None);
    assert_eq!(titles(&core, news), [("Own Bulletin".to_string(), minute - 3600)]);
    assert_eq!(titles(&core, sport)[0].0, "Cup Final", "still from the added guide");

    // Removing the added guide takes its listings and the choices made from it.
    core.remove_guide_source(&added.id).unwrap();
    assert!(core.guide_sources().unwrap().is_empty());
    assert!(crate::SecretStore::get(secrets.as_ref(), &added.id).unwrap().is_none());
    assert!(titles(&core, sport).is_empty());
    assert!(titles(&core, film).is_empty());
    assert!(!core.guide_match(film).unwrap().chosen);
    assert!(core.guide_channels("film", 10).unwrap().is_empty());
    assert!(core.refresh_guide_source(&added.id).await.is_err());
}

#[tokio::test]
async fn bad_playlists_are_refused_when_added() {
    let server = Server::start(|request| match request.path.as_str() {
        "/page" => (200, b"<html>Not found</html>".to_vec()),
        _ => (401, Vec::new()),
    })
    .await;
    let (_folder, core, _) = open_core();
    let new = |link: String| NewSource {
        name: String::new(),
        kind: SourceKind::M3uUrl,
        secrets: SourceSecrets { playlist_url: Some(link), ..Default::default() },
        file_path: None,
        live_format: LiveStreamFormat::Hls,
        user_agent: None,
    };
    let error = core.add_source(new(format!("{}/page", server.base))).await.unwrap_err();
    assert_eq!(error.to_string(), "The playlist could not be read: it is not an M3U playlist");
    let error = core.add_source(new(format!("{}/get.php?username=alice&password=s3cret", server.base))).await.unwrap_err();
    assert_eq!(error.to_string(), "The provider rejected the username or password.");
    assert!(core.add_source(new("not a link".into())).await.is_err());
    assert!(core.list_sources().unwrap().is_empty());
}

// MARK: Demo and library

#[tokio::test]
async fn the_demo_library_end_to_end() {
    let (folder, core, secrets) = open_core();
    core.add_demo_source().unwrap();
    core.add_demo_source().unwrap();
    let sources = core.list_sources().unwrap();
    assert_eq!(sources.len(), 1);
    assert!(sources[0].status.last_success.is_some());
    assert!(core.sources_due_for_refresh(now()).unwrap().is_empty(), "just filled");
    assert_eq!(core.sources_due_for_refresh(now() + 2 * 86_400).unwrap(), [DEMO_SOURCE_ID]);

    let channels = core.channels(&ChannelFilter::default()).unwrap();
    assert_eq!(channels.len(), 24);
    let ids: Vec<String> = channels.iter().map(|c| c.id.clone()).collect();
    let rows = core.now_next(&ids, now()).unwrap();
    assert!(rows.iter().all(|row| row.programs.len() == 2 && row.programs[0].is_airing(now())));
    let grid = core.guide(&ids[..3], now(), now() + 3 * 3600).unwrap();
    assert!(grid.iter().all(|row| !row.programs.is_empty()));
    assert_eq!(core.channels(&ChannelFilter { limit: Some(5), ..Default::default() }).unwrap().len(), 5);
    let news = core.categories(CategoryKind::Live).unwrap().into_iter().find(|c| c.name == "News").unwrap();
    assert_eq!(core.channels(&ChannelFilter { category_id: Some(news.id), ..Default::default() }).unwrap().len(), 4);

    let hits = core.search("glass val", 20).unwrap();
    assert!(hits.movies.iter().all(|movie| movie.title == "The Glass Valley") && !hits.movies.is_empty());
    assert!(!core.search("tidewa", 20).unwrap().series.is_empty());
    assert!(core.search("\"*) OR", 20).is_ok(), "odd input never reaches FTS5 unquoted");
    let (top_movies, top_series) = core.top_titles(CategoryKind::Live, 5).unwrap();
    assert_eq!(top_movies.len(), 5);
    assert!(!top_series.is_empty());
    assert!(top_movies[0].year.unwrap() >= i64::from(chrono::Datelike::year(&chrono::Local::now())) - 1);
    assert!(core.genres(CategoryKind::Movie).unwrap().contains(&"Science Fiction".to_string()));
    let by_title = core.movies(&BrowseQuery { sort: BrowseSort::Title, limit: 3, ..Default::default() }).unwrap();
    assert_eq!(by_title[0].title, "A Quiet Signal");
    let page_two = core.movies(&BrowseQuery { sort: BrowseSort::Title, limit: 3, offset: 3, ..Default::default() }).unwrap();
    assert_ne!(page_two[0].id, by_title[0].id);
    let dramas = core.series_list(&BrowseQuery { genre: Some("Drama".into()), ..Default::default() }).unwrap();
    assert_eq!(dramas.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["Tidewater"]);
    let details = core.details(&by_title[0].id).await.unwrap();
    assert_eq!(details.backdrop_urls.len(), 3);
    assert_eq!(details.people.len(), 6);

    // Favorites, recent channels.
    core.set_favorite(&ids[3], true).unwrap();
    core.set_favorite(&ids[1], true).unwrap();
    assert_eq!(core.favorite_channel_ids().unwrap(), [ids[3].clone(), ids[1].clone()]);
    let favorites = core.channels(&ChannelFilter { favorites_only: true, ..Default::default() }).unwrap();
    assert_eq!(favorites.iter().map(|c| c.id.clone()).collect::<Vec<_>>(), [ids[3].clone(), ids[1].clone()]);
    core.set_favorite(&ids[3], false).unwrap();
    core.record_channel_watched(&ids[5]).unwrap();
    core.record_channel_watched(&ids[2]).unwrap();
    core.record_channel_watched(&ids[5]).unwrap();
    let recent: Vec<String> = core.recent_channels(10).unwrap().into_iter().map(|c| c.id).collect();
    assert_eq!(recent.len(), 2);

    // Continue Watching: a film part-way, and a series carrying on after a finished episode.
    let film = by_title[0].clone();
    let progress = |item_id: &str, kind, series_id: Option<&str>, position: f64, duration: f64, finished: bool, at: Timestamp| {
        PlaybackProgress {
            item_id: item_id.into(),
            kind,
            source_id: DEMO_SOURCE_ID.into(),
            series_id: series_id.map(str::to_string),
            position_seconds: position,
            duration_seconds: duration,
            finished,
            updated_at: at,
        }
    };
    core.save_progress(progress(&film.id, MediaKind::Movie, None, 1200.0, 5400.0, false, now() - 100)).unwrap();
    let tidewater = core.series_list(&BrowseQuery::default()).unwrap().into_iter().find(|s| s.title == "Tidewater").unwrap();
    let episodes = core.episodes(&tidewater.id).await.unwrap();
    assert_eq!(episodes.len(), 22);
    core.save_progress(progress(&episodes[0].id, MediaKind::Episode, Some(&tidewater.id), 2640.0, 2640.0, true, now() - 50))
        .unwrap();
    // An older write never replaces a newer one.
    core.save_progress(progress(&film.id, MediaKind::Movie, None, 10.0, 5400.0, false, now() - 1000)).unwrap();
    assert_eq!(core.progress(&film.id).unwrap().unwrap().position_seconds, 1200.0);

    let row = core.continue_watching(10).unwrap();
    assert_eq!(row.len(), 2, "{row:?}");
    assert!(row[0].up_next);
    assert!(matches!(row[0].reason, Some(UpNextReason::Next | UpNextReason::NewEpisode)), "{:?}", row[0].reason);
    assert_eq!(row[1].reason, None, "a film part-way says how far, not why");
    assert_eq!(row[0].item_id, episodes[1].id);
    assert_eq!(row[0].title, "Tidewater");
    assert!(row[0].subtitle.as_deref().is_some_and(|s| s.starts_with("S01 E02 · ")));
    assert_eq!(row[1].item_id, film.id);
    assert!((row[1].progress - 1200.0 / 5400.0).abs() < 1e-9);
    assert_eq!(row[1].remaining_seconds, Some(4200.0));

    // The watchlist: a programme on now leads, then films and series.
    let airing = &rows[0].programs[0];
    let entry = crate::watchlist::entry_for_program(airing, &ids[0], false, now());
    core.set_watchlisted(entry.clone(), true).unwrap();
    let saved_series = WatchlistEntry {
        id: tidewater.id.clone(),
        kind: MediaKind::Series,
        source_id: DEMO_SOURCE_ID.into(),
        title: tidewater.title.clone(),
        subtitle: None,
        artwork_url: tidewater.poster_url.clone(),
        added_at: 0,
        channel_id: None,
        start: None,
        end: None,
        remind: false,
    };
    core.set_watchlisted(saved_series.clone(), true).unwrap();
    let list = core.watchlist().unwrap();
    assert_eq!(list.iter().map(|e| e.id.clone()).collect::<Vec<_>>(), [entry.id.clone(), tidewater.id.clone()]);
    assert!(list[1].added_at > 0);
    core.set_watchlisted(saved_series, false).unwrap();
    assert_eq!(core.watchlist().unwrap().len(), 1);
    let hit = core.program(&entry.id).unwrap().unwrap();
    assert_eq!(hit.channel.id, ids[0]);
    assert!(core.program("demo|nowhere|0").unwrap().is_none());
    assert!(core.program("garbage").unwrap().is_none());

    // Demo streams are public test streams; demo replay plays a sample.
    let target = core.stream_target(DEMO_SOURCE_ID, &channels[0].stream, true).unwrap();
    assert!(target.url.starts_with("https://"));
    assert!(core.replay_target(&ids[0], now() - 3600, now() - 1800).is_ok());

    // A refresh of the demo works like any other, and everything survives a restart.
    let status = core.refresh_source(DEMO_SOURCE_ID, &|_| {}).await.unwrap();
    assert_eq!(status.channel_count, 24);
    drop(core);
    let reopened = Core::open(folder.path(), secrets).unwrap();
    assert_eq!(reopened.list_sources().unwrap().len(), 1);
    assert_eq!(reopened.channels(&ChannelFilter::default()).unwrap().len(), 24);
    assert_eq!(reopened.favorite_channel_ids().unwrap(), [ids[1].clone()]);
    assert_eq!(reopened.continue_watching(10).unwrap().len(), 2);

    reopened.remove_source(DEMO_SOURCE_ID).unwrap();
    assert!(reopened.channels(&ChannelFilter::default()).unwrap().is_empty());
    assert!(reopened.search("harbor", 10).unwrap().channels.is_empty());
    assert!(reopened.watchlist().unwrap().is_empty());
    assert!(reopened.continue_watching(10).unwrap().is_empty());
}

// MARK: Storage

/// The catalog's size on disk, with its write-ahead log.
#[tokio::test]
async fn viewing_history_names_what_was_watched() {
    let (_folder, core, _) = open_core();
    core.add_demo_source().unwrap();
    let channel = core.channels(&ChannelFilter::default()).unwrap().remove(0);
    let film = core.movies(&BrowseQuery { sort: BrowseSort::Title, limit: 1, ..Default::default() }).unwrap().remove(0);
    let tidewater = core.series_list(&BrowseQuery::default()).unwrap().into_iter().find(|s| s.title == "Tidewater").unwrap();
    let episode = core.episodes(&tidewater.id).await.unwrap().remove(0);

    core.record_channel_watched(&channel.id).unwrap();
    core.record_watched(&film.id, MediaKind::Movie, DEMO_SOURCE_ID, &film.title, None).unwrap();
    core.record_watched(&episode.id, MediaKind::Episode, DEMO_SOURCE_ID, "Tidewater", Some("S01 E01")).unwrap();
    // An add-on's film isn't in the catalog: it keeps the name it played under.
    core.record_watched("addon|movie|tt0111161", MediaKind::Movie, "addon", "The Shawshank Redemption", None).unwrap();

    let history = core.history(100).unwrap();
    let titles: Vec<&str> = history.iter().map(|entry| entry.title.as_str()).collect();
    assert_eq!(titles, ["The Shawshank Redemption", "Tidewater", film.title.as_str(), channel.name.as_str()], "newest first");
    assert!(history[1].subtitle.as_deref().is_some_and(|subtitle| subtitle.starts_with("S01 E01")));
    assert_eq!((history[0].kind, history[0].source_id.as_str()), (MediaKind::Movie, "addon"));

    // Hidden channels stay out of it, as everywhere.
    core.set_hidden(&channel.id, true).unwrap();
    assert_eq!(core.history(100).unwrap().len(), 3);

    core.clear_history().unwrap();
    assert!(core.history(100).unwrap().is_empty());
    assert!(core.recent_channels(10).unwrap().is_empty(), "Recent Channels comes from the same history");
}

fn catalog_bytes(folder: &std::path::Path) -> u64 {
    ["catalog.sqlite", "catalog.sqlite-wal"]
        .iter()
        .filter_map(|name| std::fs::metadata(folder.join(name)).ok())
        .map(|meta| meta.len())
        .sum()
}

#[tokio::test]
async fn clearing_guide_listings_gives_their_space_back() {
    let (folder, core, _) = open_core();
    core.add_demo_source().unwrap();
    let ids: Vec<String> = core.channels(&ChannelFilter::default()).unwrap().into_iter().map(|c| c.id).collect();
    let listings = core.guide_listing_count().unwrap();
    assert!(listings > 1000, "{listings}");
    let on_air = core.now_next(&ids[..1], now()).unwrap()[0].programs[0].clone();
    let entry = crate::watchlist::entry_for_program(&on_air, &ids[0], true, now());
    core.set_watchlisted(entry.clone(), true).unwrap();
    // Measured compacted too, so only the listings' own space counts.
    core.catalog.compact().unwrap();
    let before = catalog_bytes(folder.path());

    // Not while a refresh is writing listings.
    crate::store::lock(&core.refreshing).insert(DEMO_SOURCE_ID.into());
    assert!(core.clear_guide_listings().is_err());
    assert_eq!(core.guide_listing_count().unwrap(), listings);
    crate::store::lock(&core.refreshing).clear();

    core.clear_guide_listings().unwrap();
    assert_eq!(core.guide_listing_count().unwrap(), 0);
    assert!(core.now_next(&ids, now()).unwrap().iter().all(|row| row.programs.is_empty()));
    assert_eq!(core.guide_span().unwrap(), None);
    assert!(core.search_guide(&on_air.title, now() - 86_400, 10).unwrap().is_empty());
    assert_eq!(core.list_sources().unwrap()[0].status.program_count, 0, "Sources shows none");
    let after = catalog_bytes(folder.path());
    assert!(after < before, "compacted: {after} bytes, from {before}");

    // Everything else stays: channels, films, the watchlist.
    assert_eq!(core.channels(&ChannelFilter::default()).unwrap().len(), 24);
    assert!(!core.movies(&BrowseQuery::default()).unwrap().is_empty());
    assert_eq!(core.watchlist().unwrap(), [entry]);

    // The next guide refresh brings them back, on the same channels.
    let status = core.refresh_guide(DEMO_SOURCE_ID).await.unwrap();
    assert_eq!(status.program_count, listings);
    assert_eq!(core.guide_listing_count().unwrap(), listings);
    assert_eq!(core.now_next(&ids[..1], now()).unwrap()[0].programs[0].title, on_air.title);
}

#[tokio::test]
async fn paused_sources_and_edited_settings() {
    let (_folder, core, secrets) = open_core();
    core.add_demo_source().unwrap();
    let mut config = core.list_sources().unwrap()[0].config.clone();
    config.is_paused = true;
    config.name = "Paused demo".into();
    config.display_host = Some("user:pw@evil.example".into());
    core.update_source(config.clone(), None).unwrap();
    assert!(core.channels(&ChannelFilter::default()).unwrap().is_empty(), "paused sources are left out");
    assert!(core.search("harbor", 10).unwrap().channels.is_empty());
    assert_eq!(
        core.channels(&ChannelFilter { source_id: Some(DEMO_SOURCE_ID.into()), ..Default::default() }).unwrap().len(),
        24,
        "but can still be browsed by name"
    );
    assert!(core.refresh_source(DEMO_SOURCE_ID, &|_| {}).await.is_err());
    let summary = &core.list_sources().unwrap()[0];
    assert_eq!(summary.config.name, "Paused demo");
    assert_eq!(summary.config.display_host, None, "the host shown is never taken from the interface");

    let new_secrets = SourceSecrets { playlist_url: Some(" https://u:p@cdn.example/list.m3u ".into()), ..Default::default() };
    core.update_source(summary.config.clone(), Some(new_secrets)).unwrap();
    assert_eq!(core.source_secrets(DEMO_SOURCE_ID).unwrap().playlist_url.as_deref(), Some("https://u:p@cdn.example/list.m3u"));
    assert!(crate::SecretStore::get(secrets.as_ref(), DEMO_SOURCE_ID).unwrap().is_some());
    core.remove_source(DEMO_SOURCE_ID).unwrap();
    assert!(crate::SecretStore::get(secrets.as_ref(), DEMO_SOURCE_ID).unwrap().is_none());
    assert!(core.update_source(config, None).is_err(), "a removed source can't be edited");
}

/// Futures handed to Tauri run on a multi-threaded runtime, so they must be `Send`.
#[allow(dead_code)]
fn core_futures_are_send(core: &Core) {
    fn send<T: Send>(_: T) {}
    send(core.refresh_source("x", &|_| {}));
    send(core.refresh_guide("x"));
    send(core.episodes("x"));
    send(core.details("x"));
    send(core.trakt_sync());
    send(core.trakt_start_sign_in());
    send(core.trakt_check_sign_in());
    send(core.trakt_disconnect());
    send(core.trakt_set_app("", ""));
    send(core.trakt_scrobble("x", MediaKind::Movie, TraktScrobbleAction::Start, 0.0));
    send(core.add_source(NewSource {
        name: String::new(),
        kind: SourceKind::Demo,
        secrets: SourceSecrets::default(),
        file_path: None,
        live_format: LiveStreamFormat::Hls,
        user_agent: None,
    }));
}

#[test]
fn marking_watched_and_unwatched() {
    let (_dir, core, _) = open_core();
    let progress = |item_id: &str, kind, series_id: Option<&str>, position: f64, duration: f64, finished: bool, at: Timestamp| PlaybackProgress {
        item_id: item_id.into(),
        kind,
        source_id: DEMO_SOURCE_ID.into(),
        series_id: series_id.map(str::to_string),
        position_seconds: position,
        duration_seconds: duration,
        finished,
        updated_at: at,
    };
    let ids = ["s|se|1|ep|1", "s|se|1|ep|2"];
    let items: Vec<PlaybackProgress> = ids.iter().map(|id| progress(id, MediaKind::Episode, Some("s|se|1"), 0.0, 0.0, false, 0)).collect();
    // A part-watched episode is marked finished too.
    core.save_progress(progress(ids[0], MediaKind::Episode, Some("s|se|1"), 600.0, 2400.0, false, now() - 100)).unwrap();

    core.set_watched(&items, true).unwrap();
    for id in ids {
        assert!(core.progress(id).unwrap().is_some_and(|p| p.finished), "{id} is watched");
    }
    assert_eq!(core.finished_in_series("s|se|1").unwrap().len(), 2);

    core.set_watched(&items[..1], false).unwrap();
    assert!(core.progress(ids[0]).unwrap().is_none(), "unwatched forgets the progress");
    assert!(core.progress(ids[1]).unwrap().is_some());
}

#[test]
fn removing_from_continue_watching() {
    let (_dir, core, _) = open_core();
    let progress = |item_id: &str, kind, series_id: Option<&str>, finished: bool| PlaybackProgress {
        item_id: item_id.into(),
        kind,
        source_id: "addon".into(),
        series_id: series_id.map(str::to_string),
        position_seconds: 600.0,
        duration_seconds: 2400.0,
        finished,
        updated_at: now() - 100,
    };
    core.save_progress(progress("addon|movie|tt1", MediaKind::Movie, None, false)).unwrap();
    core.save_progress(progress("addon|series|tt2|ep|1:1", MediaKind::Episode, Some("addon|series|tt2"), true)).unwrap();
    core.save_progress(progress("addon|series|tt2|ep|1:2", MediaKind::Episode, Some("addon|series|tt2"), false)).unwrap();

    core.remove_from_continue_watching("addon|movie|tt1").unwrap();
    assert!(core.progress("addon|movie|tt1").unwrap().is_none(), "a film forgets where it got to");
    core.remove_from_continue_watching("addon|series|tt2|ep|1:2").unwrap();
    assert!(core.progress("addon|series|tt2|ep|1:2").unwrap().is_none());
    core.remove_from_continue_watching("addon|series|tt2|ep|1:1").unwrap();
    assert!(core.progress("addon|series|tt2|ep|1:1").unwrap().is_some_and(|p| p.finished), "watched stays watched");

    let card = |item_id: &str, updated_at: Timestamp| ContinueItem {
        item_id: item_id.into(),
        kind: MediaKind::Episode,
        source_id: "addon".into(),
        title: String::new(),
        subtitle: None,
        artwork_url: None,
        progress: 0.0,
        remaining_seconds: None,
        up_next: true,
        reason: None,
        updated_at,
    };
    let mut items = vec![card("addon|series|tt2|ep|1:3", now() - 50), card("addon|series|tt2|ep|1:3", now() + 60), card("addon|series|tt9|ep|1:1", 0)];
    core.without_dismissed(&mut items).unwrap();
    let kept: Vec<_> = items.iter().map(|item| (item.item_id.as_str(), item.updated_at > now())).collect();
    assert_eq!(kept, [("addon|series|tt2|ep|1:3", true), ("addon|series|tt9|ep|1:1", false)], "the series returns once something newer happens");
}

#[test]
fn reminders_go_out_once_shortly_before_the_start() {
    let (dir, core, secrets) = open_core();
    core.add_demo_source().unwrap();
    let channel = core.channels(&ChannelFilter::default()).unwrap().remove(0);
    let at = now();
    let programme = |title: &str, starts_in_minutes: i64, remind: bool| {
        let start = at + starts_in_minutes * 60;
        WatchlistEntry {
            id: Program::make_id(DEMO_SOURCE_ID, "test", start),
            kind: MediaKind::Program,
            source_id: DEMO_SOURCE_ID.into(),
            title: title.into(),
            subtitle: None,
            artwork_url: None,
            added_at: at - 3600,
            channel_id: Some(channel.id.clone()),
            start: Some(start),
            end: Some(start + 3600),
            remind,
        }
    };
    for entry in [
        programme("Kick-off", 4, true),
        programme("Not Reminded", 3, false),
        WatchlistEntry { channel_id: Some("demo|gone".into()), ..programme("Elsewhere", 2, true) },
        programme("Tonight", 60, true),
        programme("Under Way", -10, true),
        programme("Just Started", -1, true),
    ] {
        core.set_watchlisted(entry, true).unwrap();
    }

    let due = core.due_reminders(at, 5 * 60).unwrap();
    let said: Vec<(&str, &str)> = due.iter().map(|reminder| (reminder.title.as_str(), reminder.body.as_str())).collect();
    let on_channel = |title: &str| format!("{title} on {}", channel.name);
    assert_eq!(
        said,
        [("On now", on_channel("Just Started").as_str()), ("Starting in 2 minutes", "Elsewhere"), ("Starting in 4 minutes", on_channel("Kick-off").as_str())],
        "within the lead time or just started, soonest first; a channel that's gone leaves just the title"
    );
    assert_eq!(due[2].channel_id.as_deref(), Some(channel.id.as_str()));

    let sent: Vec<String> = due.iter().map(|reminder| reminder.item_id.clone()).collect();
    core.mark_reminders_sent(&sent, at).unwrap();
    assert!(core.due_reminders(at + 30, 5 * 60).unwrap().is_empty(), "each goes out once");
    drop(core);
    let core = Core::open(dir.path(), secrets).unwrap();
    assert!(core.due_reminders(at + 60, 5 * 60).unwrap().is_empty(), "a relaunch doesn't send them again");

    // The evening programme's turn: as far ahead as the setting says, or at the start.
    assert!(core.due_reminders(at + 54 * 60, 5 * 60).unwrap().is_empty());
    assert_eq!(core.due_reminders(at + 46 * 60, 15 * 60).unwrap()[0].title, "Starting in 14 minutes");
    assert_eq!(core.due_reminders(at + 60 * 60, 0).unwrap()[0].title, "Starting now");
    assert!(core.due_reminders(at + 63 * 60, 5 * 60).unwrap().is_empty(), "missed by more than a couple of minutes (asleep): skipped");
}

// MARK: Channel management

fn channel_names(core: &Core, filter: ChannelFilter) -> Vec<String> {
    core.channels(&filter).unwrap().into_iter().map(|channel| channel.name).collect()
}

#[tokio::test]
async fn renamed_and_hidden_channels_and_categories() {
    let (_folder, core, _) = open_core();
    core.add_demo_source().unwrap();
    let channels = core.channels(&ChannelFilter::default()).unwrap();
    let harbor = channels.iter().find(|c| c.name == "Harbor News 24").unwrap().clone();
    let news = core.categories(CategoryKind::Live).unwrap().into_iter().find(|c| c.name == "News").unwrap();

    // A name of one's own shows everywhere, survives a refresh, and is found by search.
    core.rename_item(&harbor.id, Some("  Local News  ")).unwrap();
    core.rename_item(&news.id, Some("Headlines")).unwrap();
    assert_eq!(core.channel(&harbor.id).unwrap().unwrap().name, "Local News");
    assert_eq!(core.guide(std::slice::from_ref(&harbor.id), now(), now() + 3600).unwrap()[0].channel.name, "Local News");
    assert!(core.categories(CategoryKind::Live).unwrap().iter().any(|c| c.id == news.id && c.name == "Headlines"));
    core.refresh_source(DEMO_SOURCE_ID, &|_| {}).await.unwrap();
    assert_eq!(channel_names(&core, ChannelFilter { category_id: Some(news.id.clone()), ..Default::default() })[0], "Local News");
    assert!(core.search("local ne", 20).unwrap().channels.iter().any(|c| c.id == harbor.id));
    let edits = core.item_edits().unwrap();
    let edit = edits.iter().find(|edit| edit.item_id == harbor.id).unwrap();
    assert_eq!((edit.kind, edit.original_name.as_str(), edit.display_name.as_deref()), (EditKind::Channel, "Harbor News 24", Some("Local News")));
    core.rename_item(&harbor.id, Some(" ")).unwrap();
    assert_eq!(core.channel(&harbor.id).unwrap().unwrap().name, "Harbor News 24", "a blank name goes back to the provider's");
    core.rename_item(&news.id, None).unwrap();
    assert!(core.item_edits().unwrap().is_empty(), "nothing left to undo");

    // Hidden channels leave lists, search, recents, favorites and Tonight on TV,
    // but can still be listed to show them again.
    core.set_favorite(&harbor.id, true).unwrap();
    core.record_channel_watched(&harbor.id).unwrap();
    core.set_hidden(&harbor.id, true).unwrap();
    assert_eq!(core.channels(&ChannelFilter::default()).unwrap().len(), 23);
    assert!(!channel_names(&core, ChannelFilter::default()).contains(&"Harbor News 24".to_string()));
    assert!(core.channels(&ChannelFilter { favorites_only: true, ..Default::default() }).unwrap().is_empty());
    assert!(core.recent_channels(10).unwrap().is_empty());
    assert!(core.search("harbor news", 20).unwrap().channels.is_empty());
    assert!(core.upcoming_programs(now(), now() + 12 * 3600, 100).unwrap().iter().all(|hit| hit.channel.id != harbor.id));
    assert_eq!(core.channels(&ChannelFilter { include_hidden: true, ..Default::default() }).unwrap().len(), 24);
    assert!(core.channel(&harbor.id).unwrap().is_some(), "asked for by id, it still plays");

    // A hidden category takes its channels with it.
    core.set_hidden(&news.id, true).unwrap();
    assert!(core.categories(CategoryKind::Live).unwrap().iter().all(|c| c.id != news.id));
    assert!(core.categories_with_hidden(CategoryKind::Live).unwrap().iter().any(|c| c.id == news.id));
    assert_eq!(core.channels(&ChannelFilter::default()).unwrap().len(), 20);
    assert!(core.channels(&ChannelFilter { category_id: Some(news.id.clone()), ..Default::default() }).unwrap().is_empty());
    assert!(core.search("meridian world", 20).unwrap().channels.is_empty());

    // Film categories too, from the grid and from Home's rows.
    let science_fiction = core.categories(CategoryKind::Movie).unwrap().into_iter().find(|c| c.name == "Science Fiction").unwrap();
    core.set_hidden(&science_fiction.id, true).unwrap();
    assert!(core.categories(CategoryKind::Movie).unwrap().iter().all(|c| c.id != science_fiction.id));
    let films = core.movies(&BrowseQuery::default()).unwrap();
    assert!(!films.is_empty() && films.iter().all(|m| m.category_id.as_deref() != Some(science_fiction.id.as_str())));
    assert!(core.top_titles(CategoryKind::Movie, 50).unwrap().0.iter().all(|m| m.category_id.as_deref() != Some(science_fiction.id.as_str())));
    assert_eq!(core.item_edits().unwrap().iter().filter(|edit| edit.hidden).count(), 3);

    // "Show All" brings everything back.
    core.show_all_hidden().unwrap();
    assert_eq!(core.channels(&ChannelFilter::default()).unwrap().len(), 24);
    assert!(core.item_edits().unwrap().is_empty());
    assert!(core.rename_item("demo|mv|demo-mv-1-0", Some("No")).is_err(), "films are renamed by their provider");

    // Removing the source forgets its edits.
    core.set_hidden(&harbor.id, true).unwrap();
    core.remove_source(DEMO_SOURCE_ID).unwrap();
    core.add_demo_source().unwrap();
    assert_eq!(core.channels(&ChannelFilter::default()).unwrap().len(), 24);
}

#[test]
fn channel_lists_keep_their_order() {
    let (_folder, core, _) = open_core();
    core.add_demo_source().unwrap();
    let ids: Vec<String> = core.channels(&ChannelFilter::default()).unwrap().into_iter().map(|c| c.id).collect();

    let sports = core.create_favorite_list(" Sports ", Some(&ids[4])).unwrap();
    assert_eq!(sports.name, "Sports");
    assert!(core.create_favorite_list("sports", None).is_err(), "names are unique");
    assert!(core.create_favorite_list("  ", None).is_err());
    let news = core.create_favorite_list("News", None).unwrap();
    for id in [&ids[5], &ids[6], &ids[7]] {
        core.set_in_favorite_list(&sports.id, id, true).unwrap();
    }
    core.set_in_favorite_list(&sports.id, &ids[5], true).unwrap();
    let in_list = |list: &str| -> Vec<String> {
        core.channels(&ChannelFilter { list_id: Some(list.into()), ..Default::default() }).unwrap().into_iter().map(|c| c.id).collect()
    };
    assert_eq!(in_list(&sports.id), [ids[4].clone(), ids[5].clone(), ids[6].clone(), ids[7].clone()]);
    assert!(in_list(&news.id).is_empty());
    assert!(core.favorite_channel_ids().unwrap().is_empty(), "named lists aren't Favorites");

    // Moving what's shown leaves a hidden channel where it was.
    core.set_hidden(&ids[5], true).unwrap();
    core.reorder_favorite_list(&sports.id, &[ids[7].clone(), ids[4].clone(), ids[6].clone()]).unwrap();
    assert_eq!(in_list(&sports.id), [ids[7].clone(), ids[4].clone(), ids[6].clone()]);
    let lists = core.favorite_lists().unwrap();
    assert_eq!(lists.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Sports", "News"]);
    assert_eq!(lists[0].channel_ids, [ids[7].clone(), ids[5].clone(), ids[4].clone(), ids[6].clone()]);

    // Favorites can be rearranged the same way.
    core.set_favorite(&ids[0], true).unwrap();
    core.set_favorite(&ids[1], true).unwrap();
    core.reorder_favorite_list("default", &[ids[1].clone(), ids[0].clone()]).unwrap();
    assert_eq!(core.favorite_channel_ids().unwrap(), [ids[1].clone(), ids[0].clone()]);

    core.rename_favorite_list(&news.id, "World News").unwrap();
    assert!(core.rename_favorite_list(&news.id, "SPORTS").is_err());
    core.set_in_favorite_list(&sports.id, &ids[4], false).unwrap();
    core.delete_favorite_list(&sports.id).unwrap();
    assert!(core.set_in_favorite_list(&sports.id, &ids[4], true).is_err(), "a deleted list takes nothing new");
    assert!(core.delete_favorite_list("default").is_err());
    let lists = core.favorite_lists().unwrap();
    assert_eq!((lists.len(), lists[0].name.as_str()), (1, "World News"));
}

fn adult_playlist(base: &str) -> String {
    format!(
        "#EXTM3U\n\
         #EXTINF:-1 group-title=\"News\",Harbor News\n{base}/live/news.m3u8\n\
         #EXTINF:-1 group-title=\"XXX | Adults\",Late Channel\n{base}/live/late.m3u8\n\
         #EXTINF:-1 group-title=\"FR| ADULTES 18+\",Canal Nuit\n{base}/live/nuit.m3u8\n\
         #EXTINF:-1 group-title=\"Kids\",Cartoon Bay\n{base}/live/kids.m3u8\n"
    )
}

#[tokio::test]
async fn the_parental_pin_locks_adult_categories() {
    let server = Server::start(|_| (404, Vec::new())).await;
    let list = adult_playlist(&server.base);
    server.set(move |request| match request.path.as_str() {
        "/list.m3u" => (200, list.clone().into_bytes()),
        _ => (404, Vec::new()),
    });
    let (folder, core, secrets) = open_core();
    let source = core
        .add_source(NewSource {
            name: "Family".into(),
            kind: SourceKind::M3uUrl,
            secrets: SourceSecrets { playlist_url: Some(format!("{}/list.m3u", server.base)), ..Default::default() },
            file_path: None,
            live_format: LiveStreamFormat::Hls,
            user_agent: None,
        })
        .await
        .unwrap();
    core.refresh_source(&source.id, &|_| {}).await.unwrap();
    let all = channel_names(&core, ChannelFilter::default());
    assert_eq!(all.len(), 4, "without a PIN nothing is locked");
    assert!(core.item_edits().unwrap().is_empty());
    let kids = core.channels(&ChannelFilter::default()).unwrap().into_iter().find(|c| c.name == "Cartoon Bay").unwrap();
    assert!(core.set_locked(&kids.id, true).is_err(), "locking needs a PIN");

    // A PIN locks adult-looking categories at once.
    assert!(core.set_pin(None, Some("12a4")).is_err());
    core.set_pin(None, Some("1234")).unwrap();
    assert_eq!(channel_names(&core, ChannelFilter::default()), ["Harbor News", "Cartoon Bay"]);
    assert_eq!(core.categories(CategoryKind::Live).unwrap().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["News", "Kids"]);
    assert_eq!(core.categories_with_hidden(CategoryKind::Live).unwrap().len(), 2, "showing hidden ones doesn't unlock");
    assert!(core.search("late", 10).unwrap().channels.is_empty());
    let status = core.parental_status().unwrap();
    assert_eq!((status.has_pin, status.unlocked, status.locked_count), (true, false, 2));
    assert!(core.item_edits().unwrap().is_empty(), "locked names stay out of sight too");

    // Entering it shows them until locking again.
    assert!(!core.unlock("0000").unwrap());
    assert!(core.unlock("1234").unwrap());
    assert_eq!(channel_names(&core, ChannelFilter::default()).len(), 4);
    let edits = core.item_edits().unwrap();
    assert_eq!(edits.len(), 2);
    assert!(edits.iter().all(|edit| edit.locked && edit.locked_automatically && edit.kind == EditKind::LiveCategory));
    let french = edits.iter().find(|edit| edit.original_name.contains("ADULTES")).unwrap().item_id.clone();

    // One adult category unlocked for good, and a channel locked by hand.
    core.set_locked(&french, false).unwrap();
    core.set_locked(&kids.id, true).unwrap();
    core.lock();
    assert_eq!(channel_names(&core, ChannelFilter::default()), ["Harbor News", "Canal Nuit"]);
    assert_eq!(core.parental_status().unwrap().locked_count, 2);
    assert!(core.set_locked(&french, true).is_ok(), "locking more needs no PIN");
    assert!(core.set_locked(&kids.id, false).is_err(), "unlocking does");

    // Changing or removing the PIN takes the current one, which is never stored as is.
    assert!(core.set_pin(Some("9999"), Some("4321")).is_err());
    assert!(core.set_pin(None, None).is_err());
    core.set_pin(Some("1234"), Some("4321")).unwrap();
    assert!(!core.unlock("1234").unwrap());
    let stored = core.library.preference(crate::parental::PIN_KEY).unwrap().unwrap();
    // Hashed with a random salt; the hex can hold "4321" by chance, so look at whole parts.
    assert!(stored.starts_with("v1:") && !stored.split(':').any(|part| part == "4321"), "{stored}");

    // Survives a restart, and turns tries away after five wrong ones.
    drop(core);
    let core = Core::open(folder.path(), secrets).unwrap();
    assert_eq!(channel_names(&core, ChannelFilter::default()), ["Harbor News"], "locked again on launch");
    for _ in 0..4 {
        assert!(!core.unlock("0000").unwrap());
    }
    assert!(core.unlock("1111").is_ok());
    assert!(core.unlock("4321").is_err(), "even the right PIN waits a minute after five wrong ones");

    *core.pin_failures.lock().unwrap() = (0, None);
    core.set_pin(Some("4321"), None).unwrap();
    assert_eq!(channel_names(&core, ChannelFilter::default()).len(), 4, "without a PIN locks have no effect");
    assert_eq!(core.parental_status().unwrap(), ParentalStatus::default());
    assert!(core.item_edits().unwrap().is_empty(), "a lock kept for a later PIN has nothing to show");
}

// MARK: Catch-up offsets

async fn add_playlist(core: &Core, server: &Server, path: &str) -> SourceConfig {
    let source = core
        .add_source(NewSource {
            name: "Playlist".into(),
            kind: SourceKind::M3uUrl,
            secrets: SourceSecrets { playlist_url: Some(format!("{}{path}", server.base)), ..Default::default() },
            file_path: None,
            live_format: LiveStreamFormat::Hls,
            user_agent: None,
        })
        .await
        .unwrap();
    let status = core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert_eq!(status.last_error, None);
    source
}

/// Serves `text` as /list.m3u.
async fn playlist_server(text: String) -> Server {
    let server = Server::start(|_| (404, Vec::new())).await;
    let text = text.replace("{base}", &server.base);
    server.set(move |request| match request.path.as_str() {
        "/list.m3u" => (200, text.clone().into_bytes()),
        _ => (404, Vec::new()),
    });
    server
}

#[tokio::test]
async fn catch_up_offsets_move_replays() {
    let server = playlist_server(playlist("{base}")).await;
    let (folder, core, secrets) = open_core();
    let source = add_playlist(&core, &server, "/list.m3u").await;
    let one = core.channels(&ChannelFilter::default()).unwrap().remove(0);
    assert_eq!((one.name.as_str(), one.catchup_offset_minutes), ("Channel One", 0));
    let start = now() - 3600;
    let replay_start = |core: &Core| {
        let url = core.replay_target(&one.id, start, start + 1800).unwrap().url;
        url.split("utc=").nth(1).and_then(|rest| rest.split('&').next()).unwrap().parse::<Timestamp>().unwrap()
    };
    assert_eq!(replay_start(&core), start);

    // Replays that start early are asked for later, and the other way round.
    core.set_catchup_offset(&one.id, 2).unwrap();
    assert_eq!(replay_start(&core), start + 120);
    assert_eq!(core.channel(&one.id).unwrap().unwrap().catchup_offset_minutes, 2);
    assert_eq!(core.channels(&ChannelFilter::default()).unwrap()[0].catchup_offset_minutes, 2);
    core.set_catchup_offset(&one.id, -3).unwrap();
    assert_eq!(replay_start(&core), start - 180);
    core.set_catchup_offset(&one.id, 45).unwrap();
    assert_eq!(core.channel(&one.id).unwrap().unwrap().catchup_offset_minutes, 30, "half an hour at most");
    assert!(core.set_catchup_offset(&one.category_id.clone().unwrap(), 5).is_err(), "only channels");

    // Whether it can be replayed goes by the programme, not the moved request.
    assert!(core.replay_target(&one.id, now() - 5 * 86_400, now() - 5 * 86_400 + 60).is_err());

    // Kept through a restart; 0 takes it away; gone with the source.
    drop(core);
    let core = Core::open(folder.path(), secrets).unwrap();
    assert_eq!(replay_start(&core), start + 30 * 60);
    core.set_catchup_offset(&one.id, 0).unwrap();
    assert_eq!(replay_start(&core), start);
    assert!(core.library.catchup_offsets().unwrap().is_empty());
    core.set_catchup_offset(&one.id, 4).unwrap();
    core.remove_source(&source.id).unwrap();
    assert!(core.library.catchup_offsets().unwrap().is_empty());
}

#[tokio::test]
async fn catch_up_offsets_keep_a_replay_its_length() {
    let server = Server::start(panel_handler(Panel::default())).await;
    let (_folder, core, _) = open_core();
    let source = add_panel(&core, &server).await;
    core.refresh_source(&source.id, &|_| {}).await.unwrap();
    let harbor = core.channels(&ChannelFilter::default()).unwrap().remove(0);
    let start = now() - 7200;
    // …/timeshift/user/pass/<minutes>/<start>/<stream>.m3u8
    let parts = |url: String| url.rsplit('/').skip(1).take(2).map(str::to_string).collect::<Vec<_>>();
    let plain = parts(core.replay_target(&harbor.id, start, start + 1800).unwrap().url);
    core.set_catchup_offset(&harbor.id, -5).unwrap();
    let moved = parts(core.replay_target(&harbor.id, start, start + 1800).unwrap().url);
    assert_eq!((plain[1].as_str(), moved[1].as_str()), ("30", "30"), "still 30 minutes long");
    assert_ne!(plain[0], moved[0], "but asked for earlier");
}

// MARK: Similar channels

fn versions_playlist() -> String {
    let entries = [
        ("tvg-id=\"harbor\" group-title=\"News\" catchup=\"default\" catchup-days=\"2\"", "Harbor News HD", "h-hd"),
        ("group-title=\"News\"", "Meridian 24", "m"),
        ("tvg-id=\"harbor\" group-title=\"News\"", "US: Harbor News 4K", "h-4k"),
        ("tvg-id=\"harbor\" group-title=\"News\"", "Harbor News (backup)", "h-b"),
        ("group-title=\"News\"", "Harbor News +1", "h-plus"),
        ("group-title=\"Sports\"", "Harbor News HD", "h-sport"),
        ("group-title=\"Sports\"", "Summit Sports FHD", "s-fhd"),
        ("group-title=\"Sports\"", "Summit Sports UHD (backup)", "s-uhd-b"),
        ("group-title=\"Sports\"", "Summit Sports ᵁᴴᴰ", "s-uhd"),
    ];
    let mut text = String::from("#EXTM3U\n");
    for (attributes, name, path) in entries {
        text.push_str(&format!("#EXTINF:-1 {attributes},{name}\n{{base}}/live/{path}.m3u8\n"));
    }
    text
}

#[tokio::test]
async fn similar_channels_show_as_one_row() {
    let server = playlist_server(versions_playlist()).await;
    let (_folder, core, _) = open_core();
    let source = add_playlist(&core, &server, "/list.m3u").await;
    let each = core.channels(&ChannelFilter { group_similar: Some(false), ..Default::default() }).unwrap();
    assert_eq!(each.len(), 9);
    let id = |position: usize| each[position].id.clone();
    let (hd, four_k, backup) = (id(0), id(2), id(3));

    // One row per channel within a category, where its first version is,
    // playing the best version: 4K over HD, a main feed over a backup.
    let rows = core.channels(&ChannelFilter::default()).unwrap();
    assert_eq!(
        rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        ["Harbor News", "Meridian 24", "Harbor News +1", "Harbor News", "Summit Sports"]
    );
    assert_eq!(rows[0].id, four_k);
    assert_eq!(
        rows[0].versions.iter().map(|v| (v.name.as_str(), v.quality.as_deref())).collect::<Vec<_>>(),
        [("Harbor News HD", Some("HD")), ("US: Harbor News 4K", Some("4K")), ("Harbor News (backup)", None)]
    );
    assert!(rows[1].versions.is_empty() && rows[3].versions.is_empty(), "a channel listed once is just itself");
    assert_eq!(rows[4].id, id(8), "the UHD main feed, not the UHD backup");
    assert_eq!(rows[4].versions.len(), 3);
    let news = rows[0].category_id.clone();
    assert_eq!(core.channels(&ChannelFilter { category_id: news, ..Default::default() }).unwrap().len(), 3);

    // The setting, and a list's own choice over it.
    core.set_group_similar_channels(false);
    assert_eq!(core.channels(&ChannelFilter::default()).unwrap().len(), 9);
    assert_eq!(core.channels(&ChannelFilter { group_similar: Some(true), ..Default::default() }).unwrap().len(), 5);
    core.set_group_similar_channels(true);

    // "Always play this version" is remembered, through a refresh, until undone.
    core.set_preferred_version(&hd, true).unwrap();
    let row = core.channels(&ChannelFilter::default()).unwrap().remove(0);
    assert_eq!(row.id, hd);
    assert_eq!(row.versions.iter().map(|v| v.preferred).collect::<Vec<_>>(), [true, false, false]);
    core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert_eq!(core.channels(&ChannelFilter::default()).unwrap()[0].id, hd);
    core.set_preferred_version(&backup, true).unwrap();
    assert_eq!(core.channels(&ChannelFilter::default()).unwrap()[0].id, backup, "one choice per group");
    core.set_preferred_version(&backup, false).unwrap();
    assert_eq!(core.channels(&ChannelFilter::default()).unwrap()[0].id, four_k);

    // A hidden version leaves the row; renaming the one that plays names the row.
    core.set_hidden(&four_k, true).unwrap();
    let row = core.channels(&ChannelFilter::default()).unwrap().remove(0);
    assert_eq!((row.id.as_str(), row.versions.len()), (hd.as_str(), 2));
    core.set_hidden(&four_k, false).unwrap();
    core.rename_item(&four_k, Some("My News")).unwrap();
    assert_eq!(core.channels(&ChannelFilter::default()).unwrap()[0].name, "My News");
    core.rename_item(&four_k, None).unwrap();

    // Favourites keep real channel ids, and versions in a list group there too.
    core.set_favorite(&backup, true).unwrap();
    core.set_favorite(&hd, true).unwrap();
    assert_eq!(core.favorite_channel_ids().unwrap(), [backup.clone(), hd.clone()]);
    let favorites = core.channels(&ChannelFilter { favorites_only: true, ..Default::default() }).unwrap();
    assert_eq!(favorites.len(), 1);
    assert_eq!((favorites[0].id.as_str(), favorites[0].versions.len()), (hd.as_str(), 2));

    // Choices go with their source.
    core.set_preferred_version(&hd, true).unwrap();
    core.remove_source(&source.id).unwrap();
    assert!(core.library.preferred_versions().unwrap().is_empty());
}

/// A provider the size of a large real one: 20,000 channels in blocks of a
/// hundred per category, every third an extra version of the one before.
fn large_playlist() -> String {
    let forms = ["{} HD", "US: {} FHD", "{} 4K", "{} (backup)", "UK | {} ᴴᴰ"];
    let mut text = String::from("#EXTM3U\n");
    for index in 0..20_000 {
        let name = if index % 3 == 2 {
            forms[index % forms.len()].replace("{}", &format!("Channel {}", index - 1))
        } else {
            format!("Channel {index}")
        };
        let category = index / 100;
        text.push_str(&format!("#EXTINF:-1 tvg-id=\"ch{index}\" group-title=\"Group {category}\",{name}\n{{base}}/live/{index}.ts\n"));
    }
    text
}

#[tokio::test]
async fn grouping_a_large_catalog_stays_quick() {
    let server = playlist_server(large_playlist()).await;
    let (_folder, core, _) = open_core();
    add_playlist(&core, &server, "/list.m3u").await;

    let timed = |group: bool| {
        let started = std::time::Instant::now();
        let rows = core.channels(&ChannelFilter { group_similar: Some(group), ..Default::default() }).unwrap();
        (rows.len(), started.elapsed())
    };
    timed(true); // the edits cache and SQLite's pages
    let (each, plain) = timed(false);
    let (rows, grouped) = timed(true);
    eprintln!("20,000 channels listed in {plain:?} each on its own, {grouped:?} as {rows} rows");
    assert_eq!(each, 20_000);
    // 6,666 extra versions, less the 66 that start a new category.
    assert_eq!(rows, 13_400);
    assert!(grouped < std::time::Duration::from_millis(1000), "grouping took {grouped:?}");
}

#[test]
fn what_a_media_server_remembers_merges_with_progress_here() {
    use crate::sources::server::ServerProgress;
    let folder = tempfile::tempdir().unwrap();
    let core = Core::open(folder.path(), Arc::new(MemorySecrets::default())).unwrap();
    let local = |id: &str, position: f64, finished: bool, at: Timestamp| PlaybackProgress {
        item_id: id.into(),
        kind: MediaKind::Movie,
        source_id: "srv".into(),
        series_id: None,
        position_seconds: position,
        duration_seconds: 6000.0,
        finished,
        updated_at: at,
    };
    let remote = |id: &str, position: f64, finished: bool, at: Option<Timestamp>| ServerProgress {
        item_id: id.into(),
        kind: MediaKind::Movie,
        series_id: None,
        position_seconds: position,
        duration_seconds: 6000.0,
        finished,
        updated_at: at,
    };
    // Started here an hour ago; further on there since; watched there; started here later than there.
    core.library.save_progress(&local("srv|mv|a", 100.0, false, 1_000)).unwrap();
    core.library.save_progress(&local("srv|mv|b", 100.0, false, 1_000)).unwrap();
    core.library.save_progress(&local("srv|mv|c", 900.0, false, 5_000)).unwrap();
    core.library.save_progress(&local("srv|mv|d", 0.0, true, 5_000)).unwrap();
    core.merge_server_progress(
        "srv",
        &[
            remote("srv|mv|a", 2000.0, false, Some(2_000)),
            remote("srv|mv|b", 0.0, true, Some(2_000)),
            remote("srv|mv|c", 300.0, false, Some(4_000)),
            remote("srv|mv|d", 50.0, false, Some(4_000)),
            remote("srv|mv|e", 700.0, false, None),
            remote("srv|mv|f", 0.0, false, None),
        ],
    )
    .unwrap();
    let progress = |id: &str| core.progress(id).unwrap();
    assert_eq!(progress("srv|mv|a").unwrap().position_seconds, 2000.0, "the server's is later");
    assert!(progress("srv|mv|b").unwrap().finished, "watched on the server");
    assert_eq!(progress("srv|mv|c").unwrap().position_seconds, 900.0, "Xorvio's is later");
    assert!(progress("srv|mv|d").unwrap().finished, "finished here later than the server's position");
    assert_eq!(progress("srv|mv|e").unwrap().position_seconds, 700.0, "new from the server");
    assert!(progress("srv|mv|f").is_none(), "nothing to take");
}

#[tokio::test]
async fn intro_db_is_asked_only_while_skip_times_are_on() {
    let folder = tempfile::tempdir().unwrap();
    let core = Core::open(folder.path(), Arc::new(MemorySecrets::new())).unwrap();
    let intro_db = Server::start(|_| ok_json(json!({"intro": {"start_sec": 5.0, "end_sec": 60.0}, "recap": null, "outro": null}))).await;
    core.set_intro_db_base(&intro_db.base);

    core.set_intro_db_enabled(false);
    assert_eq!(core.episode_segments("tt0903747", 1, 2).await.unwrap(), None);
    assert!(intro_db.requests().is_empty(), "nothing is sent while it's off");

    core.set_intro_db_enabled(true);
    let found = core.episode_segments("tt0903747", 1, 2).await.unwrap().unwrap();
    assert_eq!(found.intro, Some(Segment { start_seconds: 5.0, end_seconds: 60.0 }));
    let asked = intro_db.requests();
    assert_eq!(asked.len(), 1);
    assert_eq!((asked[0].path.as_str(), asked[0].query.get("imdb_id").map(String::as_str)), ("/segments", Some("tt0903747")));
}
