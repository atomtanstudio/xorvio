//! Media folders end to end, against temporary folders of empty files,
//! .nfo files and images made here. Nothing reads anyone's own folders.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use xorvio_model::{RefreshProgress, SourceKind, StreamLocator};

use super::*;
use crate::{BrowseQuery, Core, MemorySecrets};

/// The bytes that start a JPEG, enough for an image to be taken for one.
const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F'];

fn make(root: &Path, files: &[(&str, &[u8])]) {
    for (path, bytes) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

fn empty(root: &Path, paths: &[&str]) {
    make(root, &paths.iter().map(|path| (*path, &b""[..])).collect::<Vec<_>>());
}

fn root_string(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

fn scan_once(root: &Path) -> crate::snapshot::CatalogSnapshot {
    let cache = tempfile::tempdir().unwrap();
    scan("src", &[root_string(root)], &cache.path().join("scan.json")).unwrap()
}

fn file_name(url: &str) -> String {
    let parsed = url::Url::parse(url).unwrap();
    let (_, path) = parse_folder_path(parsed.path()).unwrap();
    path.file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn a_folder_becomes_a_library_with_artwork() {
    let folder = tempfile::tempdir().unwrap();
    empty(
        folder.path(),
        &[
            "Films/Heat (1995)/Heat.mkv",
            "Films/Heat (1995)/poster.jpg",
            "Films/Arrival.2016.1080p.BluRay.x264.mkv",
            "Films/Arrival.2016.1080p.BluRay.x264.jpg",
            "Films/Arrival (2016)/Extras/Interview.mkv",
            "Series/Severance/folder.jpg",
            "Series/Severance/Season 1/Severance S01E01.mkv",
            "Series/Severance/Season 1/Severance S01E02.mkv",
            "Series/Severance/Season 2/Severance S02E01.mkv",
            "Notes/readme.txt",
            ".hidden/secret.mkv",
            "Films/@eaDir/Heat.mkv",
            "Code/app.ts",
        ],
    );
    let snapshot = scan_once(folder.path());

    let mut titles: Vec<&str> = snapshot.movies.iter().map(|movie| movie.title.as_str()).collect();
    titles.sort();
    assert_eq!(titles, ["Arrival", "Heat"]);
    let heat = snapshot.movies.iter().find(|movie| movie.title == "Heat").unwrap();
    assert_eq!(heat.year, Some(1995));
    assert_eq!(heat.poster_url.as_deref().map(file_name).as_deref(), Some("poster.jpg"));
    let arrival = snapshot.movies.iter().find(|movie| movie.title == "Arrival").unwrap();
    assert_eq!(arrival.poster_url.as_deref().map(file_name).as_deref(), Some("Arrival.2016.1080p.BluRay.x264.jpg"));
    assert_eq!(snapshot.series.iter().map(|series| series.title.as_str()).collect::<Vec<_>>(), ["Severance"]);
    assert_eq!(snapshot.series[0].poster_url.as_deref().map(file_name).as_deref(), Some("folder.jpg"));
    let codes: Vec<String> = snapshot.episodes.iter().map(|episode| crate::names::episode_code(episode.season, episode.number)).collect();
    assert_eq!(codes, ["S01 E01", "S01 E02", "S02 E01"]);
    assert_eq!(snapshot.categories.iter().map(|category| category.name.as_str()).collect::<Vec<_>>(), ["Films", "Series"]);
    assert!(snapshot.capabilities.movies && snapshot.capabilities.series && !snapshot.capabilities.live);
    // Every film and episode plays straight from its file.
    let streams = snapshot.movies.iter().map(|movie| &movie.stream).chain(snapshot.episodes.iter().map(|episode| &episode.stream));
    for stream in streams {
        let StreamLocator::File { path } = stream else { panic!("not a file: {stream:?}") };
        assert!(Path::new(path).starts_with(folder.path()), "{path}");
    }
}

#[test]
fn artwork_and_nfos_beside_the_videos() {
    let folder = tempfile::tempdir().unwrap();
    make(
        folder.path(),
        &[
            ("Films/Heat (1995)/Heat.mkv", b""),
            ("Films/Heat (1995)/movie.nfo", b"<movie><title>Heat</title><plot>Cops and robbers.</plot><rating>8.3</rating><genre>Crime</genre><uniqueid type=\"tmdb\">949</uniqueid></movie>"),
            ("Films/Heat (1995)/fanart.jpg", b""),
            ("Films/Heat (1995)/clearlogo.png", b""),
            ("Films/Loose/Arrival (2016).mkv", b""),
            ("Films/Loose/Arrival (2016).nfo", b"<movie><title>Arrival</title><runtime>116</runtime></movie>"),
            ("Films/Loose/Sicario (2015).mkv", b""),
            ("Films/Loose/poster.jpg", b""),
            ("Films/Loose/movie.nfo", b"<movie><title>Wrong</title></movie>"),
            ("Series/Severance/tvshow.nfo", b"<tvshow><title>Severance</title><year>2022</year><studio>Apple TV+</studio></tvshow>"),
            ("Series/Severance/fanart.jpg", b""),
            ("Series/Severance/season01-poster.jpg", b""),
            ("Series/Severance/Season 2/folder.jpg", b""),
            ("Series/Severance/Season 1/Severance S01E02.mkv", b""),
            ("Series/Severance/Season 1/Severance S01E02.nfo", b"<episodedetails><title>Half Loop</title><aired>2022-02-18</aired></episodedetails>"),
            ("Series/Severance/Season 1/Severance S01E02-thumb.jpg", b""),
            ("Series/Severance/Season 2/Severance S02E01E02.mkv", b""),
            ("TV/poster.jpg", b""),
            ("TV/tvshow.nfo", b"<tvshow><title>Wrong</title></tvshow>"),
            ("TV/The.Bear.S01E01.mkv", b""),
            ("TV/Fargo 3x05.avi", b""),
        ],
    );
    let snapshot = scan_once(folder.path());

    let heat = snapshot.movies.iter().find(|movie| movie.title == "Heat").unwrap();
    assert_eq!(heat.plot.as_deref(), Some("Cops and robbers."));
    assert_eq!((heat.tmdb_id.as_deref(), heat.rating, heat.genre.as_deref()), (Some("949"), Some(8.3), Some("Crime")));
    let details = &snapshot.details[&heat.id];
    assert_eq!(details.backdrop_urls.iter().map(|url| file_name(url)).collect::<Vec<_>>(), ["fanart.jpg"]);
    assert_eq!(details.logo_url.as_deref().map(file_name).as_deref(), Some("clearlogo.png"));
    assert_eq!(details.overview.as_deref(), Some("Cops and robbers."));

    // A folder of several films: their own .nfo files, but not its poster.jpg or movie.nfo.
    let arrival = snapshot.movies.iter().find(|movie| movie.title == "Arrival").unwrap();
    assert_eq!(arrival.duration_seconds, Some(116 * 60));
    assert_eq!(arrival.poster_url, None);
    let sicario = snapshot.movies.iter().find(|movie| movie.title == "Sicario").unwrap();
    assert_eq!((sicario.year, sicario.poster_url.as_ref()), (Some(2015), None));

    let severance = snapshot.series.iter().find(|series| series.title == "Severance").unwrap();
    assert_eq!(severance.year, Some(2022));
    let details = &snapshot.details[&severance.id];
    assert_eq!(details.network.as_deref(), Some("Apple TV+"));
    assert_eq!(details.backdrop_urls.iter().map(|url| file_name(url)).collect::<Vec<_>>(), ["fanart.jpg"]);
    let covers: Vec<(i64, String)> = details.seasons.iter().map(|season| (season.number, file_name(season.cover_url.as_deref().unwrap()))).collect();
    assert_eq!(covers, [(1, "season01-poster.jpg".to_string()), (2, "folder.jpg".to_string())]);
    let episodes: Vec<_> = snapshot.episodes.iter().filter(|episode| episode.series_id == severance.id).collect();
    assert_eq!(episodes[0].title, "Half Loop");
    assert!(episodes[0].air_date.is_some());
    assert_eq!(episodes[0].still_url.as_deref().map(file_name).as_deref(), Some("Severance S01E02-thumb.jpg"));
    assert_eq!((episodes[1].season, episodes[1].number, episodes[1].title.as_str()), (2, 1, "Episodes 1–2"));

    // TV holds two shows, so its poster and tvshow.nfo belong to neither.
    let mut titles: Vec<&str> = snapshot.series.iter().map(|series| series.title.as_str()).collect();
    titles.sort();
    assert_eq!(titles, ["Fargo", "Severance", "The Bear"]);
    assert!(snapshot.series.iter().filter(|series| series.title != "Severance").all(|series| series.poster_url.is_none()));
}

#[test]
fn a_folder_chosen_for_one_series_or_film() {
    let show = tempfile::tempdir().unwrap();
    let show_root = show.path().join("Shogun (2024)");
    empty(&show_root, &["poster.jpg", "Season 1/S01E01.mkv", "Season 1/S01E02.mkv"]);
    let snapshot = scan_once(&show_root);
    assert_eq!((snapshot.series[0].title.as_str(), snapshot.series[0].year), ("Shogun", Some(2024)));
    assert_eq!(snapshot.series[0].poster_url.as_deref().map(file_name).as_deref(), Some("poster.jpg"));
    assert_eq!(snapshot.episodes.len(), 2);

    let film = tempfile::tempdir().unwrap();
    let film_root = film.path().join("Heat (1995)");
    empty(&film_root, &["title_t00.mkv", "poster.jpg"]);
    let snapshot = scan_once(&film_root);
    assert_eq!((snapshot.movies[0].title.as_str(), snapshot.movies[0].year), ("Heat", Some(1995)));
    assert_eq!(snapshot.movies[0].poster_url.as_deref().map(file_name).as_deref(), Some("poster.jpg"));
}

#[test]
fn ids_stay_the_same_from_scan_to_scan() {
    let folder = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let cache = cache.path().join("scan.json");
    empty(folder.path(), &["Heat (1995)/Heat.mkv", "Show/Season 1/Show S01E01.mkv"]);
    let roots = [root_string(folder.path())];
    let first = scan("src", &roots, &cache).unwrap();
    // New files, and an episode replaced by a better copy with another name.
    empty(folder.path(), &["Arrival (2016).mkv", "Show/Season 1/Show S01E02.mkv"]);
    std::fs::remove_file(folder.path().join("Show/Season 1/Show S01E01.mkv")).unwrap();
    make(folder.path(), &[("Show/Season 1/Show.S01E01.1080p.mkv", b"bigger")]);
    let second = scan("src", &roots, &cache).unwrap();

    let ids = |snapshot: &crate::snapshot::CatalogSnapshot| {
        let mut ids: Vec<String> = snapshot.movies.iter().map(|movie| movie.id.clone()).chain(snapshot.series.iter().map(|series| series.id.clone())).chain(snapshot.episodes.iter().map(|episode| episode.id.clone())).collect();
        ids.sort();
        ids
    };
    let before = ids(&first);
    let after = ids(&second);
    assert!(before.iter().all(|id| after.contains(id)), "{before:?} → {after:?}");
    assert_eq!(after.len(), before.len() + 2);
    // Two folders with the same files keep apart.
    let twin = tempfile::tempdir().unwrap();
    empty(twin.path(), &["Heat (1995)/Heat.mkv"]);
    let both = scan("src", &[roots[0].clone(), root_string(twin.path())], &cache).unwrap();
    let heats: Vec<_> = both.movies.iter().filter(|movie| movie.title == "Heat").map(|movie| movie.id.as_str()).collect();
    assert_eq!(heats.len(), 2);
    assert_ne!(heats[0], heats[1]);
}

/// A "|" in a name never reads as the part of an id it separates. (Windows
/// allows none in a file's name.)
#[cfg(unix)]
#[test]
fn a_bar_in_a_name_stays_in_the_name() {
    let folder = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    empty(folder.path(), &["Odd | Name.mkv"]);
    let snapshot = scan("src", &[root_string(folder.path())], &cache.path().join("scan.json")).unwrap();
    let odd = snapshot.movies.iter().find(|movie| movie.title == "Odd | Name").unwrap();
    assert_eq!(odd.id.matches('|').count(), 2, "{}", odd.id);
    assert!(odd.id.starts_with("src|mv|"));
}

#[test]
fn only_changed_nfo_files_are_read_again() {
    let folder = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let cache = cache.path().join("scan.json");
    make(folder.path(), &[("Heat (1995)/Heat.mkv", b""), ("Heat (1995)/Heat.nfo", b"<movie><title>Heat One</title></movie>")]);
    let roots = [root_string(folder.path())];
    assert_eq!(scan("src", &roots, &cache).unwrap().movies[0].title, "Heat One");

    // Same size, same modification time: taken as unchanged, so not read.
    let nfo = folder.path().join("Heat (1995)/Heat.nfo");
    let modified = std::fs::metadata(&nfo).unwrap().modified().unwrap();
    std::fs::write(&nfo, b"<movie><title>Heat Two</title></movie>").unwrap();
    std::fs::File::options().write(true).open(&nfo).unwrap().set_modified(modified).unwrap();
    assert_eq!(scan("src", &roots, &cache).unwrap().movies[0].title, "Heat One");

    // A new modification time is a change.
    std::fs::File::options().write(true).open(&nfo).unwrap().set_modified(modified + Duration::from_secs(5)).unwrap();
    assert_eq!(scan("src", &roots, &cache).unwrap().movies[0].title, "Heat Two");
}

#[test]
fn a_folder_that_isnt_there_keeps_what_it_had() {
    let missing = format!("/nonexistent-xorvio-{}", uuid::Uuid::new_v4());
    let cache = tempfile::tempdir().unwrap();
    let cache = cache.path().join("scan.json");
    let error = scan("src", std::slice::from_ref(&missing), &cache).unwrap_err();
    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert!(error.to_string().contains("isn't available right now"), "{error}");

    // Two folders, one of which goes away: it's listed as it was, with a warning.
    let here = tempfile::tempdir().unwrap();
    let away = tempfile::tempdir().unwrap();
    empty(here.path(), &["Heat (1995).mkv"]);
    empty(away.path(), &["Arrival (2016).mkv"]);
    let roots = [root_string(here.path()), root_string(away.path())];
    assert_eq!(scan("src", &roots, &cache).unwrap().movies.len(), 2);
    let away_path = away.path().to_path_buf();
    drop(away);
    let snapshot = scan("src", &roots, &cache).unwrap();
    assert_eq!(snapshot.movies.len(), 2, "the folder that's away is still listed");
    assert_eq!(snapshot.warnings.len(), 1);
    assert!(snapshot.warnings[0].contains("isn't available right now") && snapshot.warnings[0].contains("still listed"), "{:?}", snapshot.warnings);
    assert!(!away_path.exists());
}

#[test]
fn a_large_folder_is_quick() {
    let folder = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for show in 0..50 {
        for episode in 1..=60 {
            paths.push(format!("TV/Show {show}/Season 1/Show {show} S01E{episode:02}.mkv"));
        }
    }
    for film in 0..2000 {
        paths.push(format!("Films/Film Number {film} (2001)/Film Number {film}.mkv"));
    }
    empty(folder.path(), &paths.iter().map(String::as_str).collect::<Vec<_>>());
    let started = Instant::now();
    let snapshot = scan_once(folder.path());
    assert_eq!((snapshot.movies.len(), snapshot.series.len(), snapshot.episodes.len()), (2000, 50, 3000));
    assert!(started.elapsed() < Duration::from_secs(10), "5,000 files took {:?}", started.elapsed());
}

#[test]
fn folder_addresses_round_trip() {
    let path = "/Volumes/NAS/Films/Heat (1995)/poster 1+2 & ü.jpg";
    let address = folder_file_url("src-1", path, Some(1234));
    let parsed = url::Url::parse(&address).unwrap();
    assert_eq!(parsed.query(), Some("v=1234"));
    assert_eq!(parse_folder_path(parsed.path()), Some(("src-1".to_string(), PathBuf::from(path))));
    assert_eq!(parse_folder_path("/"), None);
    assert_eq!(parse_folder_path("/src-1/"), None);
}

#[cfg(unix)]
#[test]
fn only_images_inside_the_folders_are_served() {
    let outside = tempfile::tempdir().unwrap();
    make(outside.path(), &[("secret.jpg", JPEG), ("notes.txt", b"private")]);
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("Films");
    make(&root, &[("Heat/poster.jpg", JPEG), ("Heat/Heat.mkv", b""), ("Heat/Heat.nfo", b"<movie/>"), (".hidden/poster.jpg", JPEG)]);
    std::os::unix::fs::symlink(outside.path().join("secret.jpg"), root.join("Heat/link.jpg")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("Heat/out")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("notes.txt"), root.join("Heat/notes.jpg")).unwrap();
    std::os::unix::fs::symlink(root.join("Heat/poster.jpg"), root.join("Heat/again.jpg")).unwrap();
    let roots = [root_string(&root)];
    let check = |path: PathBuf| checked_file(&roots, &path, naming::IMAGE_EXTENSIONS);

    assert!(check(root.join("Heat/poster.jpg")).is_ok());
    assert!(check(root.join("Heat/again.jpg")).is_ok(), "a link to an image inside is fine");
    for refused in [
        root.join("Heat/Heat.mkv"),
        root.join("Heat/Heat.nfo"),
        root.join("Heat/missing.jpg"),
        root.join("Heat/../Heat/poster.jpg"),
        root.join("Heat/../../Films/Heat/poster.jpg"),
        root.join(".hidden/poster.jpg"),
        root.join("Heat/link.jpg"),
        root.join("Heat/out/secret.jpg"),
        root.join("Heat/notes.jpg"),
        outside.path().join("secret.jpg"),
        root.clone(),
        PathBuf::from("Heat/poster.jpg"),
        PathBuf::from("/etc/passwd"),
    ] {
        assert!(check(refused.clone()).is_err(), "{refused:?} was served");
    }
    // A root's sibling with a longer name isn't inside it.
    make(folder.path(), &[("Films Extra/poster.jpg", JPEG)]);
    assert!(check(folder.path().join("Films Extra/poster.jpg")).is_err());
}

// MARK: Through Core

fn open_core() -> (tempfile::TempDir, Arc<Core>) {
    let data = tempfile::tempdir().unwrap();
    let core = Core::open(data.path(), Arc::new(MemorySecrets::new())).unwrap();
    (data, core)
}

fn ignore(_: RefreshProgress) {}

#[tokio::test]
async fn media_folder_sources_end_to_end() {
    let (data, core) = open_core();
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("Media");
    make(
        &root,
        &[
            ("Heat (1995)/Heat.mkv", b""),
            ("Heat (1995)/poster.jpg", JPEG),
            ("Heat (1995)/fanart.jpg", JPEG),
            ("Heat (1995)/Heat.en.srt", b"1\n00:00:01,000 --> 00:00:02,000\nHello\n"),
            ("Heat (1995)/Heat.fr.forced.srt", b""),
            ("Severance/Season 1/Severance S01E01.mkv", b""),
        ],
    );
    let config = core.add_folder_source("", &[root_string(&root)]).unwrap();
    assert_eq!((config.kind, config.name.as_str(), config.folders.len()), (SourceKind::Folder, "Media", 1));
    assert!(config.display_host.is_some());

    let status = core.refresh_source(&config.id, &ignore).await.unwrap();
    assert_eq!((status.last_error.as_deref(), status.movie_count, status.series_count), (None, 1, 1));
    let movies = core.movies(&BrowseQuery::default()).unwrap();
    let heat = movies.iter().find(|movie| movie.title == "Heat").unwrap().clone();
    let series = core.series_list(&BrowseQuery::default()).unwrap();
    let episodes = core.episodes(&series[0].id).await.unwrap();
    assert_eq!(episodes.len(), 1);

    // Its artwork is served, and nothing outside its folders.
    let poster = heat.poster_url.clone().unwrap();
    let (source_id, path) = parse_folder_path(url::Url::parse(&poster).unwrap().path()).unwrap();
    assert_eq!(source_id, config.id);
    let (bytes, kind) = core.folder_image(&config.id, &path).await.unwrap();
    assert_eq!((bytes.len(), kind.as_str()), (JPEG.len(), "image/jpeg"));
    assert!(core.folder_image(&config.id, &data.path().join("catalog.sqlite")).await.is_err());
    assert!(core.folder_image(&config.id, &root.join("Heat (1995)/Heat.mkv")).await.is_err());
    assert!(core.folder_image("demo", &path).await.is_err(), "only a media folder source's own");
    let details = core.details(&heat.id).await.unwrap();
    assert_eq!(details.backdrop_urls.len(), 1);

    // Subtitles beside the film, for the player, and only those.
    let subtitles = core.folder_subtitles(&heat.id).unwrap();
    assert_eq!(subtitles.iter().map(|s| s.language.as_str()).collect::<Vec<_>>(), ["eng", "fre"]);
    let text = core.folder_subtitle_file(&subtitles[0].url).await.unwrap();
    assert!(String::from_utf8_lossy(&text).contains("Hello"));
    let elsewhere = folder_file_url(&config.id, &data.path().join("sources.json").to_string_lossy(), None);
    assert!(core.folder_subtitle_file(&elsewhere).await.is_err());
    assert!(core.folder_subtitles(&episodes[0].id).unwrap().is_empty());

    // Watched marks stay through a rescan, and a deleted fanart takes its backdrop with it.
    let watched = xorvio_model::PlaybackProgress {
        item_id: heat.id.clone(),
        kind: xorvio_model::MediaKind::Movie,
        source_id: heat.source_id.clone(),
        series_id: None,
        position_seconds: 0.0,
        duration_seconds: 0.0,
        finished: true,
        updated_at: 0,
    };
    core.set_watched(&[watched], true).unwrap();
    std::fs::remove_file(root.join("Heat (1995)/fanart.jpg")).unwrap();
    core.refresh_source(&config.id, &ignore).await.unwrap();
    let again = core.movie(&heat.id).unwrap().expect("the same id after a rescan");
    assert_eq!(again.title, "Heat");
    assert!(core.progress(&heat.id).unwrap().is_some_and(|progress| progress.finished));
    assert!(core.details(&heat.id).await.unwrap().backdrop_urls.is_empty());

    // The folder goes away: the catalog stays, and the status says why.
    let moved = folder.path().join("Moved");
    std::fs::rename(&root, &moved).unwrap();
    let status = core.refresh_source(&config.id, &ignore).await.unwrap();
    assert!(status.last_error.as_deref().is_some_and(|error| error.contains("“Media” isn't available right now")), "{status:?}");
    assert_eq!(core.movies(&BrowseQuery::default()).unwrap().len(), 1);
    assert_eq!(status.movie_count, 1);
    let playing = core.stream_target(&config.id, &heat.stream, false).unwrap_err().to_string();
    assert!(playing.contains("isn't available right now"), "{playing}");
    std::fs::rename(&moved, &root).unwrap();
    assert!(core.stream_target(&config.id, &heat.stream, false).unwrap().url.starts_with("file://"));

    // Folders can be added and removed in its settings.
    let more = folder.path().join("More");
    empty(&more, &["Arrival (2016).mkv"]);
    let mut edited = core.list_sources().unwrap().into_iter().find(|s| s.config.id == config.id).unwrap().config;
    edited.folders.push(root_string(&more));
    core.update_source(edited.clone(), None).unwrap();
    let status = core.refresh_source(&config.id, &ignore).await.unwrap();
    assert_eq!(status.movie_count, 2);
    edited.folders.remove(0);
    core.update_source(edited, None).unwrap();
    let status = core.refresh_source(&config.id, &ignore).await.unwrap();
    assert_eq!((status.movie_count, status.series_count), (1, 0));

    core.remove_source(&config.id).unwrap();
    assert!(!data.path().join("folder-scans").join(format!("{}.json", config.id)).exists());
}

#[tokio::test]
async fn folders_are_checked_when_chosen() {
    let (_data, core) = open_core();
    let folder = tempfile::tempdir().unwrap();
    let films = folder.path().join("Films");
    empty(&films, &["Kids/Up (2009).mkv", "notes.txt"]);
    let films_path = root_string(&films);
    let kids = root_string(&films.join("Kids"));

    let error = |result: crate::Result<xorvio_model::SourceConfig>| result.unwrap_err().to_string();
    assert!(error(core.add_folder_source("", &[])).contains("missing a folder"));
    assert!(error(core.add_folder_source("", &["relative/path".into()])).contains("isn't a folder Xorvio can use"));
    let system_disk = if cfg!(windows) { r"C:\" } else { "/" };
    assert!(error(core.add_folder_source("", &[system_disk.into()])).contains("rather than the whole disk"));
    assert!(error(core.add_folder_source("", &[format!("{films_path}/missing")])).contains("can't be found"));
    assert!(error(core.add_folder_source("", &[format!("{films_path}/notes.txt")])).contains("isn't a folder"));
    assert!(error(core.add_folder_source("", &[films_path.clone(), kids.clone()])).contains("“Kids” is inside “Films”"));

    let added = core.add_folder_source("My Films", &[format!("{films_path}/"), films_path.clone()]).unwrap();
    assert_eq!((added.name.as_str(), added.folders.clone()), ("My Films", vec![films_path.clone()]), "tidied and once each");
    assert!(error(core.add_folder_source("", std::slice::from_ref(&kids))).contains("already part of “My Films”"));

    // A folder the source already has may be away; a new one must be there.
    let mut edited = added.clone();
    edited.folders = vec![films_path, root_string(&folder.path().join("Away"))];
    assert!(core.update_source(edited, None).unwrap_err().to_string().contains("can't be found"));
}

#[tokio::test]
async fn folders_are_read_again_each_launch_and_while_one_is_away() {
    let (_data, core) = open_core();
    let folder = tempfile::tempdir().unwrap();
    empty(folder.path(), &["Heat (1995).mkv"]);
    let config = core.add_folder_source("", &[root_string(folder.path())]).unwrap();
    assert!(core.sources_due_for_refresh(crate::now()).unwrap().contains(&config.id), "never read");
    core.refresh_source(&config.id, &ignore).await.unwrap();
    assert!(!core.sources_due_for_refresh(crate::now()).unwrap().contains(&config.id), "just read");

    let mut status = core.catalog.status(&config.id).unwrap().unwrap();
    status.catalog_warning = Some("“Films” isn't available right now.".into());
    core.catalog.save_status(&config.id, &status).unwrap();
    assert!(core.sources_due_for_refresh(crate::now()).unwrap().contains(&config.id), "a folder is away");

    status.catalog_warning = None;
    status.last_success = Some(crate::now() - 3600);
    core.catalog.save_status(&config.id, &status).unwrap();
    assert!(core.sources_due_for_refresh(crate::now()).unwrap().contains(&config.id), "read before this launch");
}

#[test]
fn many_videos_of_one_name_in_a_folder_each_show_but_versions_still_group() {
    let folder = tempfile::tempdir().unwrap();
    let mut paths: Vec<String> = (1..=6).map(|n| format!("Visualization Videos/VJ LOOPS 2019-{n}.mp4")).collect();
    paths.extend(["Films/Heat (1995)/Heat (1995) - 1080p.mkv".into(), "Films/Heat (1995)/Heat (1995) - 4K.mkv".into()]);
    paths.extend((0..3).map(|n| format!("imported/3174fb7a-9884-4147-986c-736748bf7691/0{n}800d3d-f0ca-4d66-b9f7-f887f0c45ab0.mp4")));
    empty(folder.path(), &paths.iter().map(String::as_str).collect::<Vec<_>>());
    let snapshot = scan_once(folder.path());

    let mut loops: Vec<&str> = snapshot.movies.iter().map(|movie| movie.title.as_str()).filter(|title| title.starts_with("VJ LOOPS")).collect();
    loops.sort();
    assert_eq!(loops, ["VJ LOOPS 2019-1", "VJ LOOPS 2019-2", "VJ LOOPS 2019-3", "VJ LOOPS 2019-4", "VJ LOOPS 2019-5", "VJ LOOPS 2019-6"], "six videos, not six versions of one");
    assert_eq!(snapshot.movies.iter().filter(|movie| movie.title == "Heat").count(), 2, "a film's 1080p and 4K copies are still versions of it");
    let mut hashed: Vec<&str> = snapshot.movies.iter().map(|movie| movie.title.as_str()).filter(|title| title.ends_with("-f0ca-4d66-b9f7-f887f0c45ab0")).collect();
    hashed.sort();
    assert_eq!(hashed.len(), 3, "named by their files, not their folder: {hashed:?}");
    assert_eq!(hashed.iter().collect::<std::collections::HashSet<_>>().len(), 3);
}

#[test]
fn camera_files_are_named_with_their_folder() {
    let folder = tempfile::tempdir().unwrap();
    empty(folder.path(), &["Beach Trip/video 00001.mp4", "Beach Trip/video 00002.mp4", "Party/video 00001.mp4", "IMG_1234.MOV", "Films/Heat (1995)/Heat.mkv"]);
    let snapshot = scan_once(folder.path());

    let root = naming::own_name(naming::folder_name(&root_string(folder.path())));
    let mut titles: Vec<String> = snapshot.movies.iter().map(|movie| movie.title.clone()).collect();
    titles.sort();
    let mut expected = vec!["Beach Trip · video 00001".to_string(), "Beach Trip · video 00002".into(), "Heat".into(), "Party · video 00001".into(), format!("{root} · IMG_1234")];
    expected.sort();
    assert_eq!(titles, expected);
}

#[tokio::test]
async fn videos_without_artwork_take_a_frame_and_tmdbs_guesses_by_name_go() {
    let (_data, core) = open_core();
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("Videos");
    empty(&root, &["Heat (1995)/Heat.mkv", "Beach Trip/video 00001.mp4", "XTC - Dear God.mp4"]);
    let config = core.add_folder_source("", &[root_string(&root)]).unwrap();
    core.refresh_source(&config.id, &ignore).await.unwrap();
    let movies = core.movies(&BrowseQuery::default()).unwrap();
    let id = |title: &str| movies.iter().find(|movie| movie.title == title).unwrap_or_else(|| panic!("{title} in {:?}", movies.iter().map(|m| &m.title).collect::<Vec<_>>())).id.clone();
    let (heat, beach, song) = (id("Heat"), id("Beach Trip · video 00001"), id("XTC - Dear God"));
    let ids = vec![heat.clone(), beach.clone(), song.clone()];

    // None has artwork of its own: each can have a frame of its file.
    let wanted = core.videos_without_pictures(&ids).unwrap();
    assert_eq!(wanted.len(), 3);
    assert!(wanted.iter().all(|(_, source, file)| source == &config.id && file.starts_with(&root)));

    // A frame kept for one shows in lists from then on, and it isn't wanted again.
    core.keep_video_picture(&beach, &config.id, "xorvio-still://localhost/0123456789abcdef0123456789abcdef.jpg").unwrap();
    let shown = core.movie(&beach).unwrap().unwrap();
    assert_eq!(shown.poster_url.as_deref(), Some("xorvio-still://localhost/0123456789abcdef0123456789abcdef.jpg"));
    assert_eq!(core.videos_without_pictures(&ids).unwrap().len(), 2);

    // TMDB's posters found by name: a film's (it has a year) stays; a music
    // video's goes when the folder is read again, so it takes a frame too.
    core.catalog.save_poster_lookup(&heat, &config.id, Some("https://image.tmdb.org/t/p/w342/heat.jpg"), crate::now()).unwrap();
    core.catalog.save_poster_lookup(&song, &config.id, Some("https://image.tmdb.org/t/p/w342/song.jpg"), crate::now()).unwrap();
    core.refresh_source(&config.id, &ignore).await.unwrap();
    assert_eq!(core.movie(&heat).unwrap().unwrap().poster_url.as_deref(), Some("https://image.tmdb.org/t/p/w342/heat.jpg"));
    assert_eq!(core.movie(&song).unwrap().unwrap().poster_url, None);
    assert!(core.movie(&beach).unwrap().unwrap().poster_url.is_some(), "a kept frame stays");
    let wanted: Vec<String> = core.videos_without_pictures(&ids).unwrap().into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(wanted, [song]);
}
