//! Files and folders people chose in Xorvio's own dialogs.
//!
//! The interface never names a path itself: Xorvio's dialogs say what was
//! chosen, and the commands that keep a path (a media folder, a playlist
//! file, a recordings, backup or sync folder) or act on one (restoring a
//! backup) take only those, or paths already in the sources and settings.
//! So a script that somehow ran in the web view couldn't point Xorvio at a
//! folder to delete from or a file to read.

use std::collections::HashSet;
use std::sync::Mutex;

#[derive(Default)]
pub struct Picked(Mutex<HashSet<String>>);

impl Picked {
    /// Remembers what a dialog returned.
    pub fn add(&self, path: &str) {
        self.0.lock().unwrap().insert(path.to_string());
    }

    /// Fine when every one of `paths` was chosen in a dialog or is in `known`.
    pub fn check<'a>(&self, paths: impl IntoIterator<Item = &'a str>, known: &[&str]) -> Result<(), String> {
        let picked = self.0.lock().unwrap();
        match paths.into_iter().find(|path| !picked.contains(*path) && !known.contains(path)) {
            Some(_) => Err("Choose the file or folder with Choose… first.".into()),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Picked;

    #[test]
    fn takes_only_chosen_or_known_paths() {
        let picked = Picked::default();
        picked.add("/Users/me/Movies");
        assert!(picked.check(["/Users/me/Movies"], &[]).is_ok());
        assert!(picked.check(["/Users/me/Movies", "/Volumes/Films"], &["/Volumes/Films"]).is_ok());
        assert!(picked.check(["/Users/me"], &[]).is_err());
        assert!(picked.check(["/Users/me/Movies", "/"], &[]).is_err());
        assert!(picked.check([], &[]).is_ok());
    }
}
