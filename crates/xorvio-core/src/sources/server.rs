//! Media servers: Plex, Jellyfin and Emby. They hold a person's own films
//! and series, with artwork, details and what's been watched, and play their
//! files directly. This module is all the rest of Xorvio sees of them;
//! [`plex`] and [`jellyfin`] (which speaks to Emby too) know each server's API.
//!
//! Tokens stay out of the catalog and the interface: titles keep the
//! server's own keys ([`StreamLocator::Server`]), artwork keeps the server's
//! own paths behind a `xorvio-server` address ([`artwork_url`]) that adds the
//! token as it's fetched, and stream addresses are made only at playback.
//!
//! [`plex`]: super::plex
//! [`jellyfin`]: super::jellyfin

use xorvio_model::{MediaKind, SourceKind, SourceSecrets, StreamTarget, Timestamp};

use super::{jellyfin, plex};
use crate::details::DetailsRecord;
use crate::error::SourceError;
use crate::http::Http;
use crate::snapshot::CatalogSnapshot;

/// The scheme of artwork addresses, served by the app (see [`artwork_url`]).
/// Not `xorvio-art`, which names the demo library's drawn artwork.
pub const ARTWORK_SCHEME: &str = "xorvio-server";

/// Everything needed to talk to one signed-in server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerSession {
    pub kind: SourceKind,
    /// The server's address with no trailing slash ("http://192.168.1.5:8096").
    pub base_url: String,
    pub token: Option<String>,
    /// Jellyfin and Emby: the signed-in user's id. Plex: the server's machine identifier.
    pub user_id: Option<String>,
    /// This copy of Xorvio, as servers list it among their devices.
    pub device_id: String,
}

impl ServerSession {
    pub fn from_secrets(kind: SourceKind, secrets: &SourceSecrets, device_id: &str) -> Option<Self> {
        if !kind.is_media_server() {
            return None;
        }
        let base_url = secrets.server_url.as_deref().and_then(normalize_address)?;
        Some(Self {
            kind,
            base_url,
            token: secrets.token.clone(),
            user_id: secrets.user_id.clone(),
            device_id: device_id.to_string(),
        })
    }
}

/// What a server remembers of a film or episode: where it was left, or that
/// it was watched. Merged into Xorvio's own progress after a refresh.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerProgress {
    /// The catalog id ("<source>|mv|<key>", "<source>|se|<key>|ep|<key>").
    pub item_id: String,
    /// [`MediaKind::Movie`] or [`MediaKind::Episode`].
    pub kind: MediaKind,
    pub series_id: Option<String>,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub finished: bool,
    /// When it was last played, if the server says.
    pub updated_at: Option<Timestamp>,
}

/// A Jellyfin or Emby sign-in, or a Plex server checked at an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerLogin {
    /// The address that answered, normalized ([`normalize_address`]).
    pub base_url: String,
    pub token: Option<String>,
    /// As [`ServerSession::user_id`].
    pub user_id: Option<String>,
    /// The server's own name ("Living Room"), for naming the source.
    pub server_name: Option<String>,
}

/// An address as typed ("192.168.1.5:8096", "https://media.example/jellyfin/")
/// made into a base address: a scheme (http when none was given), no
/// trailing slash, no query. `None` when it isn't an address.
pub fn normalize_address(typed: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    let with_scheme = if typed.contains("://") { typed.to_string() } else { format!("http://{typed}") };
    let mut parsed = url::Url::parse(&with_scheme).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return None;
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    Some(parsed.as_str().trim_end_matches('/').to_string())
}

/// The address the interface shows a server's artwork at: the app fetches
/// `path` (with its query, e.g. "/Items/abc/Images/Primary?tag=1") from the
/// source's server, adding the token. Stored in the catalog in place of the
/// server's own link, so the catalog never holds a token and keeps working
/// when the server's address changes.
pub fn artwork_url(source_id: &str, path: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(path.as_bytes()).collect();
    if cfg!(windows) {
        format!("http://{ARTWORK_SCHEME}.localhost/{source_id}/{encoded}")
    } else {
        format!("{ARTWORK_SCHEME}://localhost/{source_id}/{encoded}")
    }
}

/// The source id and server path in an artwork address's path
/// ("/<source>/<encoded path>"), or `None` when it isn't one.
pub fn parse_artwork_path(request_path: &str) -> Option<(String, String)> {
    let (source_id, encoded) = request_path.trim_start_matches('/').split_once('/')?;
    let path: String = url::form_urlencoded::parse(format!("p={encoded}").as_bytes()).find(|(key, _)| key == "p")?.1.into_owned();
    is_server_path(&path).then(|| (source_id.to_string(), path))
}

/// A path on the server itself: starting with one slash, never an address
/// elsewhere, so artwork can't be used to fetch from another host.
pub(crate) fn is_server_path(path: &str) -> bool {
    path.starts_with('/') && !path.starts_with("//") && !path.contains("://") && !path.contains('\\')
}

// MARK: - What each server does

/// Signs in to Jellyfin or Emby with a username and password, or checks a
/// Plex server at an address (with a token when it needs one; the password
/// is unused). Errors say plainly what went wrong: the address didn't answer,
/// isn't that kind of server, or turned the login down.
pub(crate) async fn sign_in(
    http: &Http,
    kind: SourceKind,
    address: &str,
    username: &str,
    password: &str,
    token: Option<&str>,
    device_id: &str,
) -> Result<ServerLogin, SourceError> {
    let base_url = normalize_address(address).ok_or_else(|| SourceError::MissingConfiguration("a server address".into()))?;
    match kind {
        SourceKind::Plex => plex::check_server(http, &base_url, token, device_id).await,
        SourceKind::Jellyfin | SourceKind::Emby => {
            jellyfin::sign_in(http, kind, &base_url, username, password, device_id).await
        }
        _ => Err(not_a_server()),
    }
}

/// The server's whole library as a catalog: its film and series libraries
/// as categories, every film, series and episode, the details its lists
/// carry, and what's been watched. `report` is called with a
/// [`super::phase`] step as the work moves on.
pub(crate) async fn fetch_catalog(
    http: &Http,
    session: &ServerSession,
    source_id: &str,
    report: &(dyn Fn(&str) + Send + Sync),
) -> Result<CatalogSnapshot, SourceError> {
    match session.kind {
        SourceKind::Plex => plex::fetch_catalog(http, session, source_id, report).await,
        SourceKind::Jellyfin | SourceKind::Emby => jellyfin::fetch_catalog(http, session, source_id, report).await,
        _ => Err(not_a_server()),
    }
}

/// Fuller details for a film or series than its list carries (cast with
/// photos, studio, tagline…), asked for when it's opened. `key` is the
/// server's key ([`StreamLocator::Server`]'s, or the series' `remote_id`).
///
/// [`StreamLocator::Server`]: xorvio_model::StreamLocator::Server
pub(crate) async fn fetch_details(
    http: &Http,
    session: &ServerSession,
    source_id: &str,
    kind: MediaKind,
    key: &str,
) -> Result<DetailsRecord, SourceError> {
    match session.kind {
        SourceKind::Plex => plex::fetch_details(http, session, source_id, kind, key).await,
        SourceKind::Jellyfin | SourceKind::Emby => jellyfin::fetch_details(http, session, source_id, kind, key).await,
        _ => Err(not_a_server()),
    }
}

/// The address and headers that play a film or episode's file as it is
/// (no transcoding: Xorvio's own gateway converts what it must). The token
/// goes in a header rather than the address wherever the server allows.
pub(crate) fn stream_target(session: &ServerSession, key: &str, part: Option<&str>) -> Result<StreamTarget, SourceError> {
    match session.kind {
        SourceKind::Plex => plex::stream_target(session, key, part),
        SourceKind::Jellyfin | SourceKind::Emby => jellyfin::stream_target(session, key, part),
        _ => Err(not_a_server()),
    }
}

/// The address and headers to fetch artwork at `path` on the server.
pub(crate) fn artwork_request(session: &ServerSession, path: &str) -> Result<(String, Vec<(String, String)>), SourceError> {
    if !is_server_path(path) {
        return Err(SourceError::Unsupported("That isn't artwork on this server.".into()));
    }
    let headers = match session.kind {
        SourceKind::Plex => plex::auth_headers(session),
        SourceKind::Jellyfin | SourceKind::Emby => jellyfin::auth_headers(session),
        _ => return Err(not_a_server()),
    };
    Ok((format!("{}{path}", session.base_url), headers))
}

/// Tells the server where a film or episode was left, or that it was
/// finished, so other apps carry on from there.
pub(crate) async fn report_progress(
    http: &Http,
    session: &ServerSession,
    key: &str,
    position_seconds: f64,
    duration_seconds: f64,
    finished: bool,
) -> Result<(), SourceError> {
    match session.kind {
        SourceKind::Plex => plex::report_progress(http, session, key, position_seconds, duration_seconds, finished).await,
        SourceKind::Jellyfin | SourceKind::Emby => {
            jellyfin::report_progress(http, session, key, position_seconds, duration_seconds, finished).await
        }
        _ => Err(not_a_server()),
    }
}

/// Marks a film or episode watched or unwatched on the server.
pub(crate) async fn set_played(http: &Http, session: &ServerSession, key: &str, played: bool) -> Result<(), SourceError> {
    match session.kind {
        SourceKind::Plex => plex::set_played(http, session, key, played).await,
        SourceKind::Jellyfin | SourceKind::Emby => jellyfin::set_played(http, session, key, played).await,
        _ => Err(not_a_server()),
    }
}

fn not_a_server() -> SourceError {
    SourceError::Unsupported("This source isn't a media server.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalized() {
        assert_eq!(normalize_address("192.168.1.5:8096").as_deref(), Some("http://192.168.1.5:8096"));
        assert_eq!(normalize_address(" https://media.example/jellyfin/?x=1 ").as_deref(), Some("https://media.example/jellyfin"));
        assert_eq!(normalize_address("http://plex.local:32400/").as_deref(), Some("http://plex.local:32400"));
        assert_eq!(normalize_address(""), None);
        assert_eq!(normalize_address("ftp://host"), None);
    }

    #[test]
    fn artwork_addresses_round_trip_and_stay_on_the_server() {
        let address = artwork_url("src-1", "/Items/abc/Images/Primary?tag=x y&maxWidth=600");
        let parsed = url::Url::parse(&address).unwrap();
        assert_eq!(parse_artwork_path(parsed.path()), Some(("src-1".into(), "/Items/abc/Images/Primary?tag=x y&maxWidth=600".into())));
        for hostile in ["http://evil.example/x", "//evil.example/x", "x/y", "/a\\\\b"] {
            let address = artwork_url("s", hostile);
            assert_eq!(parse_artwork_path(url::Url::parse(&address).unwrap().path()), None, "{hostile}");
        }
    }
}
