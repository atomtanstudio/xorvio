//! Library profiles applied to every list at once, the parental PIN holding
//! whatever the profile, and Home's layouts kept for each profile.

use chrono::Datelike;

use super::*;
use crate::profiles::EVERYTHING_ID;

/// Channels, films and series in English, French, Turkish and adult
/// categories, and categories naming no language. Films are from this year,
/// so they're in Top 10.
fn mixed_playlist(base: &str, year: i32) -> String {
    let film = |group: &str, title: &str, file: &str| {
        format!("#EXTINF:-1 tvg-logo=\"{base}/{file}.jpg\" group-title=\"{group}\",{title}\n{base}/movie/{file}.mp4\n")
    };
    [
        format!("#EXTM3U url-tvg=\"{base}/guide.xml.gz\"\n"),
        format!("#EXTINF:-1 tvg-id=\"news.uk\" group-title=\"UK| NEWS\",Harbor News\n{base}/live/news.m3u8\n"),
        format!("#EXTINF:-1 tvg-id=\"info.fr\" group-title=\"FR| INFO\",Canal Info\n{base}/live/info.m3u8\n"),
        format!("#EXTINF:-1 tvg-id=\"spor.tr\" group-title=\"TR | SPOR\",Spor Bir\n{base}/live/spor.m3u8\n"),
        format!("#EXTINF:-1 tvg-id=\"late\" group-title=\"XXX | Adults\",Late Channel\n{base}/live/late.m3u8\n"),
        film("EN - ACTION", &format!("Iron Verdict ({year})"), "iron"),
        film("EN - COMEDY", &format!("Paper Moons ({year})"), "paper"),
        film("QC - FILMS", &format!("Les Saisons ({year})"), "saisons"),
        film("NETFLIX MOVIES", &format!("Night Train ({year})"), "night"),
        film("NETFLIX MOVIES", "Casa de la Noche (ES)", "casa"),
        format!("#EXTINF:-1 group-title=\"TURKISH SERIES\",Yol Ayrimi S01E01\n{base}/series/yol/s1e1.mp4\n"),
        format!("#EXTINF:-1 group-title=\"EN - DRAMA SERIES\",Harbour Lights S01E01\n{base}/series/harbour/s1e1.mp4\n"),
    ]
    .concat()
}

async fn mixed_source(core: &Core) -> (Server, SourceConfig) {
    let server = Server::start(|_| (404, Vec::new())).await;
    let list = mixed_playlist(&server.base, chrono::Utc::now().year());
    let minute = this_minute();
    let listings = guide(
        minute,
        &[("news.uk", "Harbor News"), ("info.fr", "Canal Info"), ("spor.tr", "Spor Bir"), ("late", "Late Channel")],
        &[
            ("news.uk", "Harbor Report", -20, "News"),
            ("news.uk", "Match of the Night", 120, "Sports"),
            ("info.fr", "Le Journal", -20, "News"),
            ("info.fr", "Le Grand Film", 120, "Movie"),
            ("spor.tr", "Mac Gunu", -20, "Sports"),
            ("spor.tr", "Derbi Gecesi", 120, "Sports"),
            ("late", "After Dark", -20, "Movie"),
            ("late", "After Dark Again", 120, "Movie"),
        ],
    );
    server.set(move |request| match request.path.as_str() {
        "/list.m3u" => (200, list.clone().into_bytes()),
        "/guide.xml.gz" => (200, gzip(&listings)),
        _ => (404, Vec::new()),
    });
    let source = add_playlist(core, &server, "/list.m3u").await;
    (server, source)
}

fn names<T>(items: &[T], name: impl Fn(&T) -> &str) -> Vec<String> {
    let mut names: Vec<String> = items.iter().map(|item| name(item).to_string()).collect();
    names.sort();
    names
}

fn channels(core: &Core) -> Vec<String> {
    names(&core.channels(&ChannelFilter::default()).unwrap(), |channel| &channel.name)
}

fn films(core: &Core) -> Vec<String> {
    names(&core.movies(&BrowseQuery::default()).unwrap(), |movie| &movie.title)
}

fn shows(core: &Core) -> Vec<String> {
    names(&core.series_list(&BrowseQuery::default()).unwrap(), |series| &series.title)
}

fn category_id(core: &Core, kind: CategoryKind, raw_name: &str) -> String {
    core.profile_categories().unwrap().into_iter().find(|category| category.kind == kind && category.name.contains(raw_name)).unwrap().id
}

fn english() -> LibraryProfile {
    LibraryProfile { name: "English only".into(), languages: vec![ContentLanguage::English], hide_adult: true, ..Default::default() }
}

#[tokio::test]
async fn a_profile_filters_every_listing() {
    let (folder, core, secrets) = open_core();
    let (_server, source) = mixed_source(&core).await;
    let everything = core.profiles().unwrap();
    assert_eq!(everything.active_id, EVERYTHING_ID);
    assert_eq!(everything.profiles.len(), 1, "Everything until another is made");
    assert_eq!(channels(&core).len(), 4, "Everything leaves nothing out, adult channels included");
    assert_eq!(films(&core).len(), 5);
    assert_eq!(shows(&core).len(), 2);

    // Personal state from before the switch: a French film part-way and on
    // the watchlist, a French channel as a favourite and watched recently.
    let all_films = core.movies(&BrowseQuery::default()).unwrap();
    let saisons = all_films.iter().find(|movie| movie.title == "Les Saisons").unwrap().clone();
    let night = all_films.iter().find(|movie| movie.title == "Night Train").unwrap().clone();
    let canal = core.channels(&ChannelFilter::default()).unwrap().into_iter().find(|channel| channel.name == "Canal Info").unwrap();
    core.save_progress(PlaybackProgress {
        item_id: saisons.id.clone(),
        kind: MediaKind::Movie,
        source_id: source.id.clone(),
        series_id: None,
        position_seconds: 600.0,
        duration_seconds: 6000.0,
        finished: false,
        updated_at: now(),
    })
    .unwrap();
    for movie in [&saisons, &night] {
        let entry = WatchlistEntry {
            id: movie.id.clone(),
            kind: MediaKind::Movie,
            source_id: source.id.clone(),
            title: movie.title.clone(),
            subtitle: None,
            artwork_url: None,
            added_at: now(),
            channel_id: None,
            start: None,
            end: None,
            remind: false,
        };
        core.set_watchlisted(entry, true).unwrap();
    }
    core.set_favorite(&canal.id, true).unwrap();
    core.record_channel_watched(&canal.id).unwrap();
    assert_eq!(core.continue_watching(10).unwrap().len(), 1);
    assert_eq!(core.watchlist_shown().unwrap().len(), 2);
    assert!(!core.search_guide("journal", now() - 3600, 10).unwrap().is_empty());
    assert_eq!(core.search("noche", 20).unwrap().movies.len(), 1);

    // English only: French, Turkish and adult categories leave every list.
    let profile = core.save_profile(english()).unwrap();
    assert!(!profile.id.is_empty());
    assert_eq!(core.profiles().unwrap().profiles.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Everything", "English only"]);
    assert_eq!(channels(&core).len(), 4, "saving a profile doesn't switch to it");
    core.set_active_profile(&profile.id).unwrap();

    assert_eq!(channels(&core), ["Harbor News"]);
    assert_eq!(core.categories(CategoryKind::Live).unwrap().len(), 1);
    assert_eq!(core.categories(CategoryKind::Movie).unwrap().len(), 3, "EN - ACTION, EN - COMEDY and NETFLIX MOVIES");
    assert_eq!(core.categories_with_hidden(CategoryKind::Live).unwrap().len(), 1, "a profile isn't hiding: showing hidden ones doesn't bring them");
    assert_eq!(core.channels(&ChannelFilter { include_hidden: true, ..Default::default() }).unwrap().len(), 1);
    // Unlabeled categories stay, but their titles tagged with another country don't.
    assert_eq!(films(&core), ["Iron Verdict", "Night Train", "Paper Moons"]);
    assert_eq!(shows(&core), ["Harbour Lights"]);
    let french_films = category_id(&core, CategoryKind::Movie, "FILMS");
    assert!(core.movies(&BrowseQuery { category_id: Some(french_films), ..Default::default() }).unwrap().is_empty());
    let (top, _) = core.top_titles(CategoryKind::Movie, 10).unwrap();
    assert_eq!(names(&top, |movie| &movie.title), ["Iron Verdict", "Night Train", "Paper Moons"]);

    // Search, the guide, Tonight on TV and On Now.
    assert_eq!(names(&core.search("night", 20).unwrap().movies, |movie| &movie.title), ["Night Train"]);
    assert!(core.search("noche", 20).unwrap().movies.is_empty(), "the (ES) title stays out");
    assert!(core.search("canal", 20).unwrap().channels.is_empty());
    assert!(core.search("yol", 20).unwrap().series.is_empty());
    assert!(core.search("journal", 20).unwrap().programs.is_empty());
    assert!(core.search_guide("journal", now() - 3600, 10).unwrap().is_empty());
    assert_eq!(core.search_guide("harbor", now() - 3600, 10).unwrap().len(), 1);
    let tonight = core.upcoming_programs(now(), now() + 6 * 3600, 10).unwrap();
    assert_eq!(names(&tonight, |hit| &hit.program.title), ["Match of the Night"]);
    let on_now = core.on_now(10).unwrap();
    assert_eq!(names(&on_now, |hit| &hit.channel.name), ["Harbor News"]);

    // Home's personal rows.
    assert!(core.continue_watching(10).unwrap().is_empty());
    assert_eq!(names(&core.watchlist_shown().unwrap(), |entry| &entry.title), ["Night Train"]);
    assert_eq!(core.watchlist().unwrap().len(), 2, "nothing is taken off the watchlist itself");
    assert!(core.channels(&ChannelFilter { favorites_only: true, ..Default::default() }).unwrap().is_empty());
    assert!(core.recent_channels(10).unwrap().is_empty());

    // The editor still sees every category, with what it says and holds.
    let categories = core.profile_categories().unwrap();
    assert_eq!(categories.len(), 10);
    let info = categories.iter().find(|category| category.id == canal.category_id.clone().unwrap()).unwrap();
    assert_eq!((info.languages.as_slice(), info.item_count, info.adult), ([ContentLanguage::French].as_slice(), 1, false));
    assert!(categories.iter().any(|category| category.adult && category.kind == CategoryKind::Live));

    // The choice survives a relaunch.
    drop(core);
    let core = Core::open(folder.path(), secrets).unwrap();
    assert_eq!(core.profiles().unwrap().active_id, profile.id);
    assert_eq!(channels(&core), ["Harbor News"]);

    // A genre profile without live TV: categories naming other genres leave,
    // those naming none stay.
    let action = core
        .save_profile(LibraryProfile {
            name: "Action".into(),
            genres: vec![ContentGenre::Action],
            only_genres: true,
            hidden_kinds: vec![CategoryKind::Live],
            ..Default::default()
        })
        .unwrap();
    core.set_active_profile(&action.id).unwrap();
    assert!(channels(&core).is_empty());
    assert!(core.categories(CategoryKind::Live).unwrap().is_empty());
    assert_eq!(films(&core), ["Casa de la Noche", "Iron Verdict", "Les Saisons", "Night Train"]);
    assert_eq!(shows(&core), ["Yol Ayrimi"]);
    let action_rows = core.movies(&BrowseQuery { content_genre: Some(ContentGenre::Action), ..Default::default() }).unwrap();
    assert_eq!(names(&action_rows, |movie| &movie.title), ["Iron Verdict"], "Home's genre rows find a category named for the genre");

    // Categories chosen by hand win over the rules.
    let comedy = category_id(&core, CategoryKind::Movie, "COMEDY");
    let netflix = category_id(&core, CategoryKind::Movie, "NETFLIX");
    let action = core
        .save_profile(LibraryProfile { included_categories: vec![comedy], excluded_categories: vec![netflix], ..action })
        .unwrap();
    assert_eq!(films(&core), ["Iron Verdict", "Les Saisons", "Paper Moons"]);

    // Back to Everything: everything is back.
    core.set_active_profile(EVERYTHING_ID).unwrap();
    assert_eq!(channels(&core).len(), 4);
    assert_eq!(films(&core).len(), 5);
    assert_eq!(core.continue_watching(10).unwrap().len(), 1);

    // Deleting the profile in use hands over to the first one left; the last can't go.
    core.set_active_profile(&action.id).unwrap();
    core.delete_profile(&action.id).unwrap();
    assert_eq!(core.profiles().unwrap().active_id, EVERYTHING_ID);
    core.delete_profile(EVERYTHING_ID).unwrap();
    assert_eq!(core.profiles().unwrap().active_id, profile.id);
    assert!(core.delete_profile(&profile.id).is_err());
    assert!(core.set_active_profile("gone").is_err());
    assert!(core.save_profile(LibraryProfile { name: "  ".into(), ..Default::default() }).is_err());
    assert!(core.save_profile(LibraryProfile { name: "english ONLY".into(), ..Default::default() }).is_err(), "names are kept apart");
}

#[tokio::test]
async fn the_parental_pin_holds_whatever_the_profile() {
    let (_folder, core, _) = open_core();
    mixed_source(&core).await;
    let adult = category_id(&core, CategoryKind::Live, "Adults");
    core.set_pin(None, Some("1234")).unwrap();
    assert_eq!(channels(&core).len(), 3);
    assert!(core.profile_categories().unwrap().iter().all(|category| category.id != adult), "locked ones stay out of the editor");

    // A profile that asks for the category by hand still can't show it.
    let open_door = core.save_profile(LibraryProfile { name: "Late".into(), included_categories: vec![adult.clone()], ..Default::default() }).unwrap();
    core.set_active_profile(&open_door.id).unwrap();
    assert!(!channels(&core).contains(&"Late Channel".to_string()));
    assert!(core.search("late", 10).unwrap().channels.is_empty());
    assert!(core.on_now(10).unwrap().iter().all(|hit| hit.channel.name != "Late Channel"));

    // Once the PIN is entered the profile decides again.
    assert!(core.unlock("1234").unwrap());
    assert!(channels(&core).contains(&"Late Channel".to_string()));
    let strict = core.save_profile(LibraryProfile { included_categories: Vec::new(), hide_adult: true, ..open_door }).unwrap();
    assert_eq!(strict.name, "Late");
    assert!(!channels(&core).contains(&"Late Channel".to_string()), "hiding adult content works with the PIN entered too");
    core.lock();
    assert_eq!(core.parental_status().unwrap().locked_count, 1);
}

#[tokio::test]
async fn add_on_titles_follow_what_a_profile_can_judge() {
    let (_folder, core, _) = open_core();
    let title = |id: &str, title: &str, genre: Option<&str>| Movie {
        id: format!("addon|movie|{id}"),
        source_id: "addon".into(),
        title: title.into(),
        year: None,
        poster_url: None,
        backdrop_url: None,
        category_id: None,
        rating: None,
        added_at: None,
        plot: None,
        genre: genre.map(str::to_string),
        tmdb_id: None,
        imdb_id: None,
        duration_seconds: None,
        stream: StreamLocator::Addon { media_type: "movie".into(), media_id: id.into() },
    };
    let all = vec![
        title("a", "Iron Verdict", Some("Action, Thriller")),
        title("b", "Paper Moons", Some("Comedy")),
        title("c", "Unknown Quantity", None),
        title("d", "Casa de la Noche (ES)", Some("Drama")),
    ];
    let mut movies = all.clone();
    core.addon_titles_in_profile(&mut movies, &mut Vec::new()).unwrap();
    assert_eq!(movies.len(), 4, "Everything leaves add-on titles alone");

    let action = core
        .save_profile(LibraryProfile {
            name: "Action".into(),
            languages: vec![ContentLanguage::English],
            genres: vec![ContentGenre::Action],
            only_genres: true,
            ..Default::default()
        })
        .unwrap();
    core.set_active_profile(&action.id).unwrap();
    let mut movies = all.clone();
    core.addon_titles_in_profile(&mut movies, &mut Vec::new()).unwrap();
    assert_eq!(names(&movies, |movie| &movie.title), ["Iron Verdict", "Unknown Quantity"]);

    core.save_profile(LibraryProfile { hidden_kinds: vec![CategoryKind::Movie], ..action }).unwrap();
    let mut movies = all;
    core.addon_titles_in_profile(&mut movies, &mut Vec::new()).unwrap();
    assert!(movies.is_empty());
}

#[test]
fn home_layouts_are_kept_for_each_profile() {
    let (_folder, core, _) = open_core();
    assert_eq!(core.home_layout("sources").unwrap(), None, "the standard layout until it's changed");
    let layout = json!({"showHero": false, "rows": [{"id": "watchlist", "kind": "watchlist", "visible": true}]});
    core.save_home_layout("sources", Some(layout.clone())).unwrap();
    assert_eq!(core.home_layout("sources").unwrap(), Some(layout.clone()));
    assert_eq!(core.home_layout("addons").unwrap(), None, "add-ons leading have a layout of their own");
    assert!(core.home_layout("elsewhere").is_err());
    assert!(core.save_home_layout("sources", Some(json!([1, 2]))).is_err());

    let kids = core.save_profile(LibraryProfile { name: "Kids".into(), ..Default::default() }).unwrap();
    core.set_active_profile(&kids.id).unwrap();
    assert_eq!(core.home_layout("sources").unwrap(), None);
    core.save_home_layout("sources", Some(json!({"rows": []}))).unwrap();
    core.set_active_profile(EVERYTHING_ID).unwrap();
    assert_eq!(core.home_layout("sources").unwrap(), Some(layout));
    core.save_home_layout("sources", None).unwrap();
    assert_eq!(core.home_layout("sources").unwrap(), None, "reset to the standard layout");

    // A deleted profile takes its layouts with it.
    core.delete_profile(&kids.id).unwrap();
    assert_eq!(core.library.home_layout(&kids.id, "sources").unwrap(), None);
}
