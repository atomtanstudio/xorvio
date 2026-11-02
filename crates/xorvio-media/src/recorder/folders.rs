//! Where recordings are saved: the folder chosen for their source, else the
//! one chosen for recordings, else Xorvio's own. A chosen folder that isn't
//! there when a recording starts (on a drive that's unplugged, say) is
//! passed over for the next, and the recording says so.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

/// The folders recordings can go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingFolders {
    /// Settings › Recording's folder, for sources without one of their own.
    pub main: PathBuf,
    /// Xorvio's own folder (Movies › Xorvio): used when a chosen one can't
    /// be, and made when it's needed.
    pub fallback: PathBuf,
    /// Folders chosen for particular sources, by source id.
    pub by_source: HashMap<String, PathBuf>,
}

impl RecordingFolders {
    /// Everything in one folder.
    pub fn single(dir: PathBuf) -> Self {
        RecordingFolders {
            main: dir.clone(),
            fallback: dir,
            by_source: HashMap::new(),
        }
    }

    /// The folder for a new recording from `source_id`, with a note when it
    /// isn't the one chosen for it.
    pub(crate) fn choose(&self, source_id: &str) -> (PathBuf, Option<String>) {
        let wanted = self.by_source.get(source_id).unwrap_or(&self.main);
        if self.ready(wanted) {
            return (wanted.clone(), None);
        }
        for instead in [&self.main, &self.fallback] {
            if instead != wanted && self.ready(instead) {
                let note = format!(
                    "“{}” wasn't available, so this was saved in “{}”.",
                    wanted.display(),
                    instead.display()
                );
                return (instead.clone(), Some(note));
            }
        }
        // Nowhere can be written to; the recording fails and says why.
        (wanted.clone(), None)
    }

    /// The folder is there and can be written to. Only Xorvio's own folder
    /// is made when it's missing: a chosen one that's gone is usually on a
    /// drive that isn't plugged in, and making it would fill the wrong disk.
    pub(crate) fn ready(&self, dir: &Path) -> bool {
        if dir == self.fallback {
            let _ = std::fs::create_dir_all(dir);
        }
        writable(dir)
    }
}

/// A folder a file can be made in (a read-only share, or one without
/// permission, isn't).
pub(crate) fn writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(".xorvio-{}.tmp", uuid::Uuid::new_v4().simple()));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_folder_then_the_recordings_folder_then_xorvios_own() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("Recordings");
        let sports = dir.path().join("Sports drive");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&sports).unwrap();
        let folders = RecordingFolders {
            main: main.clone(),
            fallback: dir.path().join("Movies").join("Xorvio"),
            by_source: HashMap::from([("sports".to_string(), sports.clone())]),
        };
        assert_eq!(folders.choose("sports"), (sports.clone(), None));
        assert_eq!(
            folders.choose("news"),
            (main.clone(), None),
            "no folder of its own"
        );
        assert!(
            std::fs::read_dir(&sports).unwrap().next().is_none(),
            "checking leaves nothing behind"
        );

        // The drive is unplugged: the recordings folder instead, saying so.
        std::fs::remove_dir(&sports).unwrap();
        let (used, note) = folders.choose("sports");
        assert_eq!(used, main);
        let note = note.unwrap();
        assert!(
            note.contains("Sports drive") && note.contains("wasn't available"),
            "{note}"
        );
        assert!(
            !sports.exists(),
            "a chosen folder that's gone isn't made again"
        );

        // Neither is there: Xorvio's own folder, made for it.
        std::fs::remove_dir(&main).unwrap();
        let (used, note) = folders.choose("news");
        assert_eq!(used, folders.fallback);
        assert!(used.is_dir());
        assert!(note.unwrap().contains("Recordings"));
        assert!(!main.exists());
    }

    #[test]
    fn xorvios_own_folder_is_made_without_a_note() {
        let dir = tempfile::tempdir().unwrap();
        let folders = RecordingFolders::single(dir.path().join("Movies").join("Xorvio"));
        let (used, note) = folders.choose("any");
        assert_eq!((used.is_dir(), note), (true, None));
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_cant_be_written_is_passed_over() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("Read only");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let folders = RecordingFolders {
            main: locked.clone(),
            fallback: dir.path().join("Xorvio"),
            by_source: HashMap::new(),
        };
        // Root writes anywhere, so there'd be nothing to test.
        let as_root = writable(&locked);
        let (used, note) = folders.choose("any");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if as_root {
            return;
        }
        assert_eq!(used, folders.fallback);
        assert!(note.is_some());
    }
}
