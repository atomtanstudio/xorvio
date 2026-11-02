//! Home's New Releases and New Episodes rows, and new episodes of followed
//! series found by the six-hourly check.

use chrono::Datelike;

use super::*;
use crate::new_episodes::CHECK_INTERVAL_SECONDS;

#[tokio::test]
async fn new_releases_are_this_year_and_last_one_card_each() {
    let year = chrono::Local::now().year();
    let film = |group: &str, title: &str, file: &str| format!("#EXTINF:-1 group-title=\"{group}\",{title}\n{{base}}/movie/{file}.mp4\n");
    let list = [
        "#EXTM3U\n".to_string(),
        film("EN - FILMS", &format!("Paper Moons ({})", year - 1), "paper"),
        film("EN - FILMS", &format!("Iron Verdict ({year})"), "iron"),
        // The same film again, in another category and quality.
        film("4K FILMS", &format!("4K-TOP - Iron Verdict ({year})"), "iron-4k"),
        film("EN - FILMS", &format!("The Long Reach ({})", year - 4), "reach"),
        film("EN - FILMS", "Undated Picture", "undated"),
        film("EN - FILMS", &format!("Night Train ({year})"), "night"),
    ]
    .concat();
    let server = playlist_server(list).await;
    let (_folder, core, _) = open_core();
    add_playlist(&core, &server, "/list.m3u").await;

    let titles: Vec<(String, Option<i64>)> = core.new_releases(10).unwrap().into_iter().map(|m| (m.title, m.year)).collect();
    let this_year = Some(i64::from(year));
    assert_eq!(titles.len(), 3, "older and undated films stay off, and a film's copies are one card: {titles:?}");
    assert!(titles[..2].iter().all(|(_, y)| *y == this_year), "this year's first: {titles:?}");
    assert!(titles.iter().any(|(title, _)| title == "Iron Verdict"));
    assert_eq!(titles[2], ("Paper Moons".into(), Some(i64::from(year - 1))));
    assert_eq!(core.new_releases(1).unwrap().len(), 1);

    // A hidden category's films leave; a copy elsewhere still shows.
    let films_in = |category: &Category| core.movies(&BrowseQuery { category_id: Some(category.id.clone()), ..Default::default() }).unwrap();
    let main = core.categories(CategoryKind::Movie).unwrap().into_iter().find(|c| films_in(c).iter().any(|m| m.title == "Paper Moons")).unwrap();
    core.set_hidden(&main.id, true).unwrap();
    let titles: Vec<String> = core.new_releases(10).unwrap().into_iter().map(|m| m.title).collect();
    assert_eq!(titles, ["Iron Verdict"]);
}

/// A panel with three series whose episodes the test changes: Tidewater
/// (saved), Harbour Lights (watched to the end) and Low Country (watched
/// part-way), episodes numbered 1…n in season 1.
/// Each series' episode count, and the day its newest one aired, by panel id.
type Episodes = HashMap<&'static str, (i64, Option<String>)>;

struct Shows(Arc<Mutex<Episodes>>);

impl Shows {
    fn set(&self, series: &'static str, count: i64, aired: Option<&str>) {
        self.0.lock().unwrap().insert(series, (count, aired.map(str::to_string)));
    }
}

async fn series_panel() -> (Server, Shows) {
    let shows = Shows(Arc::new(Mutex::new(HashMap::from([("30", (1, None)), ("31", (1, None)), ("32", (2, None))]))));
    let counts = shows.0.clone();
    let panel = panel_handler(Panel::default());
    let server = Server::start(move |request| {
        let action = request.query.get("action").map(String::as_str);
        match action {
            Some("get_series") if request.path == "/player_api.php" => ok_json(json!([
                {"series_id": 30, "name": "Tidewater", "category_id": "7"},
                {"series_id": 31, "name": "Harbour Lights", "category_id": "7"},
                {"series_id": 32, "name": "Low Country", "category_id": "7"}
            ])),
            Some("get_series_info") if request.path == "/player_api.php" => {
                let id = request.query.get("series_id").cloned().unwrap_or_default();
                let (count, aired) = counts.lock().unwrap().get(id.as_str()).cloned().unwrap_or((0, None));
                let episodes: Vec<Value> = (1..=count)
                    .map(|n| {
                        let info = match (&aired, n == count) {
                            (Some(day), true) => json!({"air_date": day}),
                            _ => json!({}),
                        };
                        json!({"id": format!("{id}{n:02}"), "episode_num": n, "title": format!("Chapter {n}"), "info": info})
                    })
                    .collect();
                ok_json(json!({"info": {}, "episodes": {"1": episodes}}))
            }
            _ => panel(request),
        }
    })
    .await;
    (server, shows)
}

#[tokio::test]
async fn new_episodes_of_followed_series() {
    let (server, shows) = series_panel().await;
    let (_folder, core, _) = open_core();
    let source = add_panel(&core, &server).await;
    core.refresh_source(&source.id, &|_| {}).await.unwrap();
    let series = core.series_list(&BrowseQuery { sort: BrowseSort::Title, ..Default::default() }).unwrap();
    let id_of = |title: &str| series.iter().find(|s| s.title == title).unwrap().id.clone();
    let (tidewater, harbour, low) = (id_of("Tidewater"), id_of("Harbour Lights"), id_of("Low Country"));

    // Tidewater is saved; Harbour Lights watched to its end; Low Country part of the way.
    let saved = WatchlistEntry {
        id: tidewater.clone(),
        kind: MediaKind::Series,
        source_id: source.id.clone(),
        title: "Tidewater".into(),
        subtitle: None,
        artwork_url: None,
        added_at: 0,
        channel_id: None,
        start: None,
        end: None,
        remind: false,
    };
    core.set_watchlisted(saved, true).unwrap();
    let watched = |series_id: &str, episode: &Episode| PlaybackProgress {
        item_id: episode.id.clone(),
        kind: MediaKind::Episode,
        source_id: source.id.clone(),
        series_id: Some(series_id.to_string()),
        position_seconds: 0.0,
        duration_seconds: 0.0,
        finished: true,
        updated_at: now(),
    };
    let harbour_episodes = core.episodes(&harbour).await.unwrap();
    core.set_watched(&[watched(&harbour, &harbour_episodes[0])], true).unwrap();
    let low_episodes = core.episodes(&low).await.unwrap();
    core.set_watched(&[watched(&low, &low_episodes[0])], true).unwrap();

    let start = now();
    assert!(core.new_episodes_due(start).unwrap(), "never looked");
    let asked = server.count("/player_api.php", Some("get_series_info"));
    assert!(core.check_new_episodes(start).await.unwrap().is_empty(), "the first look only notes where each series is");
    assert_eq!(server.count("/player_api.php", Some("get_series_info")), asked + 3, "every followed series asked again");
    assert!(!core.new_episodes_due(start + 3600).unwrap());
    assert!(core.new_episodes_due(start + CHECK_INTERVAL_SECONDS).unwrap());

    shows.set("30", 3, None);
    shows.set("31", 2, None);
    shows.set("32", 3, None);
    let later = start + CHECK_INTERVAL_SECONDS;
    let mut found = core.check_new_episodes(later).await.unwrap();
    found.sort_by(|a, b| a.series_id.cmp(&b.series_id));
    let said: Vec<(&str, &str, usize)> = found.iter().map(|f| (f.title.as_str(), f.body.as_str(), f.count)).collect();
    let mut expected = [
        (tidewater.clone(), ("2 new episodes of Tidewater", "Starting with S01 E02 · Chapter 2", 2)),
        (harbour.clone(), ("New episode of Harbour Lights", "S01 E02 · Chapter 2", 1)),
    ];
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(said, expected.iter().map(|(_, said)| *said).collect::<Vec<_>>(), "Low Country isn't caught up, so it's left alone");
    let first = found.iter().find(|f| f.series_id == tidewater).unwrap();
    assert!(first.episode_id.ends_with("x:3002"), "Play starts at the first new one: {}", first.episode_id);

    let counts = core.new_episode_counts().unwrap();
    assert_eq!((counts.get(&tidewater), counts.get(&harbour), counts.get(&low)), (Some(&2), Some(&1), None));
    assert_eq!(core.new_episode_series(10).unwrap().len(), 2, "both lead New Episodes");

    // Opening a series clears its badge.
    assert!(core.mark_episodes_seen(&tidewater).unwrap());
    assert!(!core.mark_episodes_seen(&tidewater).unwrap());
    assert_eq!(core.new_episode_counts().unwrap().keys().collect::<Vec<_>>(), [&harbour]);

    // Marking the rest of Low Country watched follows it from there.
    let low_episodes = core.episodes(&low).await.unwrap();
    let rest: Vec<PlaybackProgress> = low_episodes.iter().map(|episode| watched(&low, episode)).collect();
    core.set_watched(&rest, true).unwrap();
    shows.set("32", 4, None);
    let found = core.check_new_episodes(later + CHECK_INTERVAL_SECONDS).await.unwrap();
    assert_eq!(found.iter().map(|f| f.title.as_str()).collect::<Vec<_>>(), ["New episode of Low Country"]);
    assert!(core.check_new_episodes(later + 2 * CHECK_INTERVAL_SECONDS).await.unwrap().is_empty(), "each is announced once");
}

#[tokio::test]
async fn new_episodes_row_shows_series_with_episodes_just_aired() {
    let (server, shows) = series_panel().await;
    let (_folder, core, _) = open_core();
    let source = add_panel(&core, &server).await;
    core.refresh_source(&source.id, &|_| {}).await.unwrap();
    assert!(core.new_episode_series(10).unwrap().is_empty(), "no episodes known yet");

    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    shows.set("31", 2, Some(&today));
    shows.set("32", 2, Some("2019-05-01"));
    for show in core.series_list(&BrowseQuery::default()).unwrap() {
        core.episodes(&show.id).await.unwrap();
    }
    let row: Vec<String> = core.new_episode_series(10).unwrap().into_iter().map(|s| s.title).collect();
    assert_eq!(row, ["Harbour Lights"], "an episode aired this month; one from years ago isn't new");
}

#[test]
fn a_reminder_turns_off_and_on_with_the_programme_still_saved() {
    let (_folder, core, _) = open_core();
    core.add_demo_source().unwrap();
    let channel = core.channels(&ChannelFilter::default()).unwrap().remove(0);
    let start = now() + 4 * 60;
    let saved = WatchlistEntry {
        id: Program::make_id(DEMO_SOURCE_ID, "test", start),
        kind: MediaKind::Program,
        source_id: DEMO_SOURCE_ID.into(),
        title: "Kick-off".into(),
        subtitle: None,
        artwork_url: None,
        added_at: now() - 3600,
        channel_id: Some(channel.id.clone()),
        start: Some(start),
        end: Some(start + 3600),
        remind: true,
    };
    core.set_watchlisted(saved.clone(), true).unwrap();
    assert_eq!(core.due_reminders(now(), 5 * 60).unwrap().len(), 1);

    // Turn Off Reminder: saved again with the reminder off.
    core.set_watchlisted(WatchlistEntry { remind: false, added_at: now(), ..saved.clone() }, true).unwrap();
    let kept = core.watchlist().unwrap().into_iter().find(|entry| entry.id == saved.id).expect("still on the watchlist");
    assert!(!kept.remind);
    assert_eq!(kept.added_at, saved.added_at, "it keeps its place in the list");
    assert!(core.due_reminders(now(), 5 * 60).unwrap().is_empty(), "no reminder goes out");

    // Remind Me: back on.
    core.set_watchlisted(WatchlistEntry { remind: true, ..saved.clone() }, true).unwrap();
    assert_eq!(core.due_reminders(now(), 5 * 60).unwrap().len(), 1);
}
