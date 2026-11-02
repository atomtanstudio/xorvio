//! What the add and edit sheets ask of the core besides saving: trying an
//! Xtream login, noticing an Xtream login inside a playlist link, and
//! keeping a playlist file dropped onto the sheet.

use std::path::{Path, PathBuf};

use serde::Serialize;
use xorvio_model::{SourceConfig, SourceKind, SourceSecrets, Timestamp};

use crate::sources::m3u;
use crate::sources::xtream::{XtreamClient, XtreamCredentials};
use crate::{Core, Result, now, redact};

/// Where playlists dropped onto the add sheet are kept, in Xorvio's data folder.
const PLAYLISTS_FOLDER: &str = "Playlists";

/// What a provider says about an Xtream account, for Test Connection. It
/// never carries the login.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XtreamAccountCheck {
    /// Streams the account may play at once, when the provider says.
    pub max_connections: Option<i64>,
    /// When the account ends, for one that does.
    pub expires: Option<Timestamp>,
}

/// The server, username and password in an Xtream-style playlist link
/// (`…/get.php?username=…&password=…`), so the sheet can offer the Xtream
/// connection instead, which adds series details, replay and the account's
/// status. `None` for any other link.
pub fn xtream_login_in_link(link: &str) -> Option<SourceSecrets> {
    let found = XtreamCredentials::detect(link)?;
    Some(SourceSecrets {
        server_url: Some(found.server),
        username: Some(found.username),
        password: Some(found.password),
        ..Default::default()
    })
}

impl Core {
    /// Signs in to an Xtream provider with what's in the sheet, saving
    /// nothing, and says what the provider reports about the account.
    pub async fn test_xtream_login(&self, secrets: &SourceSecrets, user_agent: Option<&str>) -> Result<XtreamAccountCheck> {
        let server = secrets.server_url.as_deref().and_then(XtreamCredentials::normalize_server);
        let credentials = XtreamCredentials::from_secrets(&SourceSecrets { server_url: server, ..secrets.clone() })
            .ok_or_else(|| anyhow::anyhow!("Enter a server, username and password first."))?;
        let user_agent = user_agent.map(str::trim).filter(|agent| !agent.is_empty());
        let client = XtreamClient { http: &self.http, credentials: &credentials, user_agent };
        match client.authenticate(now()).await {
            Ok(account) => Ok(XtreamAccountCheck { max_connections: account.max_connections, expires: account.expires }),
            // The provider's wording is already plain; this makes sure no part of the login rides along.
            Err(error) => Err(anyhow::anyhow!(redact::redact(&error.to_string(), &redact::secret_values(secrets)))),
        }
    }

    /// Keeps a copy of a playlist dropped onto the add sheet and returns
    /// where, for an M3U file source to read. A dropped file reaches the
    /// interface as its contents only (the web view never says where it
    /// is), so Xorvio keeps its own copy, deleted with the source.
    pub fn keep_playlist_copy(&self, file_name: &str, contents: &[u8]) -> Result<String> {
        m3u::parse_bytes(contents)?;
        let folder = self.playlists_folder();
        std::fs::create_dir_all(&folder)?;
        let path = unused_path(&folder, &playlist_file_name(file_name));
        std::fs::write(&path, contents)?;
        Ok(path.to_string_lossy().into_owned())
    }

    /// Deletes Xorvio's own copy of a dropped playlist once its source has
    /// gone. A file chosen from anywhere else is never touched.
    pub(crate) fn forget_playlist_copy(&self, removed: &SourceConfig) {
        if removed.kind != SourceKind::M3uFile {
            return;
        }
        let Some(path) = removed.file_path.as_deref().map(Path::new) else { return };
        if path.parent() == Some(self.playlists_folder().as_path())
            && let Err(error) = std::fs::remove_file(path)
        {
            tracing::warn!("a removed source's playlist copy could not be deleted: {error}");
        }
    }

    fn playlists_folder(&self) -> PathBuf {
        self.config_file.directory().join(PLAYLISTS_FOLDER)
    }
}

/// A plain file name for a dropped playlist: its own name without any
/// folders or odd characters, ending .m3u or .m3u8.
fn playlist_file_name(given: &str) -> String {
    let base = given.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = base.chars().filter(|c| !c.is_control() && !matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect();
    let clean = clean.trim().trim_start_matches('.');
    let stem = if clean.is_empty() { "Playlist" } else { clean };
    let lower = stem.to_lowercase();
    if lower.ends_with(".m3u") || lower.ends_with(".m3u8") { stem.to_string() } else { format!("{stem}.m3u") }
}

/// `folder/name`, or "name 2", "name 3"… when that's taken.
fn unused_path(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name, "m3u"));
    (2..).map(|n| folder.join(format!("{stem} {n}.{extension}"))).find(|path| !path.exists()).unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_an_xtream_login_in_a_playlist_link() {
        let found = xtream_login_in_link("  http://provider.example:8080/get.php?username=ann&password=s%20cret&type=m3u_plus&output=ts ").unwrap();
        assert_eq!(found.server_url.as_deref(), Some("http://provider.example:8080"));
        assert_eq!(found.username.as_deref(), Some("ann"));
        assert_eq!(found.password.as_deref(), Some("s cret"));
        assert_eq!((found.playlist_url, found.epg_url, found.token), (None, None, None));
        // A panel under a path keeps it.
        let found = xtream_login_in_link("https://panel.example/iptv/get.php?username=u&password=p").unwrap();
        assert_eq!(found.server_url.as_deref(), Some("https://panel.example/iptv"));
        // Ordinary playlists, and Xtream-looking ones without a whole login, aren't offered.
        for link in [
            "https://lists.example/channels.m3u8",
            "http://provider.example/get.php?username=u",
            "http://provider.example/get.php?username=&password=p",
            "http://provider.example/playlist.php?username=u&password=p",
            "not a link",
            "",
        ] {
            assert!(xtream_login_in_link(link).is_none(), "{link}");
        }
    }

    #[test]
    fn dropped_playlists_get_plain_unused_names() {
        assert_eq!(playlist_file_name("My TV.m3u8"), "My TV.m3u8");
        assert_eq!(playlist_file_name("../../etc/passwd"), "passwd.m3u");
        assert_eq!(playlist_file_name("C:\\Lists\\uk:sport?.M3U"), "uksport.M3U");
        assert_eq!(playlist_file_name(".hidden"), "hidden.m3u");
        assert_eq!(playlist_file_name(""), "Playlist.m3u");
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("List.m3u"), "").unwrap();
        std::fs::write(folder.path().join("List 2.m3u"), "").unwrap();
        assert_eq!(unused_path(folder.path(), "List.m3u"), folder.path().join("List 3.m3u"));
        assert_eq!(unused_path(folder.path(), "Other.m3u"), folder.path().join("Other.m3u"));
    }
}
