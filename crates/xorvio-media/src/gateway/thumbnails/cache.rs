//! The thumbnails folder: a folder per title (named by its key) holding its
//! sheets and `sheet.json`, whose modification time is when the title's
//! thumbnails were last asked for. Once the folder holds more than its limit,
//! the titles asked for least recently go first. A title being made lives in
//! `.making-<key>` until it's whole.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::Layout;

pub(crate) const LAYOUT: &str = "sheet.json";
const MAKING: &str = ".making-";

pub(crate) struct Cache {
    dir: PathBuf,
    limit: u64,
}

impl Cache {
    /// Opens the folder, clearing what a previous run left half made, and
    /// brings it within its limit.
    pub fn open(dir: PathBuf, limit: u64) -> std::io::Result<Cache> {
        std::fs::create_dir_all(&dir)?;
        for entry in std::fs::read_dir(&dir)?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let whole = entry.path().join(LAYOUT).is_file();
            if name.starts_with(MAKING) || (!name.starts_with('.') && !whole) {
                let _ = remove(&entry.path());
            }
        }
        let cache = Cache { dir, limit };
        cache.tidy(None);
        Ok(cache)
    }

    /// A title's layout, when its thumbnails are made.
    pub fn layout(&self, key: &str) -> Option<Layout> {
        let bytes = std::fs::read(self.dir.join(key).join(LAYOUT)).ok()?;
        let layout: Layout = serde_json::from_slice(&bytes).ok()?;
        (0..layout.sheets)
            .all(|n| self.path(key, &format!("{n}.jpg")).is_file())
            .then_some(layout)
    }

    /// Marks a title as just used, so it's kept over others.
    pub fn touch(&self, key: &str) {
        let marked = std::fs::File::options()
            .write(true)
            .open(self.dir.join(key).join(LAYOUT))
            .and_then(|file| file.set_modified(SystemTime::now()));
        if let Err(error) = marked {
            tracing::debug!("a title's thumbnails couldn't be marked as used: {error}");
        }
    }

    pub fn path(&self, key: &str, name: &str) -> PathBuf {
        self.dir.join(key).join(name)
    }

    /// Where a title's thumbnails are made.
    pub fn work_dir(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{MAKING}{key}"))
    }

    /// Puts a title's finished folder in its place.
    pub fn finish(&self, key: &str, work: &Path) -> std::io::Result<()> {
        let done = self.dir.join(key);
        if done.exists() {
            remove(&done)?;
        }
        std::fs::rename(work, &done)
    }

    /// Removes the titles used least recently until the folder is within
    /// its limit, never `keep` (the one just made).
    pub fn tidy(&self, keep: Option<&str>) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return };
        let mut titles: Vec<(SystemTime, u64, PathBuf, String)> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    return None;
                }
                let path = entry.path();
                let used = path.join(LAYOUT).metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                Some((used, size_of(&path), path, name))
            })
            .collect();
        let mut total: u64 = titles.iter().map(|(_, size, _, _)| size).sum();
        if total <= self.limit {
            return;
        }
        titles.sort_by_key(|(used, ..)| *used);
        for (_, size, path, name) in titles {
            if total <= self.limit {
                break;
            }
            if Some(name.as_str()) == keep {
                continue;
            }
            match remove(&path) {
                Ok(()) => total = total.saturating_sub(size),
                Err(error) => tracing::warn!("old thumbnails couldn't be removed: {error}"),
            }
        }
    }
}

fn remove(path: &Path) -> std::io::Result<()> {
    if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }
}

/// The bytes of the files in a title's folder.
fn size_of(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| entries.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A made title of `bytes`, last used `ago` seconds back.
    fn title(dir: &Path, key: &str, bytes: usize, ago: u64) {
        let folder = dir.join(key);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("0.jpg"), vec![0xff; bytes]).unwrap();
        let layout = Layout { interval_seconds: 10.0, count: 5, columns: 10, rows: 1, width: 160, height: 90, sheets: 1 };
        std::fs::write(folder.join(LAYOUT), serde_json::to_vec(&layout).unwrap()).unwrap();
        let file = std::fs::File::options().write(true).open(folder.join(LAYOUT)).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(ago)).unwrap();
    }

    #[test]
    fn the_least_recently_used_go_first() {
        let dir = tempfile::tempdir().unwrap();
        title(dir.path(), "aaaaaaaaaaaaaaaa", 400_000, 300);
        title(dir.path(), "bbbbbbbbbbbbbbbb", 400_000, 200);
        title(dir.path(), "cccccccccccccccc", 400_000, 100);
        // Left by a run that stopped part-way, and a folder with no layout.
        std::fs::create_dir_all(dir.path().join(".making-dddddddddddddddd")).unwrap();
        std::fs::create_dir_all(dir.path().join("eeeeeeeeeeeeeeee")).unwrap();

        let cache = Cache::open(dir.path().to_path_buf(), 900_000).unwrap();
        assert!(!dir.path().join(".making-dddddddddddddddd").exists());
        assert!(!dir.path().join("eeeeeeeeeeeeeeee").exists());
        assert!(cache.layout("aaaaaaaaaaaaaaaa").is_none(), "the oldest went to make room");
        assert!(cache.layout("bbbbbbbbbbbbbbbb").is_some() && cache.layout("cccccccccccccccc").is_some());

        // Using one keeps it over a newer one.
        cache.touch("bbbbbbbbbbbbbbbb");
        title(dir.path(), "ffffffffffffffff", 400_000, 0);
        cache.tidy(Some("ffffffffffffffff"));
        assert!(cache.layout("cccccccccccccccc").is_none());
        assert!(cache.layout("bbbbbbbbbbbbbbbb").is_some());
        assert!(cache.layout("ffffffffffffffff").is_some());

        // The one just made stays even when it alone is over the limit.
        let small = Cache::open(dir.path().to_path_buf(), 100_000).unwrap();
        title(dir.path(), "1111111111111111", 400_000, 0);
        small.tidy(Some("1111111111111111"));
        assert!(small.layout("1111111111111111").is_some());
        assert!(small.layout("bbbbbbbbbbbbbbbb").is_none() && small.layout("ffffffffffffffff").is_none());
    }

    #[test]
    fn a_title_with_a_sheet_missing_isnt_ready() {
        let dir = tempfile::tempdir().unwrap();
        title(dir.path(), "aaaaaaaaaaaaaaaa", 10, 0);
        let cache = Cache::open(dir.path().to_path_buf(), 1 << 30).unwrap();
        assert!(cache.layout("aaaaaaaaaaaaaaaa").is_some());
        std::fs::remove_file(cache.path("aaaaaaaaaaaaaaaa", "0.jpg")).unwrap();
        assert!(cache.layout("aaaaaaaaaaaaaaaa").is_none());
    }
}
