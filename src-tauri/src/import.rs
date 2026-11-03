//! Brings sources over from the Swift version of Xorvio for Mac, so nobody
//! has to type a provider login twice. Only the source settings and logins
//! move; the catalog is rebuilt by the first refresh.

use std::path::PathBuf;

use serde::Deserialize;
use xorvio_model::{LiveStreamFormat, NewSource, SourceKind, SourceSecrets};

/// The Swift app's keychain service for source logins.
const SWIFT_KEYCHAIN_SERVICE: &str = "com.xorvio.source-credentials";

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwiftSource {
    id: String,
    name: String,
    kind: String,
    #[serde(default)]
    live_format: Option<String>,
}

/// SourceSecrets as the Swift app encoded them (Codable's default keys).
#[derive(Debug, Default, Deserialize)]
struct SwiftSecrets {
    #[serde(rename = "playlistURL")]
    playlist_url: Option<String>,
    #[serde(rename = "epgURL")]
    epg_url: Option<String>,
    #[serde(rename = "serverURL")]
    server_url: Option<String>,
    username: Option<String>,
    password: Option<String>,
}

/// The Swift app's sources.json: the development build's folder, then the
/// sandboxed App Store build's container.
fn swift_sources_file() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    [
        home.join("Library/Application Support/Xorvio/sources.json"),
        home.join("Library/Containers/com.xorvio.Xorvio/Data/Library/Application Support/Xorvio/sources.json"),
    ]
    .into_iter()
    .find(|p| p.exists())
}

/// Whether there's anything to import, so the interface only offers it then.
pub fn swift_sources_available() -> bool {
    swift_sources_file().is_some()
}

/// Reads the Swift app's provider sources and their logins. macOS asks the
/// person to allow access to the other app's keychain items: the one place
/// Xorvio lets it ask, since the person chose to import them.
pub fn read_swift_sources() -> anyhow::Result<Vec<NewSource>> {
    let path = swift_sources_file().ok_or_else(|| anyhow::anyhow!("Xorvio for Mac's sources weren't found on this Mac."))?;
    let sources: Vec<SwiftSource> = serde_json::from_slice(&std::fs::read(path)?)?;
    let mut out = Vec::new();
    for source in sources {
        let kind = match source.kind.as_str() {
            "xtream" => SourceKind::Xtream,
            "m3uURL" => SourceKind::M3uUrl,
            // Playlist files were sandbox bookmarks; they have to be chosen again.
            _ => continue,
        };
        let secrets: SwiftSecrets = xorvio_core::allowing_keychain_dialogs(|| keyring::Entry::new(SWIFT_KEYCHAIN_SERVICE, &source.id).and_then(|entry| entry.get_password()))
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        let has_login = match kind {
            SourceKind::Xtream => secrets.server_url.is_some() && secrets.username.is_some() && secrets.password.is_some(),
            _ => secrets.playlist_url.is_some(),
        };
        if !has_login {
            continue;
        }
        out.push(NewSource {
            name: source.name,
            kind,
            secrets: SourceSecrets {
                playlist_url: secrets.playlist_url,
                epg_url: secrets.epg_url,
                server_url: secrets.server_url,
                username: secrets.username,
                password: secrets.password,
                ..Default::default()
            },
            file_path: None,
            live_format: if source.live_format.as_deref() == Some("mpegts") { LiveStreamFormat::Mpegts } else { LiveStreamFormat::Hls },
            user_agent: None,
        });
    }
    Ok(out)
}

// MARK: - Add-ons and watchlist

/// Xorvio Personal's keychain item for its add-on links: a JSON list of addresses.
const PERSONAL_ADDONS_SERVICE: &str = "com.xorvio.personal.addons";
const PERSONAL_ADDONS_ACCOUNT: &str = "stream-addons";

/// Stremio's own catalogue. Xorvio Personal had it built in for titles,
/// search and episode lists; here it's an ordinary add-on.
pub const CINEMETA: &str = "https://v3-cinemeta.strem.io/manifest.json";

/// The Swift apps' data folders, the everyday one first.
fn swift_folders() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else { return vec![] };
    ["Xorvio Personal", "Xorvio"]
        .iter()
        .map(|name| home.join("Library/Application Support").join(name))
        .filter(|p| p.join("Library/library.sqlite").exists())
        .collect()
}

/// Whether either Swift app has a watchlist or Personal has add-ons to bring over.
pub fn swift_library_available() -> bool {
    !swift_folders().is_empty()
}

/// Personal's add-on links. macOS asks the person to allow reading them.
/// They can hold an account key, so they're never logged.
pub fn read_personal_addons() -> Vec<String> {
    xorvio_core::allowing_keychain_dialogs(|| keyring::Entry::new(PERSONAL_ADDONS_SERVICE, PERSONAL_ADDONS_ACCOUNT).and_then(|entry| entry.get_secret()))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
        .unwrap_or_default()
}

/// A watchlist row as the Swift apps stored it.
#[derive(Debug, Clone)]
pub struct SwiftWatchlistRow {
    pub item_id: String,
    pub kind: String,
    pub source_id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub artwork: Option<String>,
    pub added_at: i64,
    pub channel_id: Option<String>,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub remind: bool,
}

/// One Swift app's sources (id, kind, name) and its watchlist rows.
pub type SwiftWatchlist = (Vec<(String, String, String)>, Vec<SwiftWatchlistRow>);

/// Both Swift apps' watchlists, with each app's source ids and names (to
/// match them to sources here). Read-only: the Swift apps' files aren't changed.
pub fn read_swift_watchlists() -> Vec<SwiftWatchlist> {
    swift_folders()
        .into_iter()
        .filter_map(|folder| {
            let sources: Vec<SwiftSource> = std::fs::read(folder.join("sources.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
            let db = rusqlite::Connection::open_with_flags(folder.join("Library/library.sqlite"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
            let mut statement = db
                .prepare("SELECT item_id, kind, source_id, title, subtitle, artwork, added_at, channel_id, start, end, remind FROM watchlist")
                .ok()?;
            let rows = statement
                .query_map([], |row| {
                    Ok(SwiftWatchlistRow {
                        item_id: row.get(0)?,
                        kind: row.get(1)?,
                        source_id: row.get(2)?,
                        title: row.get(3)?,
                        subtitle: row.get(4)?,
                        artwork: row.get(5)?,
                        added_at: row.get::<_, f64>(6)? as i64,
                        channel_id: row.get(7)?,
                        start: row.get::<_, Option<f64>>(8)?.map(|t| t as i64),
                        end: row.get::<_, Option<f64>>(9)?.map(|t| t as i64),
                        remind: row.get::<_, i64>(10)? != 0,
                    })
                })
                .ok()?
                .filter_map(Result::ok)
                .collect();
            Some((sources.into_iter().map(|s| (s.id, s.kind, s.name)).collect(), rows))
        })
        .collect()
}

/// A progress row as the Swift apps stored it.
#[derive(Debug, Clone)]
pub struct SwiftProgressRow {
    pub item_id: String,
    pub kind: String,
    pub source_id: String,
    pub series_id: Option<String>,
    pub position: f64,
    pub duration: f64,
    pub finished: bool,
    pub updated_at: i64,
}

/// One Swift app's sources (id, kind, name) and its progress rows.
pub type SwiftProgress = (Vec<(String, String, String)>, Vec<SwiftProgressRow>);

/// Both Swift apps' watch progress, read-only.
pub fn read_swift_progress() -> Vec<SwiftProgress> {
    swift_folders()
        .into_iter()
        .filter_map(|folder| {
            let sources: Vec<SwiftSource> = std::fs::read(folder.join("sources.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
            let db = rusqlite::Connection::open_with_flags(folder.join("Library/library.sqlite"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
            let mut statement = db
                .prepare("SELECT item_id, kind, source_id, series_id, position, duration, finished, updated_at FROM progress")
                .ok()?;
            let rows = statement
                .query_map([], |row| {
                    Ok(SwiftProgressRow {
                        item_id: row.get(0)?,
                        kind: row.get(1)?,
                        source_id: row.get(2)?,
                        series_id: row.get(3)?,
                        position: row.get(4)?,
                        duration: row.get(5)?,
                        finished: row.get::<_, i64>(6)? != 0,
                        updated_at: row.get::<_, f64>(7)? as i64,
                    })
                })
                .ok()?
                .filter_map(Result::ok)
                .collect();
            Some((sources.into_iter().map(|s| (s.id, s.kind, s.name)).collect(), rows))
        })
        .collect()
}

/// The Swift linked library's ids ("linked-addons|mv|tt0058430") as add-on ids
/// here ("addon|movie|tt0058430").
pub fn addon_item_id(swift_id: &str) -> Option<String> {
    let rest = swift_id.strip_prefix("linked-addons|")?;
    let (kind, id) = rest.split_once('|')?;
    let media_type = match kind {
        "mv" => "movie",
        "se" => "series",
        _ => return None,
    };
    Some(format!("addon|{media_type}|{id}"))
}

/// A provider item's id with the Swift source id swapped for this app's.
pub fn remap_source(id: &str, from: &str, to: &str) -> Option<String> {
    id.strip_prefix(from).filter(|rest| rest.starts_with('|')).map(|rest| format!("{to}{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swift_ids_map_to_this_apps_ids() {
        assert_eq!(addon_item_id("linked-addons|mv|tt0058430").as_deref(), Some("addon|movie|tt0058430"));
        assert_eq!(addon_item_id("linked-addons|se|tt4934214").as_deref(), Some("addon|series|tt4934214"));
        assert_eq!(addon_item_id("a3159550|se|x:1702"), None);
        assert_eq!(
            addon_item_id("linked-addons|se|tt5875444|ep|tt5875444:6:1").as_deref(),
            Some("addon|series|tt5875444|ep|tt5875444:6:1")
        );
        assert_eq!(remap_source("a3159550|se|x:1702", "a3159550", "3679d442").as_deref(), Some("3679d442|se|x:1702"));
        assert_eq!(remap_source("a3159550x|se|1", "a3159550", "new"), None);
    }
}
