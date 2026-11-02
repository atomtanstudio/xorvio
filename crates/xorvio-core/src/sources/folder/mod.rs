//! Folders of a person's own video files as a source, as Infuse has them:
//! on this computer, an external drive, or a network share mounted in
//! Finder. Films and series are read from the files' names and folders
//! ([`naming`]), with details from .nfo files ([`nfo`]) and artwork from the
//! images beside them; nothing is looked up online or probed while scanning.
//!
//! Files reach the interface only through `xorvio-folder` addresses
//! ([`folder_file_url`]). The app serves them only for image files inside
//! the source's own folders ([`checked_file`]): never another path, never
//! one that climbs out with "..", never a link that leads outside.

pub(crate) mod naming;
pub(crate) mod nfo;
mod organize;
mod scan;
pub(crate) mod sidecars;

#[cfg(test)]
mod tests;

use std::path::{Component, Path, PathBuf};

pub(crate) use organize::film_id;
pub(crate) use scan::{forget, scan, skipped_folder};

use crate::error::SourceError;

/// The scheme of a media folder's files, served by the app (see [`folder_file_url`]).
pub const FOLDER_SCHEME: &str = "xorvio-folder";

/// The largest image passed on; posters and backdrops are far smaller.
pub(crate) const MOST_IMAGE_BYTES: u64 = 30 << 20;
/// Subtitle files are tens of kilobytes; anything this big isn't one.
pub(crate) const MOST_SUBTITLE_BYTES: u64 = 8 << 20;

/// The address the interface loads a file in a media folder from: the
/// source's id and the file's path. `version` (its modification time) makes
/// a changed image a new address, so the web view's cache never shows the
/// old one.
pub fn folder_file_url(source_id: &str, path: &str, version: Option<i64>) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(path.as_bytes()).collect();
    let version = version.map(|version| format!("?v={version}")).unwrap_or_default();
    if cfg!(windows) {
        format!("http://{FOLDER_SCHEME}.localhost/{source_id}/{encoded}{version}")
    } else {
        format!("{FOLDER_SCHEME}://localhost/{source_id}/{encoded}{version}")
    }
}

/// The source id and file path in a folder address's path
/// ("/<source>/<encoded path>"), or `None` when it isn't one. Whether the
/// file may be served is for [`checked_file`] to say.
pub fn parse_folder_path(request_path: &str) -> Option<(String, PathBuf)> {
    let (source_id, encoded) = request_path.trim_start_matches('/').split_once('/')?;
    let path = url::form_urlencoded::parse(format!("p={encoded}").as_bytes()).find(|(key, _)| key == "p")?.1.into_owned();
    (!source_id.is_empty() && !path.is_empty()).then(|| (source_id.to_string(), PathBuf::from(path)))
}

/// The real path of `requested` when it's a file of one of `extensions`
/// inside one of `roots`, and nothing else: it must be an absolute path
/// under a root as written, with no "." or ".." parts and no hidden ones,
/// and still be under that root once links are followed.
pub(crate) fn checked_file(roots: &[String], requested: &Path, extensions: &[&str]) -> Result<PathBuf, SourceError> {
    let refused = || SourceError::Unsupported("That file isn't in this source's folders.".into());
    let allowed = |path: &Path| path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| extensions.contains(&extension.to_ascii_lowercase().as_str()));
    let plain_below = |below: &Path| {
        !below.as_os_str().is_empty()
            && below.components().all(|part| matches!(part, Component::Normal(name) if !name.to_string_lossy().starts_with('.')))
    };
    if !requested.is_absolute() || requested.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir)) || !allowed(requested) {
        return Err(refused());
    }
    let real = std::fs::canonicalize(requested).map_err(|_| refused())?;
    if !allowed(&real) || !real.is_file() {
        return Err(refused());
    }
    for root in roots.iter().map(Path::new) {
        let (Ok(written), Ok(real_root)) = (requested.strip_prefix(root), std::fs::canonicalize(root)) else { continue };
        if plain_below(written) && real.strip_prefix(&real_root).is_ok_and(plain_below) {
            return Ok(real);
        }
    }
    Err(refused())
}

/// A file's bytes, or an error when it's bigger than `most`.
pub(crate) fn read_small(path: &Path, most: u64) -> anyhow::Result<Vec<u8>> {
    let size = std::fs::metadata(path)?.len();
    anyhow::ensure!(size <= most, "That file is too large.");
    Ok(std::fs::read(path)?)
}
