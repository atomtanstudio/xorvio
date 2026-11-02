//! Media folders as sources: choosing their folders, scanning them, and the
//! artwork and subtitles in them, which are only ever read from inside the
//! folders a source was given.

use std::path::{Path, PathBuf};

use xorvio_model::{AddonSubtitle, LiveStreamFormat, RefreshInterval, SourceConfig, SourceKind, StreamLocator};

use crate::details::DetailsRecord;
use crate::error::SourceError;
use crate::snapshot::CatalogSnapshot;
use crate::sources::folder::{self, MOST_IMAGE_BYTES, MOST_SUBTITLE_BYTES, naming, sidecars};
use crate::sources::phase;
use crate::{Core, Result, now};

/// Where each media folder source's last scan is kept, beside the catalog.
const SCANS_FOLDER: &str = "folder-scans";

impl Core {
    /// Adds a media folder source with one or more folders, named after the
    /// first unless `name` is given. Nothing is read until it's refreshed.
    pub fn add_folder_source(&self, name: &str, folders: &[String]) -> Result<SourceConfig> {
        let folders = self.checked_folders(folders, None)?;
        let name = match name.trim() {
            "" => naming::folder_name(&folders[0]).to_string(),
            name => name.to_string(),
        };
        let config = SourceConfig {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            kind: SourceKind::Folder,
            display_host: naming::describe(&folders),
            file_path: None,
            folders,
            is_paused: false,
            live_format: LiveStreamFormat::default(),
            refresh_interval: RefreshInterval::default(),
            user_agent: None,
            created_at: now(),
            sort_index: self.next_sort_index(),
            guide_offset_minutes: 0,
        };
        self.save_configs(|configs| configs.push(config.clone()))?;
        Ok(config)
    }

    /// Folders as given, tidied and checked: each must be a folder (a new one
    /// must be there now; one the source already has may be on a drive
    /// that's away), none may be inside another, and none may already be in
    /// another media folder source. `source` is the source being edited.
    pub(crate) fn checked_folders(&self, requested: &[String], source: Option<&SourceConfig>) -> Result<Vec<String>> {
        let kept = source.map(|source| source.folders.as_slice()).unwrap_or_default();
        let mut folders: Vec<String> = Vec::new();
        for raw in requested {
            let folder = tidy_folder(raw).ok_or_else(|| SourceError::Unsupported(format!("“{}” isn't a folder Xorvio can use.", raw.trim())))?;
            if whole_system_disk(Path::new(&folder)) {
                return Err(SourceError::Unsupported("Choose the folder your films or series are in, rather than the whole disk.".into()).into());
            }
            if folders.contains(&folder) {
                continue;
            }
            if !kept.contains(&folder) {
                match std::fs::metadata(&folder) {
                    Ok(metadata) if metadata.is_dir() => {}
                    Ok(_) => return Err(SourceError::Unsupported(format!("“{}” isn't a folder.", naming::folder_name(&folder))).into()),
                    Err(_) => {
                        return Err(SourceError::Unsupported(format!("“{}” can't be found. Choose the folder again.", naming::folder_name(&folder))).into());
                    }
                }
            }
            if let Some(outer) = folders.iter().find(|other| nested(other, &folder)) {
                let (inner, outer) = if Path::new(&folder).starts_with(outer) { (&folder, outer) } else { (outer, &folder) };
                return Err(SourceError::Unsupported(format!(
                    "“{}” is inside “{}”, so it's already part of this source.",
                    naming::folder_name(inner),
                    naming::folder_name(outer)
                ))
                .into());
            }
            for other in self.configs().iter().filter(|other| other.kind == SourceKind::Folder && Some(&other.id) != source.map(|s| &s.id)) {
                if other.folders.iter().any(|theirs| nested(theirs, &folder)) {
                    return Err(SourceError::Unsupported(format!(
                        "“{}” is already part of “{}”. Add it there, or remove it from there first.",
                        naming::folder_name(&folder),
                        other.name
                    ))
                    .into());
                }
            }
            folders.push(folder);
        }
        if folders.is_empty() {
            return Err(SourceError::MissingConfiguration("a folder".into()).into());
        }
        Ok(folders)
    }

    /// Scans a media folder source's folders into a catalog, off the async threads.
    pub(crate) async fn scan_folder_source(
        &self,
        config: &SourceConfig,
        report: &(dyn Fn(&str) + Send + Sync),
    ) -> std::result::Result<CatalogSnapshot, SourceError> {
        report(phase::READING_CATALOG);
        let (id, folders, cache) = (config.id.clone(), config.folders.clone(), self.folder_scan_path(&config.id));
        tokio::task::spawn_blocking(move || folder::scan(&id, &folders, &cache))
            .await
            .map_err(|_| SourceError::Unsupported("The folders couldn't be read.".into()))?
    }

    /// Forgets a removed source's last scan.
    pub(crate) fn forget_folder_scan(&self, id: &str) {
        folder::forget(&self.folder_scan_path(id));
    }

    fn folder_scan_path(&self, id: &str) -> PathBuf {
        self.config_file.directory().join(SCANS_FOLDER).join(format!("{id}.json"))
    }

    /// A media folder source's folders; an error for any other source.
    pub(crate) fn folder_roots(&self, source_id: &str) -> Result<Vec<String>> {
        let config = self.config(source_id)?;
        if config.kind != SourceKind::Folder {
            return Err(SourceError::Unsupported("This source isn't a media folder.".into()).into());
        }
        Ok(config.folders)
    }

    /// An image in a media folder source's folders, for the interface: its
    /// bytes and type. Anything that isn't an image file inside the
    /// source's own folders is refused.
    pub async fn folder_image(&self, source_id: &str, path: &Path) -> Result<(Vec<u8>, String)> {
        let roots = self.folder_roots(source_id)?;
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let file = folder::checked_file(&roots, &path, naming::IMAGE_EXTENSIONS)?;
            let bytes = folder::read_small(&file, MOST_IMAGE_BYTES)?;
            let kind = crate::images::image_type(None, &bytes).ok_or_else(|| SourceError::Unsupported("That file isn't an image.".into()))?;
            Ok((bytes, kind))
        })
        .await?
    }

    /// The subtitle files beside a media folder's film or episode, for the
    /// player. Empty for titles from anywhere else.
    pub fn folder_subtitles(&self, item_id: &str) -> Result<Vec<AddonSubtitle>> {
        let found = match self.catalog.movies_by_ids(&[item_id.to_string()])?.pop() {
            Some(movie) => Some((movie.source_id, movie.stream)),
            None => self.catalog.episodes_by_ids(&[item_id.to_string()])?.pop().map(|episode| (episode.source_id, episode.stream)),
        };
        let Some((source_id, StreamLocator::File { path })) = found else { return Ok(Vec::new()) };
        if self.folder_roots(&source_id).is_err() {
            return Ok(Vec::new());
        }
        Ok(sidecars::sidecars(Path::new(&path))
            .into_iter()
            .map(|sidecar| AddonSubtitle {
                addon_name: sidecar.file_name,
                language: sidecar.language.unwrap_or_else(|| "und".into()),
                url: folder::folder_file_url(&source_id, &sidecar.path.to_string_lossy(), None),
            })
            .collect())
    }

    /// A subtitle file's bytes, by the address [`Core::folder_subtitles`]
    /// gave it. Only subtitle files inside the source's folders are read.
    pub async fn folder_subtitle_file(&self, address: &str) -> Result<Vec<u8>> {
        let refused = || SourceError::Unsupported("Those subtitles aren't in a media folder.".into());
        let parsed = url::Url::parse(address).map_err(|_| refused())?;
        let (source_id, path) = folder::parse_folder_path(parsed.path()).ok_or_else(refused)?;
        let roots = self.folder_roots(&source_id)?;
        tokio::task::spawn_blocking(move || {
            let file = folder::checked_file(&roots, &path, naming::SUBTITLE_EXTENSIONS)?;
            folder::read_small(&file, MOST_SUBTITLE_BYTES)
        })
        .await?
    }

    pub(crate) fn is_folder_source(&self, source_id: &str) -> bool {
        self.config(source_id).is_ok_and(|config| config.kind == SourceKind::Folder)
    }

    /// Of these films, the media folders' with no picture of their own or
    /// found for them, with their files: a frame of the video can be theirs.
    pub fn videos_without_pictures(&self, ids: &[String]) -> Result<Vec<(String, String, PathBuf)>> {
        let mut wanted = Vec::new();
        for movie in self.catalog.movies_by_ids(ids)? {
            let StreamLocator::File { path } = &movie.stream else { continue };
            if movie.poster_url.is_some() || !self.is_folder_source(&movie.source_id) {
                continue;
            }
            if matches!(self.catalog.poster_lookup(&movie.id)?, Some((Some(_), _))) {
                continue;
            }
            wanted.push((movie.id, movie.source_id, PathBuf::from(path)));
        }
        Ok(wanted)
    }

    /// Keeps a frame of a film's video as its picture, as a poster found for
    /// it is kept, so lists show it from then on.
    pub fn keep_video_picture(&self, item_id: &str, source_id: &str, url: &str) -> Result<()> {
        self.catalog.save_poster_lookup(item_id, source_id, Some(url), now())
    }
}

/// TMDB's details under a media folder's own: what its .nfo files and
/// artwork say wins, as a media server's does. TMDB's seasons stay, with the
/// folder's season posters added to them.
pub(crate) fn tmdb_under_folder(own: DetailsRecord, tmdb: &DetailsRecord) -> DetailsRecord {
    let mut seasons = tmdb.seasons.clone();
    for season in &own.seasons {
        match seasons.iter_mut().find(|known| known.number == season.number) {
            Some(known) => known.cover_url = season.cover_url.clone().or(known.cover_url.take()),
            None => seasons.push(season.clone()),
        }
    }
    seasons.sort_by_key(|season| if season.number == 0 { i64::MAX } else { season.number });
    let mut details = own.filling_from(tmdb);
    details.seasons = seasons;
    details
}

/// A folder's path as chosen, tidied: absolute, with no trailing slash and
/// no "." or ".." parts. `None` when it isn't usable.
fn tidy_folder(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let path = Path::new(trimmed);
    if trimmed.is_empty() || !path.is_absolute() || path.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
        return None;
    }
    let tidy: PathBuf = path.components().collect();
    Some(tidy.to_string_lossy().into_owned())
}

/// One folder is the other, or inside it.
fn nested(a: &str, b: &str) -> bool {
    Path::new(a).starts_with(b) || Path::new(b).starts_with(a)
}

/// The disk the system is on, whole: "/", or Windows's "C:\". Any other
/// drive or share can be chosen whole, as "/Volumes/NAS" can on a Mac.
fn whole_system_disk(folder: &Path) -> bool {
    if folder.parent().is_some() {
        return false;
    }
    if cfg!(windows) {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        return folder.starts_with(format!("{drive}\\"));
    }
    true
}

#[cfg(test)]
mod tests {
    use xorvio_model::SeasonInfo;

    use super::*;

    #[test]
    fn a_folders_own_details_come_before_tmdbs() {
        let season = |number: i64, air_date: Option<&str>, cover: Option<&str>| SeasonInfo {
            number,
            air_date: air_date.map(str::to_string),
            cover_url: cover.map(str::to_string),
            ..Default::default()
        };
        let own = DetailsRecord {
            overview: Some("From the .nfo".into()),
            seasons: vec![season(1, None, Some("xorvio-folder://localhost/s/season01-poster.jpg"))],
            ..Default::default()
        };
        let tmdb = DetailsRecord {
            overview: Some("From TMDB".into()),
            tagline: Some("From TMDB too".into()),
            seasons: vec![season(0, Some("2021-01-01"), None), season(1, Some("2022-02-18"), Some("https://image.tmdb.org/1.jpg")), season(2, Some("2025-01-17"), None)],
            ..Default::default()
        };
        let details = tmdb_under_folder(own, &tmdb);
        assert_eq!(details.overview.as_deref(), Some("From the .nfo"));
        assert_eq!(details.tagline.as_deref(), Some("From TMDB too"), "TMDB fills what the folder leaves out");
        assert_eq!(
            details.seasons,
            [
                season(1, Some("2022-02-18"), Some("xorvio-folder://localhost/s/season01-poster.jpg")),
                season(2, Some("2025-01-17"), None),
                season(0, Some("2021-01-01"), None),
            ],
            "TMDB's seasons, with the folder's posters, specials last"
        );
    }

    #[cfg(unix)]
    #[test]
    fn folders_are_tidied() {
        assert_eq!(tidy_folder(" /Volumes/NAS/Films/ ").as_deref(), Some("/Volumes/NAS/Films"));
        assert_eq!(tidy_folder("/Users/me/./Movies").as_deref(), Some("/Users/me/Movies"));
        assert_eq!(tidy_folder("/Users/me/../other"), None);
        assert_eq!(tidy_folder("Movies"), None);
        assert!(nested("/Volumes/NAS/Films", "/Volumes/NAS/Films/Kids"));
        assert!(!nested("/Volumes/NAS/Films", "/Volumes/NAS/Films Extra"));
        assert!(whole_system_disk(Path::new("/")));
        assert!(!whole_system_disk(Path::new("/Volumes/NAS")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_folders_are_tidied() {
        assert_eq!(tidy_folder(r" D:\Films\ ").as_deref(), Some(r"D:\Films"));
        assert_eq!(tidy_folder("D:/Films/./Kids").as_deref(), Some(r"D:\Films\Kids"));
        assert_eq!(tidy_folder(r"\\NAS\Media\Films").as_deref(), Some(r"\\NAS\Media\Films"));
        assert_eq!(tidy_folder("/Films"), None, "no drive");
        assert!(nested(r"D:\Films", r"D:\Films\Kids"));
        assert!(!nested(r"D:\Films", r"D:\Films Extra"));
        assert!(whole_system_disk(Path::new(r"C:\")));
        assert!(!whole_system_disk(Path::new(r"D:\")), "a drive of films");
        assert!(!whole_system_disk(Path::new(r"\\NAS\Media")), "a share");
    }
}
