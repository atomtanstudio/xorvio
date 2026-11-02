//! Media server sources (Plex, Jellyfin, Emby) as the rest of the app uses
//! them: signing in, the session behind each source, artwork, and keeping
//! what's been watched the same here and on the server.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Serialize;
use xorvio_model::{MediaKind, PlaybackProgress, SourceConfig, SourceKind, SourceSecrets, StreamLocator};

use crate::error::SourceError;
use crate::http::ApiCall;
use crate::sources::plex::{self, PlexServer};
use crate::sources::server::{self, ServerProgress, ServerSession};
use crate::store::lock;
use crate::{Core, Result, now, redact};

/// How long an approved Plex sign-in waits for a server to be picked.
const PLEX_SIGN_IN_LIFETIME: Duration = Duration::from_secs(30 * 60);

/// A Plex sign-in, kept here (never sent to the interface) from approval
/// until a server is added.
pub(crate) struct PlexSignIn {
    servers: Vec<PlexServer>,
    approved_at: Instant,
}

/// A Plex sign-in waiting for approval on plex.tv.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlexSignInStart {
    pub pin_id: i64,
    pub code: String,
    /// The page to open in the browser.
    pub auth_url: String,
}

/// A server the signed-in Plex account can add.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlexServerChoice {
    pub id: String,
    pub name: String,
    pub owned: bool,
}

impl Core {
    /// This copy of Xorvio's id in servers' device lists, made once.
    pub(crate) fn device_id(&self) -> String {
        if let Ok(Some(id)) = self.library.preference("device_id") {
            return id;
        }
        let id = uuid::Uuid::new_v4().to_string();
        if let Err(error) = self.library.set_preference("device_id", Some(&id)) {
            tracing::warn!("the device id could not be saved: {error}");
        }
        id
    }

    /// The signed-in session behind a media server source.
    pub(crate) fn server_session(&self, source_id: &str) -> Result<ServerSession> {
        let config = self.config(source_id)?;
        let secrets = self.load_secrets(source_id)?;
        Ok(session_for(&config, &secrets, &self.device_id())?)
    }

    /// Signs in to the server a new source names. Returns what to keep (the
    /// address, username, token and user id; never the password) and the
    /// server's own name.
    pub(crate) async fn sign_in_to_server(
        &self,
        kind: SourceKind,
        secrets: &SourceSecrets,
    ) -> std::result::Result<(SourceSecrets, Option<String>), SourceError> {
        let address = secrets.server_url.as_deref().ok_or_else(|| SourceError::MissingConfiguration("a server address".into()))?;
        let login = server::sign_in(
            &self.http,
            kind,
            address,
            secrets.username.as_deref().unwrap_or_default(),
            secrets.password.as_deref().unwrap_or_default(),
            secrets.token.as_deref(),
            &self.device_id(),
        )
        .await?;
        let kept = SourceSecrets {
            server_url: Some(login.base_url),
            username: secrets.username.clone(),
            token: login.token,
            user_id: login.user_id,
            ..Default::default()
        };
        Ok((kept, login.server_name))
    }

    /// Signs a Jellyfin or Emby source in again (its sign-in expired or was
    /// revoked) as the same user, keeping everything else about it.
    pub async fn sign_in_again(&self, source_id: &str, password: &str) -> Result<()> {
        let config = self.config(source_id)?;
        if !matches!(config.kind, SourceKind::Jellyfin | SourceKind::Emby) {
            anyhow::bail!("Only Jellyfin and Emby sources sign in again with a password.");
        }
        let secrets = self.load_secrets(source_id)?;
        let attempt = SourceSecrets { password: Some(password.to_string()), token: None, ..secrets };
        let (kept, _) = self.sign_in_to_server(config.kind, &attempt).await?;
        crate::secrets::save(self.secrets.as_ref(), source_id, &kept)
    }

    // MARK: Plex sign-in

    /// Starts signing in to Plex: the code and the plex.tv page to approve it on.
    pub async fn plex_start_sign_in(&self) -> Result<PlexSignInStart> {
        let plex_tv = lock(&self.plex_tv_base).clone();
        let pin = plex::start_sign_in(&self.http, &plex_tv, &self.device_id()).await?;
        Ok(PlexSignInStart { pin_id: pin.id, code: pin.code, auth_url: pin.auth_url })
    }

    /// The account's servers once the sign-in is approved on plex.tv; `None`
    /// while it's still waiting.
    pub async fn plex_check_sign_in(&self, pin_id: i64) -> Result<Option<Vec<PlexServerChoice>>> {
        if let Some(waiting) = lock(&self.plex_sign_ins).get(&pin_id) {
            return Ok(Some(choices(&waiting.servers)));
        }
        let device = self.device_id();
        let plex_tv = lock(&self.plex_tv_base).clone();
        let Some(account_token) = plex::check_sign_in(&self.http, &plex_tv, &device, pin_id).await? else { return Ok(None) };
        let servers = plex::servers(&self.http, &plex_tv, &device, &account_token).await?;
        let listed = choices(&servers);
        let mut waiting = lock(&self.plex_sign_ins);
        waiting.retain(|_, sign_in| sign_in.approved_at.elapsed() < PLEX_SIGN_IN_LIFETIME);
        waiting.insert(pin_id, PlexSignIn { servers, approved_at: Instant::now() });
        Ok(Some(listed))
    }

    /// Adds a server from an approved Plex sign-in, at the first of its
    /// addresses that answers. The catalog is read by a separate refresh.
    pub async fn add_plex_server(&self, pin_id: i64, server_id: &str, name: &str) -> Result<SourceConfig> {
        let server = lock(&self.plex_sign_ins)
            .get(&pin_id)
            .and_then(|sign_in| sign_in.servers.iter().find(|server| server.id == server_id).cloned())
            .ok_or_else(|| anyhow::anyhow!("This Plex sign-in has expired. Sign in again."))?;
        let base_url = plex::best_connection(&self.http, &self.device_id(), &server).await?;
        let secrets = SourceSecrets {
            server_url: Some(base_url),
            token: Some(server.access_token.clone()),
            user_id: Some(server.id.clone()),
            ..Default::default()
        };
        let name = match name.trim() {
            "" => server.name.clone(),
            name => name.to_string(),
        };
        let config = self.save_new_source(SourceKind::Plex, name, &secrets)?;
        lock(&self.plex_sign_ins).remove(&pin_id);
        Ok(config)
    }

    /// Gives a Plex source the token from a new Plex sign-in, for when its
    /// own expired or was revoked. The account must still reach the same server.
    pub async fn replace_plex_sign_in(&self, source_id: &str, pin_id: i64) -> Result<()> {
        if self.config(source_id)?.kind != SourceKind::Plex {
            anyhow::bail!("This source isn't a Plex server.");
        }
        let secrets = self.load_secrets(source_id)?;
        let machine = secrets.user_id.clone().unwrap_or_default();
        let server = lock(&self.plex_sign_ins)
            .get(&pin_id)
            .map(|sign_in| sign_in.servers.iter().find(|server| server.id == machine).cloned())
            .ok_or_else(|| anyhow::anyhow!("This Plex sign-in has expired. Sign in again."))?
            .ok_or_else(|| anyhow::anyhow!("This Plex account can't reach this server. Sign in with the account that added it."))?;
        let base_url = plex::best_connection(&self.http, &self.device_id(), &server).await?;
        let renewed = SourceSecrets { server_url: Some(base_url), token: Some(server.access_token), ..secrets };
        crate::secrets::save(self.secrets.as_ref(), source_id, &renewed)?;
        lock(&self.plex_sign_ins).remove(&pin_id);
        Ok(())
    }

    // MARK: Artwork

    /// Artwork from a media server source (a path from a `xorvio-server`
    /// address), with its content type.
    pub async fn artwork(&self, source_id: &str, path: &str) -> Result<(Vec<u8>, Option<String>)> {
        let session = self.server_session(source_id)?;
        let (url, headers) = server::artwork_request(&session, path)?;
        let reply = self.http.api(ApiCall::get(&url, &headers)).await?;
        Ok((reply.bytes, reply.content_type))
    }

    // MARK: What's been watched

    /// Takes in what a server remembers watching after a refresh. The
    /// server wins where it knows of something later than Xorvio does;
    /// Xorvio's own newer progress is kept.
    pub(crate) fn merge_server_progress(&self, source_id: &str, progress: &[ServerProgress]) -> Result<()> {
        let at = now();
        for item in progress {
            let local = self.library.progress(&item.item_id)?;
            let server_time = item.updated_at.unwrap_or(0);
            let take = match &local {
                None => item.finished || item.position_seconds > 0.0,
                // Watched on the server, while here it was only started (and not since).
                Some(local) if item.finished => !local.finished && local.updated_at <= server_time.max(1),
                Some(local) => item.position_seconds > 0.0 && server_time > local.updated_at,
            };
            if !take {
                continue;
            }
            self.library.save_progress(&PlaybackProgress {
                item_id: item.item_id.clone(),
                kind: item.kind,
                source_id: source_id.to_string(),
                series_id: item.series_id.clone(),
                position_seconds: if item.finished { item.duration_seconds } else { item.position_seconds },
                duration_seconds: item.duration_seconds,
                finished: item.finished,
                updated_at: if server_time > 0 { server_time } else { at },
            })?;
        }
        Ok(())
    }

    /// Tells a film or episode's server where it was left, in the
    /// background. Anything but a media server's title is left alone.
    pub(crate) fn send_progress_to_server(&self, progress: &PlaybackProgress) {
        let Some((session, key)) = self.server_item(progress) else { return };
        let http = self.http.clone();
        let (position, duration, finished) = (progress.position_seconds, progress.duration_seconds, progress.finished);
        let hidden = session.token.iter().cloned().collect();
        spawn_report("progress", hidden, async move {
            server::report_progress(&http, &session, &key, position, duration, finished).await
        });
    }

    /// Marks a film or episode watched or unwatched on its server, in the background.
    pub(crate) fn send_watched_to_server(&self, item: &PlaybackProgress, watched: bool) {
        let Some((session, key)) = self.server_item(item) else { return };
        let http = self.http.clone();
        let hidden = session.token.iter().cloned().collect();
        spawn_report("watched state", hidden, async move { server::set_played(&http, &session, &key, watched).await });
    }

    /// The session and the server's key for a film or episode on a media server.
    fn server_item(&self, item: &PlaybackProgress) -> Option<(ServerSession, String)> {
        let config = self.config(&item.source_id).ok().filter(|config| config.kind.is_media_server())?;
        let locator = match item.kind {
            MediaKind::Movie => self.catalog.movies_by_ids(std::slice::from_ref(&item.item_id)).ok()?.pop()?.stream,
            MediaKind::Episode => self.catalog.episodes_by_ids(std::slice::from_ref(&item.item_id)).ok()?.pop()?.stream,
            _ => return None,
        };
        let StreamLocator::Server { key, .. } = locator else { return None };
        let session = self.server_session(&config.id).ok()?;
        Some((session, key))
    }
}

/// A source's session, from its settings and secrets.
pub(crate) fn session_for(config: &SourceConfig, secrets: &SourceSecrets, device_id: &str) -> std::result::Result<ServerSession, SourceError> {
    ServerSession::from_secrets(config.kind, secrets, device_id)
        .ok_or_else(|| SourceError::MissingConfiguration("a server address".into()))
}

fn choices(servers: &[PlexServer]) -> Vec<PlexServerChoice> {
    servers.iter().map(|server| PlexServerChoice { id: server.id.clone(), name: server.name.clone(), owned: server.owned }).collect()
}

/// Runs a report to a server off to the side; a failure is logged (without
/// the token) and otherwise ignored, since playing goes on regardless.
fn spawn_report(
    what: &'static str,
    hidden: Vec<String>,
    report: impl std::future::Future<Output = std::result::Result<(), SourceError>> + Send + 'static,
) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    runtime.spawn(async move {
        if let Err(error) = report.await {
            tracing::warn!("the server's {what} couldn't be updated: {}", redact::redact(&error.to_string(), &hidden));
        }
    });
}

/// Approved Plex sign-ins by PIN id.
pub(crate) type PlexSignIns = HashMap<i64, PlexSignIn>;
