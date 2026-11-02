//! Films and series as Movies and Series list them: copies of a film as one
//! poster with its versions, the Source and Watched filters, hiding titles,
//! and people's pages from Wikidata (a stand-in on 127.0.0.1, never the
//! real one).

use super::*;
use crate::WatchedFilter;

/// The same film three times (a TMDB id twice, then by title and year),
/// two films sharing a name a remake apart, and a series of two episodes.
const LIBRARY: &str = "#EXTM3U\n\
#EXTINF:-1 group-title=\"Films\" tmdb-id=\"603\",The Matrix (1999)\n{base}/movie/matrix.mp4\n\
#EXTINF:-1 group-title=\"4K Films\" tmdb-id=\"603\",The Matrix (1999)\n{base}/movie/matrix-4k.mkv\n\
#EXTINF:-1 group-title=\"Films\",Paper Boats (2024)\n{base}/movie/paper.mp4\n\
#EXTINF:-1 group-title=\"Kids\",Paper Boats (2024)\n{base}/movie/paper-kids.mp4\n\
#EXTINF:-1 group-title=\"Films\",Halloween (1978)\n{base}/movie/halloween-1978.mp4\n\
#EXTINF:-1 group-title=\"Films\",Halloween (2018)\n{base}/movie/halloween-2018.mp4\n\
#EXTINF:-1 group-title=\"Films\",John Wick (2014)\n{base}/movie/wick.mp4\n\
#EXTINF:-1 group-title=\"Series\",Harbor Lights S01E01\n{base}/series/harbor/1.mp4\n\
#EXTINF:-1 group-title=\"Series\",Harbor Lights S01E02\n{base}/series/harbor/2.mp4\n";

async fn library() -> (tempfile::TempDir, Arc<Core>, Server, SourceConfig) {
    let server = playlist_server(LIBRARY.to_string()).await;
    let (folder, core, _) = open_core();
    let source = add_playlist(&core, &server, "/list.m3u").await;
    (folder, core, server, source)
}

fn grouped() -> BrowseQuery {
    BrowseQuery { sort: BrowseSort::Title, limit: 50, group_versions: true, ..Default::default() }
}

fn titles(movies: &[Movie]) -> Vec<String> {
    movies.iter().map(|movie| format!("{} {}", movie.title, movie.year.unwrap_or_default())).collect()
}

fn find(movies: &[Movie], title: &str) -> Movie {
    movies.iter().find(|movie| movie.title == title).cloned().unwrap_or_else(|| panic!("{title} isn't listed"))
}

fn category_named(core: &Core, kind: CategoryKind, name: &str) -> String {
    core.categories(kind).unwrap().into_iter().find(|category| category.name == name).unwrap().id
}

fn watched(item: &Movie) -> PlaybackProgress {
    PlaybackProgress {
        item_id: item.id.clone(),
        kind: MediaKind::Movie,
        source_id: item.source_id.clone(),
        series_id: None,
        position_seconds: 0.0,
        duration_seconds: 0.0,
        finished: true,
        updated_at: now(),
    }
}

#[tokio::test]
async fn copies_of_a_film_are_one_poster_with_its_versions() {
    let (_folder, core, _server, source) = library().await;
    let all = core.movies(&BrowseQuery { sort: BrowseSort::Title, limit: 50, ..Default::default() }).unwrap();
    assert_eq!(all.len(), 7, "each copy, when versions aren't grouped");

    let films = core.movies(&grouped()).unwrap();
    assert_eq!(titles(&films), ["Halloween 1978", "Halloween 2018", "John Wick 2014", "Paper Boats 2024", "The Matrix 1999"]);

    // A page at a time, every film once.
    let mut paged = Vec::new();
    for offset in 0..6 {
        paged.extend(core.movies(&BrowseQuery { limit: 1, offset, ..grouped() }).unwrap());
    }
    assert_eq!(titles(&paged), titles(&films));
    for sort in [BrowseSort::RecentlyAdded, BrowseSort::Year, BrowseSort::Rating] {
        let mut ids: Vec<String> = (0..6).flat_map(|offset| core.movies(&BrowseQuery { sort, limit: 1, offset, ..grouped() }).unwrap()).map(|m| m.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 5, "{sort:?} pages neither repeat nor miss a film");
    }

    // The badge counts every copy; a film carried once has one.
    let ids: Vec<String> = films.iter().map(|film| film.id.clone()).collect();
    let marks = core.title_marks(CategoryKind::Movie, &ids).unwrap();
    let versions: Vec<i64> = marks.iter().map(|mark| mark.versions).collect();
    assert_eq!(versions, [1, 1, 1, 2, 2]);

    // In a category, its own copy stands for the film.
    let four_k = category_named(&core, CategoryKind::Movie, "4K Films");
    let in_4k = core.movies(&BrowseQuery { category_id: Some(four_k.clone()), ..grouped() }).unwrap();
    assert_eq!(in_4k.len(), 1);
    assert_eq!(in_4k[0].category_id.as_deref(), Some(four_k.as_str()));

    // The film's page lists its versions, the one opened first.
    let matrix = find(&films, "The Matrix");
    let picker = core.movie_versions(&matrix.id).unwrap();
    assert_eq!(picker.len(), 2);
    assert_eq!(picker[0].movie.id, matrix.id);
    let mut places: Vec<(String, Option<String>)> = picker.iter().map(|version| (version.source_name.clone(), version.category_name.clone())).collect();
    places.sort();
    assert_eq!(places, [("Playlist".to_string(), Some("4K Films".to_string())), ("Playlist".to_string(), Some("Films".to_string()))]);
    assert!(picker.iter().any(|version| version.extension.as_deref() == Some("mkv")));

    // Another source's copy joins the film, and the Source picker narrows to it.
    let other = playlist_server("#EXTM3U\n#EXTINF:-1 group-title=\"Movies\" tmdb-id=\"603\",The Matrix (1999)\n{base}/movie/m.mp4\n".into()).await;
    let second = add_playlist(&core, &other, "/list.m3u").await;
    let marks = core.title_marks(CategoryKind::Movie, std::slice::from_ref(&matrix.id)).unwrap();
    assert_eq!(marks[0].versions, 3);
    let only_second = core.movies(&BrowseQuery { source_id: Some(second.id.clone()), ..grouped() }).unwrap();
    assert_eq!(titles(&only_second), ["The Matrix 1999"]);
    assert_eq!(only_second[0].source_id, second.id);
    let only_first = core.movies(&BrowseQuery { source_id: Some(source.id.clone()), ..grouped() }).unwrap();
    assert_eq!(only_first.len(), 5);
    assert!(only_first.iter().all(|film| film.source_id == source.id));
}

#[tokio::test]
async fn films_and_series_by_what_has_been_watched() {
    let (_folder, core, _server, _) = library().await;
    let films = core.movies(&grouped()).unwrap();
    let all = core.movies(&BrowseQuery { sort: BrowseSort::Title, limit: 50, ..Default::default() }).unwrap();
    let old = films.iter().find(|film| film.title == "Halloween" && film.year == Some(1978)).unwrap().clone();
    core.set_watched(&[watched(&old)], true).unwrap();

    let unwatched = core.movies(&BrowseQuery { watched: WatchedFilter::Unwatched, ..grouped() }).unwrap();
    assert_eq!(titles(&unwatched), ["Halloween 2018", "John Wick 2014", "Paper Boats 2024", "The Matrix 1999"]);

    // Part-way through one copy: the film is in progress, whichever copy stands for it.
    let matrix_copies: Vec<&Movie> = all.iter().filter(|movie| movie.title == "The Matrix").collect();
    let playing = matrix_copies.iter().find(|copy| copy.id != find(&films, "The Matrix").id).unwrap();
    core.save_progress(PlaybackProgress { position_seconds: 1800.0, duration_seconds: 7200.0, finished: false, ..watched(playing) }).unwrap();
    let in_progress = core.movies(&BrowseQuery { watched: WatchedFilter::InProgress, ..grouped() }).unwrap();
    assert_eq!(titles(&in_progress), ["The Matrix 1999"]);

    let ids: Vec<String> = films.iter().map(|film| film.id.clone()).collect();
    let marks: HashMap<String, TitleMark> = core.title_marks(CategoryKind::Movie, &ids).unwrap().into_iter().map(|mark| (mark.id.clone(), mark)).collect();
    assert!(marks[&old.id].watched && marks[&old.id].progress.is_none());
    let matrix = &marks[&find(&films, "The Matrix").id];
    assert!(!matrix.watched);
    assert!((matrix.progress.unwrap() - 0.25).abs() < 0.001, "{matrix:?}");

    // A series is under way once an episode is, and done when every episode is.
    let harbor = core.series_list(&BrowseQuery::default()).unwrap().remove(0);
    let episodes = core.episodes(&harbor.id).await.unwrap();
    assert_eq!(episodes.len(), 2);
    let episode = |at: usize| PlaybackProgress {
        item_id: episodes[at].id.clone(),
        kind: MediaKind::Episode,
        series_id: Some(harbor.id.clone()),
        ..watched(&old)
    };
    let shows = |filter| core.series_list(&BrowseQuery { watched: filter, ..Default::default() }).unwrap().len();
    assert_eq!((shows(WatchedFilter::Unwatched), shows(WatchedFilter::InProgress)), (1, 0));
    core.set_watched(&[episode(0)], true).unwrap();
    assert_eq!((shows(WatchedFilter::Unwatched), shows(WatchedFilter::InProgress)), (0, 1));
    let mark = core.title_marks(CategoryKind::Series, std::slice::from_ref(&harbor.id)).unwrap().remove(0);
    assert_eq!((mark.watched, mark.progress, mark.versions), (false, Some(0.5), 1));
    core.set_watched(&[episode(1)], true).unwrap();
    assert_eq!((shows(WatchedFilter::Unwatched), shows(WatchedFilter::InProgress), shows(WatchedFilter::All)), (0, 0, 1));
    assert!(core.title_marks(CategoryKind::Series, std::slice::from_ref(&harbor.id)).unwrap()[0].watched);
}

#[tokio::test]
async fn titles_hidden_from_the_library() {
    let (_folder, core, _server, _) = library().await;
    let films = core.movies(&grouped()).unwrap();
    let matrix = find(&films, "The Matrix");
    let copies: Vec<String> = core.movie_versions(&matrix.id).unwrap().into_iter().map(|version| version.movie.id).collect();

    // One copy hidden: the film stays, with one version.
    core.set_hidden(&copies[0], true).unwrap();
    let shown = core.movies(&grouped()).unwrap();
    let left = find(&shown, "The Matrix");
    assert_eq!(left.id, copies[1]);
    assert_eq!(core.title_marks(CategoryKind::Movie, std::slice::from_ref(&left.id)).unwrap()[0].versions, 1);

    // Every copy hidden: it's gone from Movies, search and the watchlist as shown.
    core.set_hidden(&copies[1], true).unwrap();
    core.set_watchlisted(
        WatchlistEntry {
            id: copies[1].clone(),
            kind: MediaKind::Movie,
            source_id: matrix.source_id.clone(),
            title: "The Matrix".into(),
            subtitle: None,
            artwork_url: None,
            added_at: 0,
            channel_id: None,
            start: None,
            end: None,
            remind: false,
        },
        true,
    )
    .unwrap();
    assert!(!titles(&core.movies(&grouped()).unwrap()).contains(&"The Matrix 1999".to_string()));
    assert!(core.movies(&BrowseQuery::default()).unwrap().iter().all(|movie| movie.title != "The Matrix"));
    assert!(core.search("matrix", 10).unwrap().movies.is_empty());
    assert!(core.watchlist_shown().unwrap().is_empty());
    assert_eq!(core.watchlist().unwrap().len(), 1, "the watchlist itself keeps it");

    // A series too.
    let harbor = core.series_list(&BrowseQuery::default()).unwrap().remove(0);
    core.set_hidden(&harbor.id, true).unwrap();
    assert!(core.series_list(&BrowseQuery::default()).unwrap().is_empty());

    // Settings lists them to show again; they can't be renamed or locked.
    let edits = core.item_edits().unwrap();
    let hidden: Vec<(EditKind, &str)> = edits.iter().map(|edit| (edit.kind, edit.original_name.as_str())).collect();
    assert_eq!(hidden.iter().filter(|(kind, name)| *kind == EditKind::Movie && *name == "The Matrix (1999)").count(), 2);
    assert!(hidden.contains(&(EditKind::Series, "Harbor Lights")));
    assert!(core.rename_item(&harbor.id, Some("Mine")).is_err());
    assert!(core.set_hidden(&format!("{}|ep|1", harbor.id), true).is_err(), "an episode isn't a title to hide");
    core.set_hidden(&harbor.id, false).unwrap();
    core.show_all_hidden().unwrap();
    assert_eq!(core.movies(&grouped()).unwrap().len(), 5);
    assert_eq!(core.series_list(&BrowseQuery::default()).unwrap().len(), 1);
}

// MARK: Wikidata

fn cell(value: &str) -> Value {
    json!({"type": "literal", "value": value})
}

/// Two people named Keanu Reeves: the one in The Matrix and John Wick (and
/// Harbor Lights, known only by name), and a namesake with more work.
fn wikidata_reply() -> Value {
    let row = |person: &str, work: &str, label: &str, ids: Value| {
        let mut row = json!({"person": cell(&format!("http://www.wikidata.org/entity/{person}")), "work": cell(&format!("http://www.wikidata.org/entity/{work}")), "workLabel": cell(label)});
        for (key, value) in ids.as_object().unwrap() {
            row[key] = cell(value.as_str().unwrap());
        }
        row
    };
    json!({"results": {"bindings": [
        row("Q1", "Q10", "The Matrix", json!({"tmdbMovie": "603", "first": "1999-03-31T00:00:00Z", "summary": "Canadian actor", "born": "1964-09-02T00:00:00Z", "image": "http://commons.wikimedia.org/wiki/Special:FilePath/Keanu%20Reeves.jpg"})),
        row("Q1", "Q11", "John Wick", json!({"first": "2014-10-24T00:00:00Z"})),
        row("Q1", "Q12", "Harbor Lights", json!({"tmdbTV": "1399"})),
        row("Q2", "Q20", "Another Film", json!({})),
        row("Q2", "Q21", "Another Film Two", json!({})),
        row("Q2", "Q22", "Another Film Three", json!({})),
        row("Q2", "Q23", "Another Film Four", json!({}))
    ]}})
}

#[tokio::test]
async fn people_without_a_tmdb_key_come_from_wikidata() {
    let (_folder, core, _server, _) = library().await;
    let wikidata = Server::start(|request| match request.path.as_str() {
        "/sparql" if request.query.get("query").is_some_and(|query| query.contains("\"Keanu Reeves\"@en")) => ok_json(wikidata_reply()),
        "/sparql" => ok_json(json!({"results": {"bindings": []}})),
        // Commons, for the photo's credit.
        "/w/api.php" if request.query.get("titles").map(String::as_str) == Some("File:Keanu Reeves.jpg") => ok_json(json!({"query": {"pages": [{
            "title": "File:Keanu Reeves.jpg",
            "imageinfo": [{"descriptionurl": "https://commons.wikimedia.org/wiki/File:Keanu_Reeves.jpg", "extmetadata": {
                "Artist": {"value": "<a href=\"//commons.wikimedia.org/wiki/User:Gage\">Gage Skidmore</a>"},
                "LicenseShortName": {"value": "CC BY-SA 3.0"}, "AttributionRequired": {"value": "true"}
            }}]
        }]}})),
        _ => (404, Vec::new()),
    })
    .await;
    core.set_wikidata_base(&wikidata.base);
    let films = core.movies(&grouped()).unwrap();
    let matrix = find(&films, "The Matrix");

    let keanu = core.person("Keanu Reeves", None, Some(&matrix.id)).await.unwrap().unwrap();
    assert_eq!(keanu.attribution.as_deref(), Some("Wikidata"));
    assert_eq!(keanu.photo_url.as_deref(), Some("https://commons.wikimedia.org/wiki/Special:FilePath/Keanu%20Reeves.jpg?width=500"));
    assert_eq!(keanu.photo_credit.as_deref(), Some("Gage Skidmore, CC BY-SA 3.0"), "Commons' licences ask for credit");
    assert_eq!(keanu.photo_page.as_deref(), Some("https://commons.wikimedia.org/wiki/File:Keanu_Reeves.jpg"));
    assert_eq!((keanu.summary.as_deref(), keanu.birthday.as_deref()), (Some("Canadian actor"), Some("1964-09-02")));
    assert_eq!(keanu.movies.iter().map(|movie| movie.title.as_str()).collect::<Vec<_>>(), ["John Wick", "The Matrix"], "newest first, each film once");
    assert_eq!(keanu.series.iter().map(|series| series.title.as_str()).collect::<Vec<_>>(), ["Harbor Lights"]);
    let asked = wikidata.requests();
    assert_eq!(asked.len(), 2, "the filmography, then the photo's credit");
    assert!(asked.iter().all(|request| request.user_agent.as_deref().is_some_and(|agent| agent.starts_with("Xorvio/"))), "Wikimedia asks clients to say who they are");

    // Kept: asking again doesn't ask Wikidata, nor Commons.
    core.person("Keanu Reeves", None, Some(&matrix.id)).await.unwrap().unwrap();
    assert_eq!(wikidata.requests().len(), 2);

    // Without the title they're from, the namesake with more work is meant.
    let namesake = core.person("Keanu Reeves", None, None).await.unwrap().unwrap();
    assert!(namesake.movies.is_empty());

    // Someone Wikidata doesn't know: the search instead, and the miss is kept.
    assert_eq!(core.person("Nobody Known", None, None).await.unwrap(), None);
    assert_eq!(core.person("Nobody Known", None, None).await.unwrap(), None);
    assert_eq!(wikidata.requests().len(), 3);

    // The demo library's people are never sent.
    core.add_demo_source().unwrap();
    let demo = core.movies(&BrowseQuery { source_id: Some(DEMO_SOURCE_ID.into()), ..Default::default() }).unwrap().remove(0);
    assert_eq!(core.person("Someone Else", None, Some(&demo.id)).await.unwrap(), None);
    assert_eq!(wikidata.requests().len(), 3);
}

#[tokio::test]
async fn a_photo_commons_cannot_credit_is_left_out() {
    let (_folder, core, _server, _) = library().await;
    let wikidata = Server::start(|request| match request.path.as_str() {
        "/sparql" => ok_json(wikidata_reply()),
        // Commons doesn't say who took it, though its licence asks for credit.
        "/w/api.php" => ok_json(json!({"query": {"pages": [{"imageinfo": [{"descriptionurl": "https://commons.wikimedia.org/wiki/File:Keanu_Reeves.jpg",
            "extmetadata": {"LicenseShortName": {"value": "CC BY-SA 3.0"}, "AttributionRequired": {"value": "true"}}}]}]}})),
        _ => (404, Vec::new()),
    })
    .await;
    core.set_wikidata_base(&wikidata.base);
    let matrix = find(&core.movies(&grouped()).unwrap(), "The Matrix");
    let keanu = core.person("Keanu Reeves", None, Some(&matrix.id)).await.unwrap().unwrap();
    assert_eq!((keanu.photo_url, keanu.photo_credit), (None, None));
    assert_eq!(keanu.movies.len(), 2, "the page itself still shows");
}

#[tokio::test]
async fn when_wikidata_is_down_it_waits_before_asking_again() {
    let (_folder, core, _server, _) = library().await;
    let wikidata = Server::start(|_| (503, Vec::new())).await;
    core.set_wikidata_base(&wikidata.base);
    assert_eq!(core.person("Keanu Reeves", None, None).await.unwrap(), None);
    let asked = wikidata.requests().len();
    wikidata.set(|_| ok_json(wikidata_reply()));
    assert_eq!(core.person("Keanu Reeves", None, None).await.unwrap(), None);
    assert_eq!(wikidata.requests().len(), asked, "not again for a while");
}
