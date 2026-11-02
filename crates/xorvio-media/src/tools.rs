//! Finding ffmpeg and ffprobe.

use std::path::{Path, PathBuf};

/// The ffmpeg tools the gateway and the recorder run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// What Xorvio's own copies are called ("xorvio-ffmpeg"), so a Linux
/// package's can sit in `/usr/bin` beside the distribution's.
const OWN_PREFIX: &str = "xorvio-";

impl Tools {
    /// Names a folder holding the ffmpeg and ffprobe to use instead of any
    /// other, for running the tests against the copies a release bundles
    /// (`src-tauri/binaries`).
    pub const DIR_VARIABLE: &str = "XORVIO_FFMPEG_DIR";

    /// The folder [`Tools::DIR_VARIABLE`] names, and nothing else, when it's set:
    /// a run against a particular build never quietly uses another one.
    /// Otherwise the copies bundled with the app first, then ones on `PATH`,
    /// then Homebrew's and `/usr/local/bin` (an app opened from the Finder gets
    /// a `PATH` without them). `None` when either tool is missing.
    ///
    /// Tauri puts bundled copies ("sidecars") next to the app's executable:
    /// `Contents/MacOS` on macOS, the install folder on Windows, `usr/bin` in
    /// a Linux package. It drops their target-triple suffix when it bundles
    /// them, so names with and without it are looked for.
    pub fn locate() -> Option<Tools> {
        match std::env::var_os(Tools::DIR_VARIABLE) {
            Some(dir) if !dir.is_empty() => Tools::in_dir(Path::new(&dir)),
            _ => Some(Tools {
                ffmpeg: find("ffmpeg")?,
                ffprobe: find("ffprobe")?,
            }),
        }
    }

    fn in_dir(dir: &Path) -> Option<Tools> {
        Some(Tools {
            ffmpeg: in_dir(dir, "ffmpeg")?,
            ffprobe: in_dir(dir, "ffprobe")?,
        })
    }
}

/// comskip, when it's installed, for finding recordings' ad breaks. It's a
/// separate GPL program Xorvio never bundles, so only `PATH`, Homebrew and
/// `/usr/local/bin` are looked in.
pub fn comskip() -> Option<PathBuf> {
    on_path("comskip").or_else(|| {
        ["/opt/homebrew/bin", "/usr/local/bin"]
            .iter()
            .map(|dir| Path::new(dir).join(executable("comskip")))
            .find(|path| is_executable(path))
    })
}

fn find(name: &str) -> Option<PathBuf> {
    sidecar(name).or_else(|| on_path(name)).or_else(|| {
        ["/opt/homebrew/bin", "/usr/local/bin"]
            .iter()
            .map(|dir| Path::new(dir).join(executable(name)))
            .find(|path| is_executable(path))
    })
}

fn sidecar(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    in_dir(exe.parent()?, name)
}

/// `name` in `dir`: Xorvio's own copy first ("xorvio-ffmpeg"), then any
/// other ("ffmpeg"), each as it is or with Tauri's target-triple suffix.
fn in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    [format!("{OWN_PREFIX}{name}"), name.to_string()]
        .into_iter()
        .flat_map(|base| {
            std::iter::once(executable(&base)).chain(
                target_triples()
                    .into_iter()
                    .map(move |triple| executable(&format!("{base}-{triple}"))),
            )
        })
        .map(|file| dir.join(file))
        .find(|path| is_executable(path))
}

fn on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(executable(name)))
        .find(|path| is_executable(path))
}

fn executable(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// The suffixes Tauri gives sidecars ("ffmpeg-aarch64-apple-darwin").
fn target_triples() -> Vec<String> {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "macos" => vec![format!("{arch}-apple-darwin")],
        "windows" => vec![
            format!("{arch}-pc-windows-msvc"),
            format!("{arch}-pc-windows-gnu"),
        ],
        "linux" => vec![
            format!("{arch}-unknown-linux-gnu"),
            format!("{arch}-unknown-linux-musl"),
        ],
        os => vec![format!("{arch}-unknown-{os}")],
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triples_name_this_platform() {
        assert!(
            target_triples()
                .iter()
                .all(|t| t.starts_with(std::env::consts::ARCH))
        );
    }

    #[test]
    fn finds_tools_when_installed() {
        // Only meaningful where ffmpeg is installed; elsewhere `None` is right.
        if let Some(tools) = Tools::locate() {
            assert!(is_executable(&tools.ffmpeg) && is_executable(&tools.ffprobe));
        }
    }

    fn make_executable(path: &Path) {
        std::fs::write(path, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn a_folder_holds_both_tools_under_any_of_their_names() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Tools::in_dir(dir.path()), None);
        // As src-tauri/binaries holds them, before Tauri bundles them.
        let suffixed = executable(&format!("xorvio-ffmpeg-{}", target_triples()[0]));
        make_executable(&dir.path().join(&suffixed));
        assert_eq!(Tools::in_dir(dir.path()), None, "ffprobe is missing");
        // Someone else's, as in /usr/bin or Homebrew's folder.
        make_executable(&dir.path().join(executable("ffprobe")));
        assert_eq!(
            Tools::in_dir(dir.path()),
            Some(Tools {
                ffmpeg: dir.path().join(&suffixed),
                ffprobe: dir.path().join(executable("ffprobe")),
            })
        );
        // Xorvio's own, as a bundle holds them, comes before someone else's.
        make_executable(&dir.path().join(executable("xorvio-ffprobe")));
        assert_eq!(in_dir(dir.path(), "ffprobe"), Some(dir.path().join(executable("xorvio-ffprobe"))));
    }

    #[cfg(unix)]
    #[test]
    fn files_that_cant_run_are_passed_over() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("xorvio-ffmpeg"), b"").unwrap();
        make_executable(&dir.path().join("ffmpeg"));
        assert_eq!(in_dir(dir.path(), "ffmpeg"), Some(dir.path().join("ffmpeg")));
    }
}
