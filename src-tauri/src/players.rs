//! Other media players on this computer that a stream can be handed to:
//! IINA, VLC and Infuse. They're found by bundle id through Launch Services,
//! wherever they're installed, and given a gateway address (never the
//! provider's) that works until Xorvio quits.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{Context, anyhow};
use serde::Serialize;

/// An installed player, as the interface lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalPlayer {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Iina,
    Vlc,
    Infuse,
}

struct Known {
    id: &'static str,
    name: &'static str,
    bundle_id: &'static str,
    kind: Kind,
}

/// Infuse 7 and 8 share a bundle id, so either is found.
const KNOWN: [Known; 3] = [
    Known { id: "iina", name: "IINA", bundle_id: "com.colliderli.iina", kind: Kind::Iina },
    Known { id: "vlc", name: "VLC", bundle_id: "org.videolan.vlc", kind: Kind::Vlc },
    Known { id: "infuse", name: "Infuse", bundle_id: "com.firecore.infuse", kind: Kind::Infuse },
];

/// Films and episodes this far in or more start where they were in the
/// other player; before that, from the beginning.
const MIN_START_SECONDS: f64 = 5.0;
/// VLC's playlists are cleared out after this long.
const PLAYLIST_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// The players that are installed, always in the same order.
pub fn installed() -> Vec<ExternalPlayer> {
    offered(is_installed)
}

fn offered(installed: impl Fn(&str) -> bool) -> Vec<ExternalPlayer> {
    KNOWN
        .iter()
        .filter(|p| installed(p.bundle_id))
        .map(|p| ExternalPlayer { id: p.id.into(), name: p.name.into() })
        .collect()
}

#[cfg(target_os = "macos")]
fn is_installed(bundle_id: &str) -> bool {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;
    let Some(url) = NSWorkspace::sharedWorkspace().URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id)) else {
        return false;
    };
    // Launch Services can remember an app that has since gone to the Bin.
    url.path().map(|path| path.to_string()).is_some_and(|path| !path.contains("/.Trash/") && std::path::Path::new(&path).exists())
}

#[cfg(not(target_os = "macos"))]
fn is_installed(_bundle_id: &str) -> bool {
    false
}

/// How a player is asked to play an address.
#[derive(Debug, PartialEq)]
enum Launch {
    /// The player's own link, opened as it is.
    Link(String),
    /// A playlist written for the player to open, which carries the title
    /// and where to start.
    Playlist(String),
}

fn launch(kind: Kind, address: &str, title: &str, start_seconds: Option<f64>) -> Launch {
    let start = start_seconds.filter(|s| s.is_finite() && *s >= MIN_START_SECONDS);
    match kind {
        // IINA's link takes mpv options, `mpv_start` among them, but mpv
        // keeps an option for every file that window plays afterwards; so
        // IINA starts at the beginning rather than start later files midway.
        Kind::Iina => Launch::Link(format!("iina://weblink?url={}", encode(address))),
        // Infuse's link takes the address and nothing about where to start.
        Kind::Infuse => Launch::Link(format!("infuse://x-callback-url/play?url={}", encode(address))),
        // VLC applies a playlist entry's `start-time` option to that entry only.
        Kind::Vlc => {
            let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
            let mut playlist = format!("#EXTM3U\n#EXTINF:-1,{title}\n");
            if let Some(start) = start {
                playlist.push_str(&format!("#EXTVLCOPT:start-time={start:.0}\n"));
            }
            playlist.push_str(address);
            playlist.push('\n');
            Launch::Playlist(playlist)
        }
    }
}

/// Opens `address` in the player with id `player_id`, starting at
/// `start_seconds` where the player takes a start time.
pub fn open(player_id: &str, address: &str, title: &str, start_seconds: Option<f64>) -> anyhow::Result<()> {
    let player = KNOWN.iter().find(|p| p.id == player_id).ok_or_else(|| anyhow!("Xorvio doesn't know that player."))?;
    let argument = match launch(player.kind, address, title, start_seconds) {
        Launch::Link(link) => link,
        Launch::Playlist(text) => write_playlist(&text)?.to_string_lossy().into_owned(),
    };
    open_with(player.bundle_id, &argument).with_context(|| format!("{} couldn't be opened.", player.name))
}

#[cfg(target_os = "macos")]
fn open_with(bundle_id: &str, argument: &str) -> anyhow::Result<()> {
    let status = std::process::Command::new("/usr/bin/open").args(["-b", bundle_id, argument]).status()?;
    anyhow::ensure!(status.success(), "open exited with {status}");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn open_with(_bundle_id: &str, _argument: &str) -> anyhow::Result<()> {
    anyhow::bail!("Opening in another player works on macOS for now.")
}

/// Writes a playlist into a folder of Xorvio's in the temporary folder,
/// clearing out ones a day old.
fn write_playlist(text: &str) -> anyhow::Result<PathBuf> {
    let dir = xorvio_media::temp_root().join("xorvio-players");
    std::fs::create_dir_all(&dir)?;
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > PLAYLIST_LIFETIME);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let path = dir.join(format!("{stamp}.m3u"));
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Percent-encodes a whole address to go in a link's query.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDRESS: &str = "http://127.0.0.1:50123/token/s/id/Severance%20-%20S01E03.mkv";

    #[test]
    fn only_installed_players_are_offered_in_order() {
        let ids = |installed: &[&str]| offered(|bundle| installed.contains(&bundle)).into_iter().map(|p| p.id).collect::<Vec<_>>();
        assert!(ids(&[]).is_empty());
        assert_eq!(ids(&["com.firecore.infuse", "com.colliderli.iina"]), ["iina", "infuse"]);
        assert_eq!(ids(&["org.videolan.vlc", "org.videolan.vlc.other"]), ["vlc"]);
        assert_eq!(offered(|b| b == "com.colliderli.iina")[0].name, "IINA");
    }

    #[test]
    fn iina_and_infuse_get_their_own_links() {
        let encoded = "http%3A%2F%2F127.0.0.1%3A50123%2Ftoken%2Fs%2Fid%2FSeverance%2520-%2520S01E03.mkv";
        assert_eq!(launch(Kind::Iina, ADDRESS, "Severance", Some(600.0)), Launch::Link(format!("iina://weblink?url={encoded}")));
        assert_eq!(launch(Kind::Infuse, ADDRESS, "Severance", None), Launch::Link(format!("infuse://x-callback-url/play?url={encoded}")));
    }

    #[test]
    fn vlc_gets_a_playlist_with_the_title_and_where_to_start() {
        let Launch::Playlist(text) = launch(Kind::Vlc, ADDRESS, "Severance - S01E03\n- Good News", Some(1234.6)) else { panic!() };
        assert_eq!(text, format!("#EXTM3U\n#EXTINF:-1,Severance - S01E03 - Good News\n#EXTVLCOPT:start-time=1235\n{ADDRESS}\n"));
        for start in [None, Some(2.0), Some(f64::NAN)] {
            let Launch::Playlist(text) = launch(Kind::Vlc, ADDRESS, "Channel One", start) else { panic!() };
            assert!(!text.contains("start-time"), "{start:?}: {text}");
        }
    }

    #[test]
    fn unknown_players_are_refused() {
        assert!(open("quicktime", ADDRESS, "x", None).is_err());
    }
}
