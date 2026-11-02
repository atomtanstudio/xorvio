//! A media folder's files made into a catalog: films, series and their
//! episodes, details from .nfo files, and artwork from the images beside
//! them, named as Kodi, Jellyfin and Plex expect:
//!
//! ```text
//! Heat (1995)/Heat.mkv, poster.jpg, fanart.jpg, clearlogo.png, movie.nfo
//! Arrival.mkv, Arrival-poster.jpg, Arrival-fanart.jpg, Arrival.nfo
//! Severance/poster.jpg, fanart.jpg, tvshow.nfo, season01-poster.jpg
//! Severance/Season 1/poster.jpg (the season's), S01E01.mkv, S01E01-thumb.jpg, S01E01.nfo
//! ```
//!
//! Ids are built from paths below the folder (films) and from names and
//! numbers (series and episodes), never from what an .nfo says, so they stay
//! the same from one scan to the next and progress, watched marks and the
//! watchlist stay with them.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use xorvio_model::{Category, CategoryKind, Episode, Movie, SeasonInfo, Series, StreamLocator};

use super::folder_file_url;
use super::naming::{self, IMAGE_EXTENSIONS, Item, VIDEO_EXTENSIONS};
use super::nfo::Nfo;
use super::scan::{FileEntry, Listing};
use crate::details::DetailsRecord;
use crate::snapshot::CatalogSnapshot;
use crate::sources::ids;

/// More films of one name than this in one folder aren't versions of a film
/// (a 1080p and a 4K copy, say) but videos of their own: a folder of
/// "VJ LOOPS 2019-1" to "-64", or of files with random names in a folder
/// with one.
const MOST_VERSIONS: usize = 4;

/// Small files called "sample" are trailers' cousins, not films.
const SAMPLE_BYTES: u64 = 300 << 20;
/// A real MPEG-TS video is far bigger; smaller .ts files are usually code.
const SMALLEST_TS: u64 = 512 << 10;

/// One chosen folder's files, indexed for finding what's beside a video.
struct Folder<'a> {
    source_id: &'a str,
    root: &'a str,
    /// Short and stable, so files with the same path in two folders keep ids of their own.
    tag: String,
    /// The folder's own name, which names a film or series when it's one.
    name: &'a str,
    /// By lowercased path, so "Poster.JPG" is found as poster.jpg.
    images: HashMap<String, &'a FileEntry>,
    nfos: HashMap<String, &'a Nfo>,
}

impl<'a> Folder<'a> {
    fn new(source_id: &'a str, root: &'a str, listing: &'a Listing) -> Self {
        let mut images = HashMap::new();
        let mut nfos = HashMap::new();
        for file in &listing.files {
            if naming::has_extension(&file.path, IMAGE_EXTENSIONS) {
                images.insert(lookup_key(&file.path), file);
            } else if let Some(nfo) = file.nfo.as_ref().filter(|nfo| !nfo.is_empty()) {
                nfos.insert(lookup_key(&file.path), nfo);
            }
        }
        Self { source_id, root, tag: root_tag(root), name: naming::folder_name(root), images, nfos }
    }

    fn absolute(&self, relative: &str) -> String {
        Path::new(self.root).join(relative).to_string_lossy().into_owned()
    }

    /// The first of `names` (without extension) that's an image in `folder`.
    fn image(&self, folder: &str, names: &[String]) -> Option<String> {
        names
            .iter()
            .find_map(|name| IMAGE_EXTENSIONS.iter().find_map(|extension| self.images.get(&lookup_key(&joined(folder, &format!("{name}.{extension}"))))))
            .map(|file| folder_file_url(self.source_id, &self.absolute(&file.path), Some(file.modified)))
    }

    fn nfo(&self, folder: &str, names: &[String]) -> Option<&'a Nfo> {
        names.iter().find_map(|name| self.nfos.get(&lookup_key(&joined(folder, &format!("{name}.nfo")))).copied())
    }
}

fn joined(folder: &str, name: &str) -> String {
    if folder.is_empty() { name.to_string() } else { format!("{folder}/{name}") }
}

fn lookup_key(path: &str) -> String {
    path.nfc().collect::<String>().to_lowercase()
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

/// A folder's tag in ids: the start of a hash of its path.
fn root_tag(root: &str) -> String {
    let digest = Sha256::digest(root.nfc().collect::<String>().as_bytes());
    digest.iter().take(4).map(|byte| format!("{byte:02x}")).collect()
}

/// The id a scan gives the film in `relative` (a path below `root`, as the
/// scan lists it), for a file moved or renamed to keep what's kept for it.
pub(crate) fn film_id(source_id: &str, root: &str, relative: &str) -> String {
    ids::movie(source_id, &movie_key(&root_tag(root), relative))
}

/// A film's key: its folder's tag and its path below it, with `|` (which
/// separates the parts of ids) written out.
fn movie_key(tag: &str, path: &str) -> String {
    let path: String = path.nfc().collect();
    format!("{tag}/{}", path.replace('%', "%25").replace('|', "%7C"))
}

/// A series' key: its name, and its year when the name gives one, so
/// "Doctor Who (1963)" and "Doctor Who (2005)" stay apart.
fn series_key(name: &str, year: Option<i64>) -> String {
    let folded = naming::fold_key(name);
    let folded = if folded.is_empty() { name.to_lowercase() } else { folded };
    match year {
        Some(year) => format!("s:{folded}:{year}"),
        None => format!("s:{folded}"),
    }
}

/// When a file came to be there; none for one that couldn't be looked at.
fn known_time(added: i64) -> Option<i64> {
    (added > 0).then_some(added)
}

fn is_video(file: &FileEntry) -> bool {
    if !naming::has_extension(&file.path, VIDEO_EXTENSIONS) {
        return false;
    }
    let lower = file.path.to_lowercase();
    let sample = lower.split(|c: char| !c.is_alphanumeric()).any(|word| word == "sample");
    !(sample && file.size < SAMPLE_BYTES) && !(naming::has_extension(&lower, &["ts"]) && file.size < SMALLEST_TS)
}

/// A series as its episodes turn up, perhaps from several folders.
struct Show {
    title: String,
    year: Option<i64>,
    /// The series' own folders (folder index, path below it), in the order found.
    places: Vec<(usize, String)>,
    /// Each season's own folders, for season posters.
    season_places: BTreeMap<i64, Vec<(usize, String)>>,
    /// By (season, number), with the file's size: of two copies, the bigger stays.
    episodes: BTreeMap<(i64, i64), (u64, Episode)>,
    newest: Option<i64>,
}

/// Which folders hold one film (perhaps in several versions), so that their
/// poster.jpg and movie.nfo are that film's. The chosen folder itself counts
/// only when everything in it is that one film.
struct FilmFolders<'a> {
    folders: HashMap<&'a str, Option<String>>,
    whole: Option<Option<String>>,
}

impl<'a> FilmFolders<'a> {
    fn new(videos: &[(&'a FileEntry, Item)]) -> Self {
        let mut folders: HashMap<&str, Option<String>> = HashMap::new();
        let mut whole: Option<Option<String>> = None;
        for (file, item) in videos {
            let key = match item {
                Item::Movie { title, .. } => Some(naming::fold_key(title)),
                Item::Episode { .. } => None,
            };
            let merge = |slot: &mut Option<String>| {
                if *slot != key {
                    *slot = None;
                }
            };
            folders.entry(naming::parent_path(&file.path)).and_modify(merge).or_insert_with(|| key.clone());
            match whole.as_mut() {
                Some(slot) => merge(slot),
                None => whole = Some(key.clone()),
            }
        }
        Self { folders, whole }
    }

    fn holds_one_film(&self, folder: &str) -> bool {
        if folder.is_empty() {
            matches!(self.whole, Some(Some(_)))
        } else {
            matches!(self.folders.get(folder), Some(Some(_)))
        }
    }
}

/// Videos named apart, so every one shows (ids come from paths, so nothing
/// else changes). A camera's or phone's name ("video 00001", "IMG_1234")
/// goes after its folder's ("Beach Trip · video 00001"), as the same names
/// turn up in every folder. Films sharing a name and year in one folder are
/// versions of one film, unless there are more than [`MOST_VERSIONS`]: then
/// each is named by its own file.
fn named_apart(videos: &mut [(&FileEntry, Item)], root_name: &str) {
    for (file, item) in videos.iter_mut() {
        let base = naming::base_name(&file.path);
        if let Item::Movie { title, year } = item
            && naming::camera_name(base)
        {
            let dir = naming::parent_path(&file.path);
            let folder = if dir.is_empty() { root_name } else { naming::folder_name(dir) };
            *title = format!("{} · {}", naming::own_name(folder), base.trim());
            *year = None;
        }
    }
    let key = |file: &FileEntry, title: &str, year: Option<i64>| (naming::parent_path(&file.path).to_string(), naming::fold_key(title), year);
    let mut counts: HashMap<(String, String, Option<i64>), usize> = HashMap::new();
    for (file, item) in videos.iter() {
        if let Item::Movie { title, year } = item {
            *counts.entry(key(file, title, *year)).or_default() += 1;
        }
    }
    for (file, item) in videos.iter_mut() {
        if let Item::Movie { title, year } = item
            && counts.get(&key(file, title, *year)).is_some_and(|count| *count > MOST_VERSIONS)
        {
            *title = naming::own_name(naming::base_name(&file.path));
        }
    }
}

/// The catalog of a media folder source, from its folders' listings.
pub(super) fn snapshot(source_id: &str, folders: &[(String, Listing)]) -> CatalogSnapshot {
    let films = Category {
        id: ids::category(source_id, CategoryKind::Movie, "films"),
        source_id: source_id.into(),
        kind: CategoryKind::Movie,
        name: "Films".into(),
        sort_index: 0,
    };
    let shelf = Category {
        id: ids::category(source_id, CategoryKind::Series, "series"),
        source_id: source_id.into(),
        kind: CategoryKind::Series,
        name: "Series".into(),
        sort_index: 1,
    };
    let indexed: Vec<Folder<'_>> = folders.iter().map(|(root, listing)| Folder::new(source_id, root, listing)).collect();
    let mut snapshot = CatalogSnapshot::default();
    let mut shows: BTreeMap<String, Show> = BTreeMap::new();

    for (index, ((_, listing), folder)) in folders.iter().zip(&indexed).enumerate() {
        let mut videos: Vec<(&FileEntry, Item)> = listing
            .files
            .iter()
            .filter(|file| is_video(file))
            .filter_map(|file| naming::classify(&file.path, folder.name).map(|item| (file, item)))
            .collect();
        named_apart(&mut videos, folder.name);
        let film_folders = FilmFolders::new(&videos);
        for (file, item) in &videos {
            let base = naming::base_name(&file.path);
            let dir = naming::parent_path(&file.path);
            match item {
                Item::Movie { title, year } => {
                    let own = film_folders.holds_one_film(dir);
                    let with_own = |specific: Vec<String>, shared: &[&str]| -> Vec<String> {
                        specific.into_iter().chain(shared.iter().filter(|_| own).map(|name| name.to_string())).collect()
                    };
                    let poster = folder.image(dir, &with_own(vec![format!("{base}-poster"), base.to_string()], &["poster", "folder", "cover", "movie"]));
                    let backdrop = folder.image(dir, &with_own(vec![format!("{base}-fanart"), format!("{base}-backdrop")], &["fanart", "backdrop", "background"]));
                    let logo = folder.image(dir, &with_own(vec![format!("{base}-clearlogo")], &["clearlogo", "logo"]));
                    let nfo = folder.nfo(dir, &with_own(vec![base.to_string()], &["movie"]));
                    let id = ids::movie(source_id, &movie_key(&folder.tag, &file.path));
                    snapshot.movies.push(Movie {
                        id: id.clone(),
                        source_id: source_id.into(),
                        title: nfo.and_then(|nfo| nfo.title.clone()).unwrap_or_else(|| title.clone()),
                        year: nfo.and_then(|nfo| nfo.year).or(*year),
                        poster_url: poster,
                        backdrop_url: backdrop.clone(),
                        category_id: Some(films.id.clone()),
                        rating: nfo.and_then(|nfo| nfo.rating),
                        added_at: known_time(file.added),
                        plot: nfo.and_then(|nfo| nfo.plot.clone()),
                        genre: nfo.and_then(Nfo::genre),
                        tmdb_id: nfo.and_then(|nfo| nfo.tmdb_id.clone()),
                        imdb_id: nfo.and_then(|nfo| nfo.imdb_id.clone()),
                        duration_seconds: nfo.and_then(Nfo::duration_seconds),
                        stream: StreamLocator::File { path: folder.absolute(&file.path) },
                    });
                    let details = with_artwork(nfo.map(Nfo::details).unwrap_or_default(), backdrop, logo);
                    if !details.is_empty() {
                        snapshot.details.insert(id, details);
                    }
                }
                Item::Episode { series, year, season, number, last, title } => {
                    let key = series_key(series, *year);
                    let series_id = ids::series(source_id, &key);
                    let season_folder = dir.rsplit('/').next().and_then(naming::season_number).is_some();
                    let show_folder = if season_folder { naming::parent_path(dir) } else { dir };
                    let nfo = folder.nfo(dir, &[base.to_string()]);
                    let fallback = match last {
                        Some(last) => format!("Episodes {number}–{last}"),
                        None => format!("Episode {number}"),
                    };
                    let episode = Episode {
                        id: ids::episode(&series_id, &format!("e:{season}:{number}")),
                        series_id,
                        source_id: source_id.into(),
                        season: *season,
                        number: *number,
                        title: nfo.and_then(|nfo| nfo.title.clone()).or_else(|| title.clone()).unwrap_or(fallback),
                        plot: nfo.and_then(|nfo| nfo.plot.clone()),
                        duration_seconds: nfo.and_then(Nfo::duration_seconds),
                        still_url: folder.image(dir, &[format!("{base}-thumb"), base.to_string()]),
                        stream: StreamLocator::File { path: folder.absolute(&file.path) },
                        air_date: nfo.and_then(Nfo::air_date),
                    };
                    let show = shows.entry(key).or_insert_with(|| Show {
                        title: series.clone(),
                        year: *year,
                        places: Vec::new(),
                        season_places: BTreeMap::new(),
                        episodes: BTreeMap::new(),
                        newest: None,
                    });
                    let place = (index, show_folder.to_string());
                    if !show.places.contains(&place) {
                        show.places.push(place);
                    }
                    if season_folder {
                        let places = show.season_places.entry(*season).or_default();
                        let place = (index, dir.to_string());
                        if !places.contains(&place) {
                            places.push(place);
                        }
                    }
                    // Two copies of one episode: the bigger file is usually the better one.
                    if show.episodes.get(&(*season, *number)).is_none_or(|(size, _)| *size < file.size) {
                        show.episodes.insert((*season, *number), (file.size, episode));
                    }
                    show.newest = show.newest.max(known_time(file.added));
                }
            }
        }
    }

    // A folder of several shows (TV/The.Bear.S01E01.mkv, TV/Fargo 3x05.avi)
    // isn't any one show's, so its poster.jpg and tvshow.nfo go to neither.
    let mut shows_per_place: HashMap<(usize, String), usize> = HashMap::new();
    for show in shows.values() {
        for place in &show.places {
            *shows_per_place.entry(place.clone()).or_default() += 1;
        }
    }
    let mut ordered: Vec<(String, Show)> = shows.into_iter().collect();
    ordered.sort_by_cached_key(|(key, show)| (naming::fold_key(&show.title), key.clone()));
    for (key, show) in ordered {
        let id = ids::series(source_id, &key);
        let own = show.places.iter().find(|place| shows_per_place.get(*place) == Some(&1)).map(|(index, dir)| (&indexed[*index], dir.as_str()));
        let image = |list: &[&str]| own.and_then(|(folder, dir)| folder.image(dir, &names(list)));
        let nfo = own.and_then(|(folder, dir)| folder.nfo(dir, &names(&["tvshow"])));
        let backdrop = image(&["fanart", "backdrop", "background"]);
        let logo = image(&["clearlogo", "logo"]);
        let mut details = with_artwork(nfo.map(Nfo::details).unwrap_or_default(), backdrop.clone(), logo);
        details.seasons = season_posters(&show, own, &indexed);
        snapshot.series.push(Series {
            id: id.clone(),
            source_id: source_id.into(),
            title: nfo.and_then(|nfo| nfo.title.clone()).unwrap_or_else(|| show.title.clone()),
            year: nfo.and_then(|nfo| nfo.year).or(show.year),
            poster_url: image(&["poster", "folder", "cover", "show"]),
            backdrop_url: backdrop,
            category_id: Some(shelf.id.clone()),
            rating: nfo.and_then(|nfo| nfo.rating),
            added_at: show.newest,
            plot: nfo.and_then(|nfo| nfo.plot.clone()),
            genre: nfo.and_then(Nfo::genre),
            tmdb_id: nfo.and_then(|nfo| nfo.tmdb_id.clone()),
            imdb_id: nfo.and_then(|nfo| nfo.imdb_id.clone()),
            remote_id: None,
        });
        if !details.is_empty() {
            snapshot.details.insert(id, details);
        }
        snapshot.episodes.extend(show.episodes.into_values().map(|(_, episode)| episode));
    }

    if !snapshot.movies.is_empty() {
        snapshot.categories.push(films);
    }
    if !snapshot.series.is_empty() {
        snapshot.categories.push(shelf);
    }
    snapshot.derive_capabilities();
    snapshot
}

fn with_artwork(mut details: DetailsRecord, backdrop: Option<String>, logo: Option<String>) -> DetailsRecord {
    if let Some(backdrop) = backdrop {
        details.backdrop_urls = vec![backdrop];
    }
    details.logo_url = logo;
    details
}

/// Each season's poster: "season01-poster.jpg" in the series' folder
/// ("season-specials-poster.jpg" for specials), or the season folder's own
/// poster.jpg or folder.jpg.
fn season_posters(show: &Show, own: Option<(&Folder<'_>, &str)>, folders: &[Folder<'_>]) -> Vec<SeasonInfo> {
    let mut seasons: Vec<i64> = show.episodes.keys().map(|(season, _)| *season).collect();
    seasons.dedup();
    seasons
        .into_iter()
        .filter_map(|season| {
            let mut beside = vec![format!("season{season:02}-poster"), format!("season{season}-poster")];
            if season == 0 {
                beside.insert(0, "season-specials-poster".into());
            }
            let cover = own
                .and_then(|(folder, dir)| folder.image(dir, &beside))
                .or_else(|| {
                    show.season_places.get(&season)?.iter().find_map(|(index, dir)| folders[*index].image(dir, &names(&["poster", "folder", "cover"])))
                })?;
            Some(SeasonInfo { number: season, cover_url: Some(cover), ..Default::default() })
        })
        .collect()
}
