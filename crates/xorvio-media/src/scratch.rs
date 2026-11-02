//! Where Xorvio keeps temporary files: live TV buffers, subtitles taken from
//! streams being remuxed, comskip's work and playlists for other players.

use std::path::PathBuf;
use std::sync::OnceLock;

/// The folder for Xorvio's temporary files. macOS and Windows give each user
/// a temporary folder of their own, so it's that. On Linux `/tmp` is shared
/// by everyone: a folder with a fixed name there could be made in advance by
/// someone else, as a link into this user's files, and Xorvio would clear
/// out or serve what it points at. There it's a folder in the user's cache
/// (`~/.cache/com.xorvio.desktop/tmp`) that only they can enter.
pub fn temp_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(find_root).clone()
}

#[cfg(any(target_os = "macos", windows))]
fn find_root() -> PathBuf {
    std::env::temp_dir()
}

#[cfg(not(any(target_os = "macos", windows)))]
fn find_root() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")));
    if let Some(dir) = cache.map(|cache| cache.join("com.xorvio.desktop").join("tmp")) {
        match private_dir(&dir) {
            Ok(()) => return dir,
            Err(error) => tracing::warn!("the cache folder can't hold temporary files: {error}"),
        }
    }
    // A folder of this run's own, which no one could have made first.
    let dir = std::env::temp_dir().join(format!("xorvio-{}", uuid::Uuid::new_v4().simple()));
    if let Err(error) = private_dir(&dir) {
        tracing::warn!("no folder for temporary files: {error}");
    }
    dir
}

/// Makes `dir` (and any missing parents) so only this user can enter it,
/// and checks it's a real folder, not a link, that they own.
#[cfg(all(unix, any(test, not(target_os = "macos"))))]
fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid has no preconditions and can't fail.
    let uid = unsafe { libc::getuid() };
    if !meta.is_dir() || meta.uid() != uid {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "not a folder of this user's own"));
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::private_dir;

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn makes_a_folder_only_this_user_can_enter() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("cache").join("tmp");
        private_dir(&dir).unwrap();
        assert!(dir.is_dir());
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn closes_a_folder_others_could_enter() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("open");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn refuses_a_link_or_a_file() {
        let base = tempfile::tempdir().unwrap();
        let elsewhere = base.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        assert!(private_dir(&link).is_err());
        let file = base.path().join("file");
        std::fs::write(&file, b"").unwrap();
        assert!(private_dir(&file).is_err());
    }
}
