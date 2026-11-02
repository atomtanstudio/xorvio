//! A media folder's own library page: its folders and videos, filtered by
//! what the video files hold (GIFs, animations, videos; short or long), and
//! the changes someone asks for there: deleting, moving and renaming videos,
//! and making, renaming, moving and deleting folders.
//!
//! Nothing outside the source's own folders is ever touched, and nothing is
//! deleted at once: deleted files and folders go to the folder's Recently
//! Deleted (a hidden folder inside it, so even thousands go in an instant on
//! a network share, which has no Trash), to be put back where they were or
//! deleted for good, which happens by itself after 30 days. A video's
//! pictures, .nfo and subtitles beside it go with it. A video moved or
//! renamed keeps its progress, watchlist place, frame and the rest under its
//! new id. The catalog follows each change at once; reading the source again
//! afterwards ([`Core::read_folder_again`]) brings the rest in line.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use xorvio_model::{DeletedBatch, DeletedEntry, FileFacts, FolderEntry, FolderListing, FolderQuery, FolderVideo, RefreshProgress, StreamLocator};

use crate::sources::folder::{self, naming};
use crate::store::catalog::{FolderFilter, FolderPage, VideoMove};
use crate::store::lock;
use crate::{Core, Result, now};

/// Where a media folder's deleted files wait, inside it; hidden, so never listed.
pub const RECENTLY_DELETED: &str = ".xorvio-recently-deleted";
/// How long they wait before they're deleted for good.
const KEEP_DELETED_FOR: i64 = 30 * 86_400;
/// The longest name a file or folder is given here.
const LONGEST_NAME: usize = 200;

/// What one deletion put in Recently Deleted, kept beside it as `<id>.json`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    deleted_at: i64,
    entries: Vec<DeletedEntry>,
}

/// A place inside one of a source's folders.
struct Inside {
    root: String,
    /// Below the root, with the platform's separators; empty for the root itself.
    relative: PathBuf,
    path: PathBuf,
}

impl Inside {
    /// The relative path as the scan writes it (with "/"), which ids are made from.
    fn listed(&self) -> String {
        listed(&self.relative)
    }

    /// A file in the same folder as this.
    fn sibling(&self, file: &Path) -> Inside {
        Inside { root: self.root.clone(), relative: self.relative.with_file_name(file_name(file)), path: file.to_path_buf() }
    }

    /// A video's file once it's at `path`, in the same folder of the source as this.
    fn below(&self, path: &Path) -> NewFile {
        let listed = listed(path.strip_prefix(&self.root).unwrap_or(path));
        NewFile { path: Path::new(&self.root).join(&listed).to_string_lossy().into_owned(), root: self.root.clone(), listed }
    }
}

/// A file's new place, as the scan will list it.
struct NewFile {
    root: String,
    listed: String,
    /// As the film's stream gives it.
    path: String,
}

/// A folder, and the videos in it and below it with their paths below it.
type FolderWithVideos = (Inside, Vec<(String, PathBuf)>);

/// One rename on disk, and the videos it moves.
struct Step {
    from: PathBuf,
    to: PathBuf,
    moved: Vec<(String, NewFile)>,
}

fn listed(relative: &Path) -> String {
    relative.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

impl Core {
    // MARK: Listing

    /// One page of a media folder: the folders in the one asked for (on the
    /// first page), and the videos in it (or below it), filtered.
    pub fn folder_listing(&self, query: &FolderQuery) -> Result<FolderListing> {
        let roots = self.folder_roots(&query.source_id)?;
        let place = asked_place(&roots, query)?;
        if let Some(place) = &place {
            Disk::default().check(place, true)?;
        }
        let source_name = self.config(&query.source_id)?.name;
        let trail = match &place {
            Some(place) => trail(place, &source_name, roots.len() > 1),
            None => Vec::new(),
        };
        let folders = if query.offset > 0 {
            Vec::new()
        } else {
            match &place {
                Some(place) => self.folders_in(&query.source_id, place)?,
                None => roots.iter().map(|root| self.folder_entry(&query.source_id, root)).collect::<Result<_>>()?,
            }
        };
        // Several folders and none chosen: their videos only when asked for everything below.
        let prefix = place.as_ref().map(stored_prefix).unwrap_or_default();
        if place.is_none() && !query.below {
            return Ok(FolderListing { folder: None, trail, folders, videos: Vec::new(), total: 0, unread: 0 });
        }
        let filter = FolderFilter {
            source_id: &query.source_id,
            prefix: &prefix,
            below: query.below,
            kind: query.kind,
            length: query.length,
            sort: query.sort,
            limit: query.limit.clamp(1, 500),
            offset: query.offset.max(0),
        };
        let FolderPage { videos: found, total, unread } = self.catalog.folder_videos(&filter)?;
        let filled = self.catalog.filled_posters(&found.iter().filter(|(movie, _)| movie.poster_url.is_none()).map(|(movie, _)| movie.id.clone()).collect::<Vec<_>>())?;
        let videos = found
            .into_iter()
            .map(|(mut movie, facts)| {
                if movie.poster_url.is_none() {
                    movie.poster_url = filled.get(&movie.id).cloned();
                }
                if movie.duration_seconds.is_none() {
                    movie.duration_seconds = facts.and_then(|facts| facts.duration_seconds).map(|seconds| seconds.round() as i64);
                }
                let path = match &movie.stream {
                    StreamLocator::File { path } => relative_to(&roots, Path::new(path)).unwrap_or_else(|| path.clone()),
                    _ => String::new(),
                };
                FolderVideo { kind: facts.and_then(|facts| facts.kind()), movie: Self::present_movie(movie), path, facts }
            })
            .collect();
        Ok(FolderListing { folder: place.map(|place| place.path.to_string_lossy().into_owned()), trail, folders, videos, total, unread })
    }

    /// Every video a listing would show on all its pages, for Select All.
    pub fn folder_video_ids(&self, query: &FolderQuery) -> Result<Vec<String>> {
        let roots = self.folder_roots(&query.source_id)?;
        let place = asked_place(&roots, query)?;
        if place.is_none() && !query.below {
            return Ok(Vec::new());
        }
        let prefix = place.as_ref().map(stored_prefix).unwrap_or_default();
        self.catalog.folder_video_ids(&FolderFilter {
            source_id: &query.source_id,
            prefix: &prefix,
            below: query.below,
            kind: query.kind,
            length: query.length,
            sort: query.sort,
            limit: i64::MAX,
            offset: 0,
        })
    }

    /// The folders directly in `path`, with how many videos each holds.
    fn folders_in(&self, source_id: &str, place: &Inside) -> Result<Vec<FolderEntry>> {
        let counts = self.catalog.folder_counts(source_id, &stored_prefix(place))?;
        let mut folders: Vec<FolderEntry> = std::fs::read_dir(&place.path)
            .with_context(|| format!("“{}” couldn't be read.", place.path.display()))?
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                (!name.starts_with('.') && !folder::skipped_folder(&name, 0)).then(|| FolderEntry {
                    videos: counts.get(&name).copied().unwrap_or(0),
                    path: entry.path().to_string_lossy().into_owned(),
                    name,
                })
            })
            .collect();
        folders.sort_by_key(|folder| folder.name.to_lowercase());
        Ok(folders)
    }

    /// One of a source's several folders, with how many videos are in it and below.
    fn folder_entry(&self, source_id: &str, root: &str) -> Result<FolderEntry> {
        let place = Inside { root: root.to_string(), relative: PathBuf::new(), path: PathBuf::from(root) };
        Ok(FolderEntry { name: naming::folder_name(root).to_string(), path: root.to_string(), videos: self.videos_below(source_id, &place)? })
    }

    fn videos_below(&self, source_id: &str, place: &Inside) -> Result<i64> {
        let prefix = stored_prefix(place);
        let total = self.catalog.folder_videos(&FolderFilter {
            source_id,
            prefix: &prefix,
            below: true,
            kind: None,
            length: None,
            sort: xorvio_model::FolderSort::Added,
            limit: 0,
            offset: 0,
        })?
        .total;
        Ok(total)
    }

    // MARK: What the files hold

    /// Up to `limit` of the media folders' videos whose files haven't been read yet.
    pub fn videos_without_facts(&self, limit: i64) -> Result<Vec<(String, String, PathBuf)>> {
        let sources: Vec<String> = self.active_source_ids().into_iter().filter(|id| self.is_folder_source(id)).collect();
        if sources.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self.catalog.videos_without_facts(&sources, limit)?.into_iter().map(|(id, source, path)| (id, source, PathBuf::from(path))).collect())
    }

    /// Keeps what a video's file holds (`None`: it couldn't be read), so it's read once.
    pub fn keep_file_facts(&self, item_id: &str, source_id: &str, facts: Option<&FileFacts>) -> Result<()> {
        self.catalog.save_file_facts(item_id, source_id, facts, now())
    }

    // MARK: Deleting

    /// Moves videos (with what's beside them) and folders to Recently
    /// Deleted. Returns how many went. They're gone from the catalog at
    /// once; [`Core::read_folder_again`] brings the rest in line.
    pub async fn delete_folder_items(&self, source_id: &str, videos: &[String], folders: &[String]) -> Result<usize> {
        let roots = self.folder_roots(source_id)?;
        let folders = self.folders_with_videos(source_id, &roots, folders)?;
        let videos = self.video_places(&roots, videos)?;
        let (gone, outcome) = tokio::task::spawn_blocking(move || {
            let mut disk = Disk::default();
            // Folders first: a video in one of them goes with it.
            let mut places: Vec<(Inside, bool, Vec<String>)> = Vec::new();
            for (folder, inside_it) in folders {
                if let Err(error) = disk.check(&folder, false) {
                    return (Vec::new(), Err(error));
                }
                places.push((folder, true, inside_it.into_iter().map(|(id, _)| id).collect()));
            }
            for (video, id) in videos {
                if places.iter().any(|(folder, is_folder, _)| *is_folder && video.path.starts_with(&folder.path)) {
                    continue;
                }
                if let Err(error) = disk.check(&video, false) {
                    return (Vec::new(), Err(error));
                }
                for file in disk.beside(&video.path) {
                    places.push((video.sibling(&file), false, Vec::new()));
                }
                places.push((video, false, vec![id]));
            }
            let id = batch_id();
            let roots: HashSet<String> = places.iter().map(|(place, _, _)| place.root.clone()).collect();
            let mut gone = Vec::new();
            let mut by_root: HashMap<String, Vec<DeletedEntry>> = HashMap::new();
            let mut outcome = Ok(());
            for (place, is_folder, ids) in places {
                let to = Path::new(&place.root).join(RECENTLY_DELETED).join(&id).join(&place.relative);
                if let Err(error) = to.parent().map(std::fs::create_dir_all).transpose() {
                    outcome = Err(disk_error("made", &Path::new(&place.root).join(RECENTLY_DELETED), error));
                    break;
                }
                if let Err(error) = renamed(&place.path, &to) {
                    outcome = Err(disk_error("deleted", &place.path, error));
                    break;
                }
                by_root.entry(place.root.clone()).or_default().push(DeletedEntry { path: place.listed(), folder: is_folder });
                gone.extend(ids);
            }
            // A deletion that took nothing from a folder leaves nothing behind in it.
            for root in roots.iter().filter(|root| !by_root.contains_key(*root)) {
                remove_empty(&Path::new(root).join(RECENTLY_DELETED).join(&id));
            }
            // Whatever went before a failure can still be put back.
            for (root, entries) in by_root {
                let manifest = Manifest { deleted_at: now(), entries };
                let written = serde_json::to_vec_pretty(&manifest).map_err(anyhow::Error::from).and_then(|bytes| {
                    Ok(std::fs::write(Path::new(&root).join(RECENTLY_DELETED).join(format!("{id}.json")), bytes)?)
                });
                if let Err(error) = written {
                    outcome = outcome.and(Err(error));
                }
            }
            (gone, outcome)
        })
        .await?;
        self.catalog.forget_videos(&gone)?;
        outcome.map(|_| gone.len())
    }

    /// What's in a media folder's Recently Deleted, newest first. Anything
    /// there more than 30 days goes for good first.
    pub fn recently_deleted(&self, source_id: &str) -> Result<Vec<DeletedBatch>> {
        let roots = self.folder_roots(source_id)?;
        let mut batches = Vec::new();
        for root in &roots {
            let bin = Path::new(root).join(RECENTLY_DELETED);
            let Ok(listing) = std::fs::read_dir(&bin) else { continue };
            for entry in listing.flatten() {
                let path = entry.path();
                let Some(id) = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()) else { continue };
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) && !bin.join(format!("{id}.json")).exists() {
                    // A deletion that couldn't take anything, an hour or more ago (one under way has no list yet either).
                    let stale = entry.metadata().and_then(|meta| meta.modified()).is_ok_and(|at| at.elapsed().is_ok_and(|age| age.as_secs() > 3600));
                    if stale && valid_batch_id(&id) {
                        remove_empty(&path);
                    }
                    continue;
                }
                if path.extension().and_then(|extension| extension.to_str()) != Some("json") || !valid_batch_id(&id) {
                    continue;
                }
                let Ok(manifest) = std::fs::read(&path).map_err(anyhow::Error::from).and_then(|bytes| Ok(serde_json::from_slice::<Manifest>(&bytes)?)) else {
                    continue;
                };
                if now() - manifest.deleted_at > KEEP_DELETED_FOR {
                    let _ = std::fs::remove_dir_all(bin.join(&id));
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                batches.push(DeletedBatch { id, root: root.clone(), deleted_at: manifest.deleted_at, entries: manifest.entries });
            }
        }
        batches.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at).then(b.id.cmp(&a.id)));
        Ok(batches)
    }

    /// Puts what one deletion took back where it was (beside anything put
    /// there since, renamed). It's listed once the source is read again.
    pub async fn restore_deleted(&self, source_id: &str, batch_id: &str) -> Result<()> {
        let batch = self.deleted_batch(source_id, batch_id)?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut disk = Disk::default();
            let root = Path::new(&batch.root);
            let bin = root.join(RECENTLY_DELETED);
            for entry in &batch.entries {
                let relative = checked_relative(&entry.path)?;
                let from = bin.join(&batch.id).join(&relative);
                if !from.exists() {
                    continue;
                }
                let to = disk.unused(&root.join(&relative));
                disk.check_new(&batch.root, &to)?;
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| disk_error("made", parent, error))?;
                }
                renamed(&from, &to).map_err(|error| disk_error("put back", &to, error))?;
            }
            let _ = std::fs::remove_dir_all(bin.join(&batch.id));
            std::fs::remove_file(bin.join(format!("{}.json", batch.id)))?;
            Ok(())
        })
        .await?
    }

    /// Deletes for good what these deletions took (all of Recently Deleted
    /// with none named).
    pub async fn empty_recently_deleted(&self, source_id: &str, batch_ids: Option<&[String]>) -> Result<()> {
        let batches: Vec<DeletedBatch> =
            self.recently_deleted(source_id)?.into_iter().filter(|batch| batch_ids.is_none_or(|ids| ids.contains(&batch.id))).collect();
        tokio::task::spawn_blocking(move || -> Result<()> {
            for batch in batches {
                let bin = Path::new(&batch.root).join(RECENTLY_DELETED);
                let held = bin.join(&batch.id);
                if held.exists() {
                    std::fs::remove_dir_all(&held).map_err(|error| disk_error("deleted for good", &held, error))?;
                }
                let _ = std::fs::remove_file(bin.join(format!("{}.json", batch.id)));
            }
            Ok(())
        })
        .await??;
        Ok(())
    }

    /// Deletes for good whatever's been in a media folder's Recently Deleted
    /// for a month, in every media folder that can be reached.
    pub fn tidy_recently_deleted(&self) {
        for id in self.active_source_ids().into_iter().filter(|id| self.is_folder_source(id)) {
            if let Err(error) = self.recently_deleted(&id) {
                tracing::warn!("a media folder's Recently Deleted couldn't be tidied: {error}");
            }
        }
    }

    fn deleted_batch(&self, source_id: &str, batch_id: &str) -> Result<DeletedBatch> {
        self.recently_deleted(source_id)?.into_iter().find(|batch| batch.id == batch_id).ok_or_else(|| anyhow!("That's no longer in Recently Deleted."))
    }

    // MARK: Moving and renaming

    /// Moves videos (with what's beside them) and folders into `to`, a
    /// folder of the same source. Each keeps its name, unless that's taken
    /// there ("Clip (2).mp4"). Their progress and the rest go with them.
    pub async fn move_folder_items(&self, source_id: &str, videos: &[String], folders: &[String], to: &str) -> Result<()> {
        let roots = self.folder_roots(source_id)?;
        let target = inside(&roots, Path::new(to), true)?;
        let folders = self.folders_with_videos(source_id, &roots, folders)?;
        let videos = self.video_places(&roots, videos)?;
        self.apply_moves(source_id, move |disk| {
            disk.check(&target, true)?;
            if !target.path.is_dir() {
                bail!("That folder isn't there any more.");
            }
            let mut steps = Vec::new();
            for (folder, inside_it) in &folders {
                if target.path.starts_with(&folder.path) {
                    bail!("A folder can't go inside itself.");
                }
                disk.check(folder, false)?;
                if folder.path.parent() == Some(target.path.as_path()) {
                    continue;
                }
                let new = disk.unused(&target.path.join(file_name(&folder.path)));
                let moved = inside_it.iter().map(|(id, below)| (id.clone(), target.below(&new.join(below)))).collect();
                steps.push(Step { from: folder.path.clone(), to: new, moved });
            }
            for (video, id) in videos {
                if folders.iter().any(|(folder, _)| video.path.starts_with(&folder.path)) || video.path.parent() == Some(target.path.as_path()) {
                    continue;
                }
                disk.check(&video, false)?;
                let new = disk.unused(&target.path.join(file_name(&video.path)));
                steps.extend(disk.beside_renamed(&video.path, &new));
                steps.push(Step { moved: vec![(id, target.below(&new))], from: video.path, to: new });
            }
            Ok(steps)
        })
        .await
    }

    /// Gives a video a new name (its extension stays); what's beside it named after it follows.
    pub async fn rename_folder_video(&self, source_id: &str, item_id: &str, name: &str) -> Result<()> {
        let name = valid_name(name)?;
        let roots = self.folder_roots(source_id)?;
        let Some((video, id)) = self.video_places(&roots, &[item_id.to_string()])?.into_iter().next() else { bail!("That video isn't there any more.") };
        self.apply_moves(source_id, move |disk| {
            disk.check(&video, false)?;
            let extension = video.path.extension().map(|extension| format!(".{}", extension.to_string_lossy())).unwrap_or_default();
            let new = video.path.with_file_name(format!("{name}{extension}"));
            if new == video.path {
                return Ok(Vec::new());
            }
            if !same_but_case(&video.path, &new) && disk.taken(&new) {
                bail!("There's already something called “{name}{extension}” there.");
            }
            disk.plan(&new);
            let mut steps = disk.beside_renamed(&video.path, &new);
            steps.push(Step { moved: vec![(id, video.below(&new))], from: video.path, to: new });
            Ok(steps)
        })
        .await
    }

    /// Makes a folder in `parent` (a folder of the source, or the source's own). Returns its path.
    pub async fn create_media_folder(&self, source_id: &str, parent: &str, name: &str) -> Result<String> {
        let name = valid_name(name)?;
        let roots = self.folder_roots(source_id)?;
        let parent = inside(&roots, Path::new(parent), true)?;
        tokio::task::spawn_blocking(move || {
            let mut disk = Disk::default();
            disk.check(&parent, true)?;
            let path = parent.path.join(&name);
            if disk.taken(&path) {
                bail!("There's already something called “{name}” there.");
            }
            std::fs::create_dir(&path).map_err(|error| disk_error("made", &path, error))?;
            Ok(path.to_string_lossy().into_owned())
        })
        .await?
    }

    /// Gives a folder (not the source's own) a new name.
    pub async fn rename_media_folder(&self, source_id: &str, folder_path: &str, name: &str) -> Result<()> {
        let name = valid_name(name)?;
        let roots = self.folder_roots(source_id)?;
        let Some((folder, inside_it)) = self.folders_with_videos(source_id, &roots, &[folder_path.to_string()])?.pop() else { return Ok(()) };
        self.apply_moves(source_id, move |disk| {
            disk.check(&folder, false)?;
            let new = folder.path.with_file_name(&name);
            if new == folder.path {
                return Ok(Vec::new());
            }
            if !same_but_case(&folder.path, &new) && disk.taken(&new) {
                bail!("There's already something called “{name}” there.");
            }
            let moved = inside_it.into_iter().map(|(id, below)| (id, folder.below(&new.join(below)))).collect();
            Ok(vec![Step { from: folder.path, to: new, moved }])
        })
        .await
    }

    /// Plans renames (off the async threads, as a share answers slowly) and
    /// makes them, one at a time, then carries each video that moved (up to
    /// a failure) to its new id and file: its progress, watchlist place,
    /// frame and facts, and its place in the catalog.
    async fn apply_moves(&self, source_id: &str, plan: impl FnOnce(&mut Disk) -> Result<Vec<Step>> + Send + 'static) -> Result<()> {
        let (moved, outcome) = tokio::task::spawn_blocking(move || {
            let steps = match plan(&mut Disk::default()) {
                Ok(steps) => steps,
                Err(error) => return (Vec::new(), Err(error)),
            };
            let mut moved = Vec::new();
            for step in steps {
                if let Err(error) = renamed(&step.from, &step.to) {
                    // Either end may be the folder that's read-only.
                    let error = match (step.from.parent(), step.to.parent()) {
                        (Some(from), Some(to)) if from != to && error.kind() == std::io::ErrorKind::PermissionDenied => anyhow!(
                            "“{}” or “{}” is read-only for you, so “{}” couldn't be moved. Give your login write access to both where they're kept (for a network share, in the server's settings for them), then try again.",
                            file_name(from),
                            file_name(to),
                            file_name(&step.from)
                        ),
                        _ => disk_error("moved or renamed", &step.from, error),
                    };
                    return (moved, Err(error));
                }
                moved.extend(step.moved);
            }
            (moved, Ok(()))
        })
        .await?;
        let moves: Vec<VideoMove> = moved
            .into_iter()
            .map(|(id, file)| VideoMove { to: folder::film_id(source_id, &file.root, &file.listed), from: id, path: file.path })
            .collect();
        let rekeyed: Vec<(String, String)> = moves.iter().map(|video| (video.from.clone(), video.to.clone())).collect();
        self.library.rekey_items(&rekeyed)?;
        self.catalog.move_videos(&moves)?;
        outcome
    }

    // MARK: Helpers

    /// These videos' files, each inside one of the source's folders.
    fn video_places(&self, roots: &[String], ids: &[String]) -> Result<Vec<(Inside, String)>> {
        let mut places = Vec::new();
        for movie in self.catalog.movies_by_ids(ids)? {
            let StreamLocator::File { path } = &movie.stream else { continue };
            places.push((inside(roots, Path::new(path), false)?, movie.id.clone()));
        }
        Ok(places)
    }

    /// These folders (any inside another of them left out, as they go with
    /// it), each with the videos in it and below it.
    fn folders_with_videos(&self, source_id: &str, roots: &[String], folders: &[String]) -> Result<Vec<FolderWithVideos>> {
        let folders = outermost(folders.iter().map(|folder| inside(roots, Path::new(folder), false)).collect::<Result<_>>()?);
        folders
            .into_iter()
            .map(|folder| {
                let videos = self.videos_in(source_id, &folder)?;
                Ok((folder, videos))
            })
            .collect()
    }

    /// The videos in a folder and below it, with their paths below it.
    fn videos_in(&self, source_id: &str, folder: &Inside) -> Result<Vec<(String, PathBuf)>> {
        let prefix = stored_prefix(folder);
        let videos = self.catalog.folder_videos(&FolderFilter {
            source_id,
            prefix: &prefix,
            below: true,
            kind: None,
            length: None,
            sort: xorvio_model::FolderSort::Added,
            limit: i64::MAX,
            offset: 0,
        })?
        .videos;
        Ok(videos
            .into_iter()
            .filter_map(|(movie, _)| {
                let StreamLocator::File { path } = &movie.stream else { return None };
                let below = Path::new(path).strip_prefix(&folder.path).ok()?.to_path_buf();
                Some((movie.id, below))
            })
            .collect())
    }

    /// Reads a media folder again after a change to it. A change made while
    /// it's being read is read by one more reading after that one; changes
    /// made while that one waits are read by it too.
    pub async fn read_folder_again(&self, source_id: &str, progress: &(dyn Fn(RefreshProgress) + Send + Sync)) -> Result<()> {
        if !lock(&self.folder_rereads).insert(source_id.to_string()) {
            return Ok(());
        }
        while lock(&self.refreshing).contains(source_id) {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        lock(&self.folder_rereads).remove(source_id);
        self.refresh_source(source_id, progress).await.map(|_| ())
    }
}

/// The folder a listing asks for: the one named, or the source's own when
/// it has one; none for a source of several with none named.
fn asked_place(roots: &[String], query: &FolderQuery) -> Result<Option<Inside>> {
    Ok(match (&query.folder, roots) {
        (Some(folder), _) => Some(inside(roots, Path::new(folder), true)?),
        (None, [only]) => Some(inside(roots, Path::new(only), true)?),
        (None, _) => None,
    })
}

/// `path` when it's one of `roots` (with `root_allowed`) or inside one, with
/// no "." or ".." parts and nothing hidden. [`Disk::check`] looks at where
/// links lead.
fn inside(roots: &[String], path: &Path, root_allowed: bool) -> Result<Inside> {
    if !path.is_absolute() || path.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir)) {
        return Err(refused());
    }
    for root in roots {
        let Ok(relative) = path.strip_prefix(root) else { continue };
        if relative.as_os_str().is_empty() && !root_allowed {
            return Err(refused());
        }
        if relative.components().any(|part| !matches!(part, Component::Normal(name) if !name.to_string_lossy().starts_with('.'))) {
            return Err(refused());
        }
        return Ok(Inside { root: root.clone(), relative: relative.to_path_buf(), path: path.to_path_buf() });
    }
    Err(refused())
}

/// Of these folders, the ones not inside another of them, once each.
fn outermost(mut folders: Vec<Inside>) -> Vec<Inside> {
    folders.sort_by_key(|folder| folder.path.components().count());
    let mut kept: Vec<Inside> = Vec::new();
    for folder in folders {
        if !kept.iter().any(|outer| folder.path.starts_with(&outer.path)) {
            kept.push(folder);
        }
    }
    kept
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A file's path below one of `roots`, with "/".
fn relative_to(roots: &[String], path: &Path) -> Option<String> {
    roots.iter().find_map(|root| path.strip_prefix(root).ok()).map(listed)
}

/// The way down to a folder: the source's folder (named for the source when
/// it has only one) and each folder below it.
fn trail(place: &Inside, source_name: &str, several: bool) -> Vec<FolderEntry> {
    let root = Path::new(&place.root);
    let mut trail = vec![FolderEntry {
        name: if several { naming::folder_name(&place.root).to_string() } else { source_name.to_string() },
        path: place.root.clone(),
        videos: 0,
    }];
    let mut path = root.to_path_buf();
    for part in place.relative.components() {
        path.push(part);
        trail.push(FolderEntry { name: part.as_os_str().to_string_lossy().into_owned(), path: path.to_string_lossy().into_owned(), videos: 0 });
    }
    trail
}

/// A place's path as the films' files in it begin: the source's folder, then
/// "/" between the folders below it, as the scan writes them.
fn stored_prefix(place: &Inside) -> String {
    let root = Path::new(&place.root).join("").to_string_lossy().into_owned();
    match place.listed() {
        below if below.is_empty() => root,
        below => format!("{root}{below}/"),
    }
}

/// What a change couldn't do, in words. A folder that's read-only says so:
/// a network share often lets a login read a folder it can't change, and
/// then nothing in it can be deleted, moved or renamed.
fn disk_error(doing: &str, path: &Path, error: std::io::Error) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        let folder = path.parent().map(file_name).unwrap_or_default();
        return anyhow!(
            "“{folder}” is read-only for you: you can see what's in it, but nothing in it can be added, deleted, moved or renamed. Give your login write access to “{folder}” where it's kept (for a network share, in the server's settings for it), then try again."
        );
    }
    anyhow!("“{}” couldn't be {doing}: {error}", file_name(path))
}

/// Removes `dir` and the folders in it when they hold no files; never a file.
fn remove_empty(dir: &Path) {
    if let Ok(listing) = std::fs::read_dir(dir) {
        for entry in listing.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                remove_empty(&entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}

fn refused() -> anyhow::Error {
    anyhow!("That isn't in this source's folders.")
}

/// What a change looks at on disk, each folder read once (a network share
/// answers slowly): where links lead, what's in each folder, and which
/// names there are taken, or will be by this change.
#[derive(Default)]
struct Disk {
    real: HashMap<PathBuf, Option<PathBuf>>,
    /// Each folder's files and folders: their names, and whether each is a file.
    listings: HashMap<PathBuf, Vec<(String, bool)>>,
    /// Names this change will give, folded, by folder.
    planned: HashMap<PathBuf, HashSet<String>>,
}

impl Disk {
    fn real(&mut self, path: &Path) -> Option<PathBuf> {
        self.real.entry(path.to_path_buf()).or_insert_with(|| std::fs::canonicalize(path).ok()).clone()
    }

    /// Refuses a place reached through a link that leads out of the
    /// source's folder: the folder it's in (or, `within`, it itself) must
    /// really be inside. Renaming a link itself never touches what it leads to.
    fn check(&mut self, place: &Inside, within: bool) -> Result<()> {
        let dir = if within { place.path.clone() } else { place.path.parent().map(Path::to_path_buf).unwrap_or_default() };
        match (self.real(&dir), self.real(Path::new(&place.root))) {
            (Some(real), Some(root)) if !real.starts_with(&root) => Err(refused()),
            _ => Ok(()),
        }
    }

    /// As [`Disk::check`], for a place that may not be there yet: the
    /// nearest folder above it that is.
    fn check_new(&mut self, root: &str, path: &Path) -> Result<()> {
        let Some(there) = path.ancestors().skip(1).find(|ancestor| ancestor.exists()) else { return Err(refused()) };
        match (self.real(there), self.real(Path::new(root))) {
            (Some(real), Some(root)) if real.starts_with(&root) => Ok(()),
            _ => Err(refused()),
        }
    }

    fn listing(&mut self, dir: &Path) -> &[(String, bool)] {
        self.listings.entry(dir.to_path_buf()).or_insert_with(|| {
            std::fs::read_dir(dir)
                .map(|listing| listing.flatten().map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.file_type().is_ok_and(|kind| kind.is_file()))).collect())
                .unwrap_or_default()
        })
    }

    /// Whether something in `path`'s folder has its name already (in any
    /// case, as most disks don't tell them apart) or will by this change.
    fn taken(&mut self, path: &Path) -> bool {
        let Some(dir) = path.parent() else { return true };
        let name = folded(&file_name(path));
        self.planned.get(dir).is_some_and(|names| names.contains(&name)) || self.listing(dir).iter().any(|(other, _)| folded(other) == name)
    }

    /// Notes a name this change will give.
    fn plan(&mut self, path: &Path) {
        if let Some(dir) = path.parent() {
            self.planned.entry(dir.to_path_buf()).or_default().insert(folded(&file_name(path)));
        }
    }

    /// `path`, or "Name (2).ext" and so on when that's taken.
    fn unused(&mut self, path: &Path) -> PathBuf {
        let chosen = if !self.taken(path) {
            path.to_path_buf()
        } else {
            let stem = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
            let extension = path.extension().map(|extension| format!(".{}", extension.to_string_lossy())).unwrap_or_default();
            (2..10_000).map(|n| path.with_file_name(format!("{stem} ({n}){extension}"))).find(|candidate| !self.taken(candidate)).unwrap_or_else(|| path.to_path_buf())
        };
        self.plan(&chosen);
        chosen
    }

    /// A video's pictures, .nfo and subtitles beside it, named after it
    /// ("Clip.srt", "Clip-poster.jpg") and not after another video there
    /// with a longer name ("Clip-2.mp4" keeps "Clip-2.jpg"), and any file
    /// named for the whole of its file name ("Clip.mp4.json").
    fn beside(&mut self, video: &Path) -> Vec<PathBuf> {
        let (Some(dir), Some(base)) = (video.parent(), video.file_stem().map(|stem| stem.to_string_lossy().into_owned())) else { return Vec::new() };
        let own = file_name(video);
        let files: Vec<String> = self.listing(dir).iter().filter(|(name, is_file)| *is_file && *name != own).map(|(name, _)| name.clone()).collect();
        let named_after = |name: &str, base: &str| name.strip_prefix(base).is_some_and(|rest| rest.starts_with('.') || rest.starts_with('-'));
        // "Clip.mp4.json", "Clip.mp4.xmp": what a downloader or editor keeps about it.
        let about_it = |name: &str| name.strip_prefix(own.as_str()).is_some_and(|rest| rest.starts_with('.')) && !naming::has_extension(name, naming::VIDEO_EXTENSIONS);
        let others: Vec<&str> = files
            .iter()
            .filter(|name| naming::has_extension(name, naming::VIDEO_EXTENSIONS))
            .filter_map(|name| name.rsplit_once('.').map(|(stem, _)| stem))
            .filter(|other| other.len() > base.len())
            .collect();
        files
            .iter()
            .filter(|name| {
                about_it(name)
                    || (named_after(name, &base)
                        && !others.iter().any(|other| named_after(name, other))
                        && [naming::IMAGE_EXTENSIONS, naming::SUBTITLE_EXTENSIONS, &["nfo"]].iter().any(|extensions| naming::has_extension(name, extensions)))
            })
            .map(|name| dir.join(name))
            .collect()
    }

    /// What's beside a video, renamed to follow it to `new` ("Clip.srt" →
    /// "Memes/Clip (2).srt").
    fn beside_renamed(&mut self, video: &Path, new: &Path) -> Vec<Step> {
        let stem = |path: &Path| path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let (old_base, new_base) = (stem(video), stem(new));
        let dir = new.parent().map(Path::to_path_buf).unwrap_or_default();
        self.beside(video)
            .into_iter()
            .map(|file| {
                let rest = file_name(&file).strip_prefix(&old_base).map(str::to_string).unwrap_or_default();
                let wanted = dir.join(format!("{new_base}{rest}"));
                let to = if same_but_case(&file, &wanted) { wanted } else { self.unused(&wanted) };
                Step { from: file, to, moved: Vec::new() }
            })
            .collect()
    }
}

/// A name as most disks compare them: in any case, and however its accents are written.
fn folded(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}

/// Two paths in the same folder whose names differ only in case.
fn same_but_case(a: &Path, b: &Path) -> bool {
    a.parent() == b.parent() && folded(&file_name(a)) == folded(&file_name(b))
}

/// Renames, never over something already at `to` (which renaming would
/// replace); a new name differing only in case is the same file on most disks.
fn renamed(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Ok(there) = to.symlink_metadata() {
        #[cfg(unix)]
        let same = {
            use std::os::unix::fs::MetadataExt;
            from.symlink_metadata().is_ok_and(|here| (here.dev(), here.ino()) == (there.dev(), there.ino()))
        };
        #[cfg(not(unix))]
        let same = {
            let _ = there;
            same_but_case(from, to)
        };
        if !same {
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "something's already there with that name"));
        }
    }
    std::fs::rename(from, to)
}

/// A name for a file or folder: not empty, not hidden, no path in it.
fn valid_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("Give it a name.");
    }
    if name.starts_with('.') || name.chars().any(|c| matches!(c, '/' | '\\' | ':' | '\0')) {
        bail!("A name can't start with a full stop or contain / \\ or :.");
    }
    if name.chars().count() > LONGEST_NAME {
        bail!("That name is too long.");
    }
    Ok(name.to_string())
}

/// A path kept in a manifest, as a relative path with nothing that climbs out.
fn checked_relative(path: &str) -> Result<PathBuf> {
    let relative: PathBuf = path.split('/').collect();
    if path.split('/').any(|part| matches!(part, "" | "." | "..")) || relative.components().any(|part| !matches!(part, Component::Normal(_))) {
        bail!("Recently Deleted has something it can't put back.");
    }
    Ok(relative)
}

/// When something was deleted, and something to tell two deletions in the same second apart.
fn batch_id() -> String {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    format!("{stamp}-{}", &uuid::Uuid::new_v4().simple().to_string()[..6])
}

fn valid_batch_id(id: &str) -> bool {
    id.len() <= 40 && !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[cfg(test)]
mod tests;
