//! Talking to Trakt: the app's credentials, the connection (a device-code
//! sign-in, its tokens kept fresh), and requests with Trakt's headers, rate
//! limits and status codes, worded for people.
//!
//! The tokens and the device code stay here and in the keychain. They're never logged, never put in an error, and never part of
//! anything the interface receives.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use xorvio_model::{Timestamp, TraktOptions, TraktSignInStart, TraktStatus};

use super::ids::{self, TraktIds, Title};
use crate::error::SourceError;
use crate::http::{ApiCall, Http, RawReply};
use crate::secrets::SecretStore;
use crate::store::library::LibraryStore;
use crate::store::lock;
use crate::{json, now};

/// Trakt's API.
pub(crate) const API_BASE: &str = "https://api.trakt.tv";
/// Where Trakt's sign-in (OAuth) requests go; Trakt asks that they don't go
/// to the API's own address.
pub(crate) const AUTH_BASE: &str = "https://auth.trakt.tv";
/// The redirect address a Trakt app registers when none is given. Trakt lets
/// development apps (the kind anyone can make) redirect only to this
/// computer. Connecting with a code never redirects anywhere, but refreshing
/// a token names the app's address, which must match.
pub(crate) const DEFAULT_REDIRECT_URI: &str = "http://localhost";

fn default_redirect_uri() -> String {
    DEFAULT_REDIRECT_URI.into()
}

/// Keychain entries: a Trakt app entered in Settings, and the connection.
pub(crate) const APP_KEY: &str = "trakt:app";
pub(crate) const CONNECTION_KEY: &str = "trakt:connection";

/// Library preferences.
const OPTIONS_PREFERENCE: &str = "trakt_options";
const LAST_SYNC_PREFERENCE: &str = "trakt_last_sync";
const LAST_ERROR_PREFERENCE: &str = "trakt_last_error";
/// Followed by a part of `/sync/last_activities`.
pub(super) const ACTIVITY_PREFERENCE: &str = "trakt_activity:";

/// The gap Trakt asks for between writes (POST, PUT, DELETE): one a second.
const WRITE_SPACING: Duration = Duration::from_secs(1);
/// The longest Xorvio waits when Trakt says to slow down, before giving up for now.
const LONGEST_RETRY_WAIT: u64 = 60;

/// Why something to do with Trakt didn't work, in words for the person
/// using Xorvio. None of these carry anything Trakt sent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum TraktError {
    #[error("Xorvio has no Trakt app to connect with. Enter a Trakt app's client ID in Settings › Trakt.")]
    NotConfigured,
    #[error("Xorvio isn't connected to Trakt.")]
    NotConnected,
    #[error("Trakt signed Xorvio out. Connect again.")]
    SignedOut,
    #[error("Trakt doesn't know this title.")]
    NotFound,
    /// Trakt turned the request down for a reason the person can act on.
    #[error("{0}")]
    Refused(&'static str),
    #[error("Trakt asked Xorvio to slow down. It will try again later.")]
    RateLimited,
    #[error("Trakt isn't working properly right now (HTTP {0}). Xorvio will try again later.")]
    Unavailable(u16),
    #[error("Trakt responded with HTTP {0}.")]
    Http(u16),
    #[error("Trakt couldn't be reached. {0}")]
    Network(String),
    #[error("Trakt sent something Xorvio couldn't read.")]
    Unreadable,
}

impl From<SourceError> for TraktError {
    fn from(error: SourceError) -> Self {
        Self::Network(error.to_string())
    }
}

/// The error for a reply that wasn't a success (a 401 has been dealt with by then).
fn failure(status: u16) -> TraktError {
    match status {
        401 => TraktError::SignedOut,
        403 => TraktError::Refused("Trakt turned down Xorvio's Trakt app. Check its client ID in Settings › Trakt."),
        404 => TraktError::NotFound,
        420 => TraktError::Refused("Your Trakt account has reached its limit for this. Trakt VIP raises it."),
        423 => TraktError::Refused("Trakt has locked this account. Trakt's support can unlock it."),
        426 => TraktError::Refused("This needs Trakt VIP."),
        429 => TraktError::RateLimited,
        500..=599 => TraktError::Unavailable(status),
        _ => TraktError::Http(status),
    }
}

fn checked(reply: RawReply) -> Result<RawReply, TraktError> {
    if reply.is_success() { Ok(reply) } else { Err(failure(reply.status)) }
}

/// A Trakt app: built in, or entered in Settings. Only its client id is
/// used: Trakt's code sign-in, token refreshes and revoking need no client
/// secret, and Trakt asks that desktop apps never send one (anyone could
/// pull it out of the app). One saved by an earlier version is ignored.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct App {
    pub client_id: String,
    /// The redirect address the app was registered with (apps saved before
    /// this was asked for read as the default).
    #[serde(default = "default_redirect_uri")]
    pub redirect_uri: String,
    #[serde(skip)]
    pub custom: bool,
}

impl App {
    /// The app this build was made with, if any: its client id comes from
    /// the environment at build time, and its redirect address too when it
    /// isn't the default.
    pub fn built_in() -> Option<App> {
        let client_id = option_env!("XORVIO_TRAKT_CLIENT_ID").unwrap_or_default().trim();
        let redirect_uri = option_env!("XORVIO_TRAKT_REDIRECT_URI").map(str::trim).filter(|uri| !uri.is_empty());
        (!client_id.is_empty()).then(|| App {
            client_id: client_id.into(),
            redirect_uri: redirect_uri.unwrap_or(DEFAULT_REDIRECT_URI).into(),
            custom: false,
        })
    }
}

/// A connection to a Trakt account, kept in the keychain.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Connection {
    access_token: String,
    refresh_token: String,
    created_at: Timestamp,
    expires_at: Timestamp,
    username: Option<String>,
    /// The app the tokens belong to: they're no good with another.
    client_id: String,
}

impl Connection {
    /// From Trakt's token reply.
    fn from_tokens(reply: &Value, client_id: &str, username: Option<String>) -> Option<Connection> {
        let created_at = json::int(reply.get("created_at")).filter(|at| *at > 0).unwrap_or_else(now);
        let lifetime = json::int(reply.get("expires_in")).filter(|seconds| *seconds > 0).unwrap_or(86_400);
        Some(Connection {
            access_token: json::string(reply.get("access_token")).filter(|token| !token.is_empty())?,
            refresh_token: json::string(reply.get("refresh_token")).filter(|token| !token.is_empty())?,
            created_at,
            expires_at: created_at + lifetime,
            username,
            client_id: client_id.to_string(),
        })
    }

    /// When to refresh: a quarter of the way before the end, between a minute
    /// and a day early, so a sync every 15 minutes never meets an expired token.
    fn refresh_due(&self) -> Timestamp {
        let lifetime = (self.expires_at - self.created_at).max(0);
        self.expires_at - (lifetime / 4).clamp(60, 86_400)
    }

    pub fn tokens(&self) -> Vec<String> {
        vec![self.access_token.clone(), self.refresh_token.clone()]
    }
}

/// A sign-in waiting for its code to be entered.
struct PendingSignIn {
    device_code: String,
    expires_at: Instant,
    interval: Duration,
    last_check: Option<Instant>,
}

/// Something about Trakt the interface should hear of.
#[derive(Debug, Clone, PartialEq)]
pub enum TraktEvent {
    /// The status changed (connected, signed out, a sync started or ended).
    Status(TraktStatus),
    /// A sync changed progress or the watchlist.
    LibraryChanged,
}

pub type TraktListener = Arc<dyn Fn(TraktEvent) + Send + Sync>;

/// Everything Trakt needs, shared with requests running in the background.
pub(crate) struct Trakt {
    http: Http,
    secrets: Arc<dyn SecretStore>,
    pub(super) library: Arc<LibraryStore>,
    api_base: Mutex<String>,
    auth_base: Mutex<String>,
    built_in: Mutex<Option<App>>,
    /// The keychain's entries, read once: `None` until then.
    app: Mutex<Option<Option<App>>>,
    connection: Mutex<Option<Option<Connection>>>,
    pending: Mutex<Option<PendingSignIn>>,
    /// One token refresh at a time: a refresh token works once.
    refreshing: tokio::sync::Mutex<()>,
    /// One sync at a time.
    pub(super) sync_turn: tokio::sync::Mutex<()>,
    syncing: AtomicBool,
    last_write: tokio::sync::Mutex<Option<Instant>>,
    write_spacing: Mutex<Duration>,
    listener: Mutex<Option<TraktListener>>,
    /// Episodes' Trakt ids by show id, season and number, as looked up for scrobbling.
    episode_ids: Mutex<HashMap<(String, i64, i64), TraktIds>>,
    /// Charts and lists read for Home, by path, with when they were read.
    kept: Mutex<HashMap<String, (Instant, Value)>>,
}

impl Trakt {
    pub fn new(http: Http, secrets: Arc<dyn SecretStore>, library: Arc<LibraryStore>) -> Trakt {
        Trakt {
            http,
            secrets,
            library,
            api_base: Mutex::new(API_BASE.into()),
            auth_base: Mutex::new(AUTH_BASE.into()),
            built_in: Mutex::new(App::built_in()),
            app: Mutex::default(),
            connection: Mutex::default(),
            pending: Mutex::default(),
            refreshing: tokio::sync::Mutex::default(),
            sync_turn: tokio::sync::Mutex::default(),
            syncing: AtomicBool::new(false),
            last_write: tokio::sync::Mutex::default(),
            write_spacing: Mutex::new(WRITE_SPACING),
            listener: Mutex::default(),
            episode_ids: Mutex::default(),
            kept: Mutex::default(),
        }
    }

    #[cfg(test)]
    pub fn use_test_server(&self, base: &str, built_in: Option<&str>, spacing: Duration) {
        *lock(&self.api_base) = base.to_string();
        *lock(&self.auth_base) = base.to_string();
        *lock(&self.built_in) =
            built_in.map(|client_id| App { client_id: client_id.into(), redirect_uri: DEFAULT_REDIRECT_URI.into(), custom: false });
        *lock(&self.write_spacing) = spacing;
    }

    pub fn set_listener(&self, listener: TraktListener) {
        *lock(&self.listener) = Some(listener);
    }

    // MARK: Status and options

    pub fn status(&self) -> TraktStatus {
        let app = self.app();
        let connection = self.connection_for(app.as_ref());
        TraktStatus {
            configured: app.is_some(),
            custom_app: app.as_ref().is_some_and(|app| app.custom),
            connected: connection.is_some(),
            username: connection.and_then(|connection| connection.username),
            last_sync: self.preference(LAST_SYNC_PREFERENCE).and_then(|text| text.parse().ok()),
            last_error: self.preference(LAST_ERROR_PREFERENCE),
            syncing: self.syncing.load(Ordering::SeqCst),
            options: self.options(),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connection_for(self.app().as_ref()).is_some()
    }

    pub fn options(&self) -> TraktOptions {
        self.preference(OPTIONS_PREFERENCE).and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn set_options(&self, options: TraktOptions) -> anyhow::Result<()> {
        self.library.set_preference(OPTIONS_PREFERENCE, Some(&serde_json::to_string(&options)?))?;
        self.notify_status();
        Ok(())
    }

    fn preference(&self, key: &str) -> Option<String> {
        self.library.preference(key).unwrap_or_else(|error| {
            tracing::warn!("a Trakt setting couldn't be read: {error}");
            None
        })
    }

    fn set_preference(&self, key: &str, value: Option<&str>) {
        if let Err(error) = self.library.set_preference(key, value) {
            tracing::warn!("a Trakt setting couldn't be saved: {error}");
        }
    }

    pub fn set_syncing(&self, syncing: bool) {
        self.syncing.store(syncing, Ordering::SeqCst);
        self.notify_status();
    }

    /// Notes how a sync went: when it finished well, or why it didn't.
    pub fn record_sync(&self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.set_preference(LAST_SYNC_PREFERENCE, Some(&now().to_string()));
                self.set_preference(LAST_ERROR_PREFERENCE, None);
            }
            Err(message) => self.set_preference(LAST_ERROR_PREFERENCE, Some(&message)),
        }
    }

    pub fn notify_status(&self) {
        let listener = lock(&self.listener).clone();
        if let Some(listener) = listener {
            listener(TraktEvent::Status(self.status()));
        }
    }

    pub fn notify_library(&self) {
        let listener = lock(&self.listener).clone();
        if let Some(listener) = listener {
            listener(TraktEvent::LibraryChanged);
        }
    }

    /// A part of `/sync/last_activities` as it was at the last sync.
    pub fn activity(&self, part: &str) -> Option<String> {
        self.preference(&format!("{ACTIVITY_PREFERENCE}{part}"))
    }

    pub fn set_activity(&self, part: &str, value: &str) {
        self.set_preference(&format!("{ACTIVITY_PREFERENCE}{part}"), Some(value));
    }

    // MARK: The app

    /// The Trakt app in use: one entered in Settings, else the built-in one.
    pub fn app(&self) -> Option<App> {
        let mut cached = lock(&self.app);
        if cached.is_none() {
            let stored = match self.secrets.get(APP_KEY) {
                Ok(text) => text.and_then(|text| serde_json::from_str::<App>(&text).ok()),
                Err(error) => {
                    tracing::warn!("the Trakt app couldn't be read from the keychain: {error}");
                    None
                }
            };
            *cached = Some(stored.map(|app| App { custom: true, ..app }));
        }
        cached.clone().flatten().or_else(|| lock(&self.built_in).clone())
    }

    /// Uses this Trakt app instead of the built-in one; an empty client id
    /// goes back to the built-in one, an empty redirect address is the
    /// default. Changing apps disconnects: tokens belong to an app.
    pub async fn set_app(&self, client_id: &str, redirect_uri: &str) -> anyhow::Result<()> {
        let (client_id, redirect_uri) = (client_id.trim(), redirect_uri.trim());
        let chosen = (!client_id.is_empty()).then(|| App {
            client_id: client_id.into(),
            redirect_uri: if redirect_uri.is_empty() { DEFAULT_REDIRECT_URI.into() } else { redirect_uri.into() },
            custom: true,
        });
        let next = chosen.clone().or_else(|| lock(&self.built_in).clone());
        let current = self.app();
        if current.as_ref().map(|app| &app.client_id) != next.as_ref().map(|app| &app.client_id) && self.is_connected() {
            self.disconnect().await?;
        }
        match &chosen {
            Some(app) => self.secrets.set(APP_KEY, &serde_json::to_string(app)?)?,
            None => self.secrets.delete(APP_KEY)?,
        }
        *lock(&self.app) = Some(chosen);
        self.notify_status();
        Ok(())
    }

    // MARK: The connection

    fn connection(&self) -> Option<Connection> {
        let mut cached = lock(&self.connection);
        if cached.is_none() {
            let stored = match self.secrets.get(CONNECTION_KEY) {
                Ok(text) => text.and_then(|text| serde_json::from_str::<Connection>(&text).ok()),
                Err(error) => {
                    tracing::warn!("the Trakt connection couldn't be read from the keychain: {error}");
                    None
                }
            };
            *cached = Some(stored);
        }
        cached.clone().flatten()
    }

    /// The connection, when it belongs to this app.
    fn connection_for(&self, app: Option<&App>) -> Option<Connection> {
        let app = app?;
        self.connection().filter(|connection| connection.client_id == app.client_id)
    }

    fn save_connection(&self, connection: Option<&Connection>) -> Result<(), TraktError> {
        let saved = match connection {
            Some(connection) => serde_json::to_string(connection).map_err(anyhow::Error::from).and_then(|text| self.secrets.set(CONNECTION_KEY, &text)),
            None => self.secrets.delete(CONNECTION_KEY),
        };
        // Lists and recommendations read as one account aren't another's.
        let account = |connection: Option<&Connection>| connection.map(|connection| connection.username.clone());
        if account(lock(&self.connection).clone().flatten().as_ref()) != account(connection) {
            lock(&self.kept).clear();
        }
        // Kept in memory either way, so a keychain that won't take it doesn't
        // also lose the connection for this session.
        *lock(&self.connection) = Some(connection.cloned());
        saved.map_err(|error| {
            tracing::warn!("the Trakt connection couldn't be saved in the keychain: {error}");
            TraktError::Refused(if cfg!(windows) {
                "Windows Credential Manager wouldn't keep Trakt's sign-in. Connect again."
            } else {
                "The keychain wouldn't keep Trakt's sign-in. Connect again."
            })
        })
    }

    /// Trakt turned the connection down (its tokens were revoked, or expired
    /// beyond refreshing): forget it, and say so.
    fn signed_out_by_trakt(&self) {
        let _ = self.save_connection(None);
        if let Err(error) = self.library.forget_trakt_account() {
            tracing::warn!("what Trakt said couldn't be cleared: {error}");
        }
        self.set_preference(LAST_ERROR_PREFERENCE, Some(&TraktError::SignedOut.to_string()));
        self.notify_status();
    }

    /// Starts connecting: asks Trakt for a code to enter at its verification address.
    pub async fn start_sign_in(&self) -> Result<TraktSignInStart, TraktError> {
        let app = self.app().ok_or(TraktError::NotConfigured)?;
        let url = format!("{}/oauth/device/code", lock(&self.auth_base));
        let reply = self.send(&app, Method::POST, &url, None, Some(&json!({"client_id": app.client_id})), true).await?;
        if matches!(reply.status, 401 | 403) {
            return Err(TraktError::Refused("Trakt didn't accept Xorvio's Trakt app. Check its client ID in Settings › Trakt."));
        }
        let body = checked(reply)?.json();
        let device_code = json::string(body.get("device_code")).filter(|code| !code.is_empty()).ok_or(TraktError::Unreadable)?;
        let user_code = json::string(body.get("user_code")).filter(|code| !code.is_empty()).ok_or(TraktError::Unreadable)?;
        let verification_url = json::string(body.get("verification_url")).unwrap_or_else(|| "https://trakt.tv/activate".into());
        let expires_in = json::int(body.get("expires_in")).filter(|seconds| *seconds > 0).unwrap_or(600);
        let interval = json::int(body.get("interval")).filter(|seconds| *seconds >= 0).unwrap_or(5);
        *lock(&self.pending) = Some(PendingSignIn {
            device_code,
            expires_at: Instant::now() + Duration::from_secs(expires_in as u64),
            interval: Duration::from_secs(interval as u64),
            last_check: None,
        });
        Ok(TraktSignInStart { user_code, verification_url, expires_in, interval })
    }

    /// Whether the code has been entered and Xorvio allowed: false while
    /// waiting. Once allowed, the tokens and the username are kept.
    pub async fn check_sign_in(&self) -> anyhow::Result<bool> {
        let app = self.app().ok_or(TraktError::NotConfigured)?;
        let device_code = {
            let mut pending = lock(&self.pending);
            let Some(waiting) = pending.as_mut() else {
                if self.connection_for(Some(&app)).is_some() {
                    return Ok(true);
                }
                anyhow::bail!("Start connecting to Trakt again.");
            };
            if waiting.expires_at <= Instant::now() {
                *pending = None;
                anyhow::bail!("The code has expired. Start connecting again for a new one.");
            }
            // Trakt says to slow down when asked more often than it said to.
            let tolerance = Duration::from_millis(500);
            if waiting.last_check.is_some_and(|last| last.elapsed() + tolerance < waiting.interval) {
                return Ok(false);
            }
            waiting.last_check = Some(Instant::now());
            waiting.device_code.clone()
        };
        let url = format!("{}/oauth/device/token", lock(&self.auth_base));
        let body = json!({"code": device_code, "client_id": app.client_id});
        let reply = self.send(&app, Method::POST, &url, None, Some(&body), false).await?;
        let gone = |message: &str| -> anyhow::Result<bool> {
            *lock(&self.pending) = None;
            Err(anyhow::anyhow!("{message}"))
        };
        match reply.status {
            200..=299 => {}
            400 => return Ok(false),
            429 => {
                if let Some(waiting) = lock(&self.pending).as_mut() {
                    waiting.interval += Duration::from_secs(1);
                }
                return Ok(false);
            }
            404 => return gone("Trakt didn't recognise the code. Start connecting again."),
            409 => return gone("This code has already been used. Start connecting again."),
            410 => return gone("The code has expired. Start connecting again for a new one."),
            418 => return gone("Xorvio wasn't allowed on Trakt. Start connecting again if that was a mistake."),
            status => return Err(failure(status).into()),
        }
        let mut connection = Connection::from_tokens(&reply.json(), &app.client_id, None).ok_or(TraktError::Unreadable)?;
        connection.username = self.username(&app, &connection.access_token).await;
        // Whatever an account connected before said goes, so it can't speak for this one.
        self.library.forget_trakt_account()?;
        self.save_connection(Some(&connection))?;
        *lock(&self.pending) = None;
        self.set_preference(LAST_ERROR_PREFERENCE, None);
        self.set_preference(LAST_SYNC_PREFERENCE, None);
        self.notify_status();
        Ok(true)
    }

    /// The account's username, from its settings.
    async fn username(&self, app: &App, token: &str) -> Option<String> {
        let url = format!("{}/users/settings", lock(&self.api_base));
        match self.send(app, Method::GET, &url, Some(token), None, true).await.and_then(checked) {
            Ok(reply) => {
                let body = reply.json();
                json::string(body.get("user").and_then(|user| user.get("username")))
            }
            Err(error) => {
                tracing::warn!("the Trakt username couldn't be read: {error}");
                None
            }
        }
    }

    /// Revokes the connection on Trakt (if it can be reached) and forgets it,
    /// with everything Trakt said about the account.
    pub async fn disconnect(&self) -> anyhow::Result<()> {
        if let (Some(app), Some(connection)) = (self.app(), self.connection()) {
            let url = format!("{}/oauth/revoke", lock(&self.auth_base));
            let body = json!({"token": connection.access_token, "client_id": app.client_id});
            match self.send(&app, Method::POST, &url, None, Some(&body), true).await.and_then(checked) {
                Ok(_) => {}
                Err(error) => tracing::warn!("Trakt couldn't be told Xorvio disconnected: {error}"),
            }
        }
        *lock(&self.pending) = None;
        let saved = self.save_connection(None);
        self.library.forget_trakt_account()?;
        self.set_preference(LAST_ERROR_PREFERENCE, None);
        self.set_preference(LAST_SYNC_PREFERENCE, None);
        lock(&self.episode_ids).clear();
        self.notify_status();
        saved.map_err(Into::into)
    }

    /// A current access token, refreshed first when it's close to expiring.
    async fn access_token(&self, app: &App) -> Result<String, TraktError> {
        let connection = self.connection_for(Some(app)).ok_or(TraktError::NotConnected)?;
        if now() < connection.refresh_due() {
            return Ok(connection.access_token);
        }
        let _turn = self.refreshing.lock().await;
        // Another request may have refreshed it while this one waited.
        let connection = self.connection_for(Some(app)).ok_or(TraktError::NotConnected)?;
        if now() < connection.refresh_due() {
            return Ok(connection.access_token);
        }
        Ok(self.refresh(app, &connection).await?.access_token)
    }

    /// A new access token after Trakt turned `rejected` down, unless another
    /// request already got one.
    async fn token_after_rejection(&self, app: &App, rejected: &str) -> Result<String, TraktError> {
        let _turn = self.refreshing.lock().await;
        let connection = self.connection_for(Some(app)).ok_or(TraktError::SignedOut)?;
        if connection.access_token != rejected {
            return Ok(connection.access_token);
        }
        Ok(self.refresh(app, &connection).await?.access_token)
    }

    /// Swaps the refresh token for new tokens. Trakt refusing it means the
    /// connection is over; not reaching Trakt keeps it for another try.
    async fn refresh(&self, app: &App, connection: &Connection) -> Result<Connection, TraktError> {
        let url = format!("{}/oauth/token", lock(&self.auth_base));
        let body = json!({
            "refresh_token": connection.refresh_token,
            "client_id": app.client_id,
            "redirect_uri": app.redirect_uri,
            "grant_type": "refresh_token",
        });
        let reply = self.send(app, Method::POST, &url, None, Some(&body), true).await?;
        match reply.status {
            200..=299 => {
                let renewed = Connection::from_tokens(&reply.json(), &app.client_id, connection.username.clone())
                    .ok_or(TraktError::Unreadable)?;
                self.save_connection(Some(&renewed))?;
                Ok(renewed)
            }
            400 | 401 => {
                self.signed_out_by_trakt();
                Err(TraktError::SignedOut)
            }
            status => Err(failure(status)),
        }
    }

    // MARK: Requests

    pub async fn get(&self, path: &str) -> Result<Value, TraktError> {
        Ok(self.request(Method::GET, path, None).await?.json())
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value, TraktError> {
        Ok(self.request(Method::POST, path, Some(body)).await?.json())
    }

    pub async fn delete(&self, path: &str) -> Result<(), TraktError> {
        self.request(Method::DELETE, path, None).await.map(|_| ())
    }

    /// A read anyone may make (charts, public lists): as the connected
    /// account when there is one, so its own private lists can be read too,
    /// else with the app's key alone.
    pub async fn get_public(&self, path: &str) -> Result<Value, TraktError> {
        if self.is_connected() {
            return self.get(path).await;
        }
        let app = self.app().ok_or(TraktError::NotConfigured)?;
        let url = format!("{}{path}", lock(&self.api_base));
        Ok(checked(self.send(&app, Method::GET, &url, None, None, true).await?)?.json())
    }

    /// A public read kept for a while: Trakt's answer to `path` while it's
    /// younger than `fresh`, else asked again, else (when Trakt can't be
    /// reached) the last answer, however old.
    pub async fn get_kept(&self, path: &str, fresh: Duration) -> Result<Value, TraktError> {
        let kept = lock(&self.kept).get(path).cloned();
        if let Some((at, value)) = &kept
            && at.elapsed() < fresh
        {
            return Ok(value.clone());
        }
        match self.get_public(path).await {
            Ok(value) => {
                lock(&self.kept).insert(path.to_string(), (Instant::now(), value.clone()));
                Ok(value)
            }
            Err(error @ (TraktError::Network(_) | TraktError::Unavailable(_) | TraktError::RateLimited)) => kept.map(|(_, value)| value).ok_or(error),
            Err(error) => Err(error),
        }
    }

    /// A request to the API as the connected account. A token Trakt turns
    /// down is refreshed once and the request tried again.
    async fn request(&self, method: Method, path: &str, body: Option<&Value>) -> Result<RawReply, TraktError> {
        let app = self.app().ok_or(TraktError::NotConfigured)?;
        let url = format!("{}{path}", lock(&self.api_base));
        let token = self.access_token(&app).await?;
        let reply = self.send(&app, method.clone(), &url, Some(&token), body, true).await?;
        if reply.status != 401 {
            return checked(reply);
        }
        let token = self.token_after_rejection(&app, &token).await?;
        let reply = self.send(&app, method, &url, Some(&token), body, true).await?;
        if reply.status == 401 {
            self.signed_out_by_trakt();
            return Err(TraktError::SignedOut);
        }
        checked(reply)
    }

    /// Sends one request with Trakt's headers. Writes wait their turn (one a
    /// second); a 429 waits as long as Retry-After says and tries again,
    /// twice at most, unless `wait_when_limited` is off.
    async fn send(
        &self,
        app: &App,
        method: Method,
        url: &str,
        token: Option<&str>,
        body: Option<&Value>,
        wait_when_limited: bool,
    ) -> Result<RawReply, TraktError> {
        let mut headers = vec![
            ("trakt-api-version".to_string(), "2".to_string()),
            ("trakt-api-key".to_string(), app.client_id.clone()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        if let Some(token) = token {
            headers.push(("Authorization".to_string(), format!("Bearer {token}")));
        }
        let mut attempt = 0;
        loop {
            if method != Method::GET {
                self.write_turn().await;
            }
            let call = ApiCall { method: method.clone(), url, headers: &headers, json: body };
            let reply = self.http.exchange(call).await?;
            if reply.status == 429 && wait_when_limited && attempt < 2 {
                attempt += 1;
                let wait = reply
                    .header("retry-after")
                    .and_then(|text| text.trim().parse::<u64>().ok())
                    .unwrap_or(1 << attempt)
                    .min(LONGEST_RETRY_WAIT);
                tracing::debug!("Trakt asked to wait {wait} s");
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            return Ok(reply);
        }
    }

    /// Waits until a second has passed since the last write began.
    async fn write_turn(&self) {
        let spacing = *lock(&self.write_spacing);
        let mut last = self.last_write.lock().await;
        if let Some(previous) = *last {
            tokio::time::sleep_until((previous + spacing).into()).await;
        }
        *last = Some(Instant::now());
    }

    /// What to mask in a log line: the tokens.
    pub fn hidden(&self) -> Vec<String> {
        self.connection().map(|connection| connection.tokens()).unwrap_or_default()
    }

    // MARK: Titles

    /// A title's ids on Trakt: its own, else what looking it up by title and
    /// year finds (once; the answer, "nothing" included, is kept).
    pub async fn resolve(&self, title: &Title) -> Option<TraktIds> {
        if !title.ids.is_empty() {
            return Some(title.ids.clone());
        }
        let key = title.key();
        if key.is_empty() {
            return None;
        }
        match self.library.trakt_lookup(title.show, &key, title.year) {
            Ok(Some(found)) => return found,
            Ok(None) => {}
            Err(error) => {
                tracing::warn!("a Trakt lookup couldn't be read: {error}");
                return None;
            }
        }
        let kind = if title.show { "show" } else { "movie" };
        let query: String = url::form_urlencoded::byte_serialize(title.name.as_bytes()).collect();
        let mut path = format!("/search/{kind}?query={query}&fields=title");
        if let Some(year) = title.year {
            path.push_str(&format!("&years={year}"));
        }
        let found = match self.get(&path).await {
            Ok(results) => ids::confident_match(&results, title.show, &key, title.year),
            // Not reaching Trakt isn't an answer; it's asked again next time.
            Err(error) => {
                tracing::debug!("a title couldn't be looked up on Trakt: {error}");
                return None;
            }
        };
        if let Err(error) = self.library.save_trakt_lookup(title.show, &key, title.year, found.as_ref(), now()) {
            tracing::warn!("a Trakt lookup couldn't be saved: {error}");
        }
        found
    }

    /// An episode's own ids, from its show's ids, season and number (the
    /// scrobble endpoints take episodes by their ids).
    pub async fn episode_ids(&self, show: &TraktIds, season: i64, number: i64) -> Result<TraktIds, TraktError> {
        let show_id = match (show.trakt, &show.imdb, show.tmdb) {
            (Some(trakt), _, _) => trakt.to_string(),
            (None, Some(imdb), _) => imdb.clone(),
            // Trakt's show pages take its own ids or IMDb's, not TMDB's.
            (None, None, Some(tmdb)) => {
                let results = self.get(&format!("/search/tmdb/{tmdb}?type=show")).await?;
                let found = results
                    .as_array()
                    .and_then(|results| results.iter().find_map(|result| TraktIds::from_json(result.get("show")?.get("ids")).trakt));
                found.ok_or(TraktError::NotFound)?.to_string()
            }
            (None, None, None) => return Err(TraktError::NotFound),
        };
        let key = (show_id.clone(), season, number);
        if let Some(found) = lock(&self.episode_ids).get(&key) {
            return Ok(found.clone());
        }
        let episode = self.get(&format!("/shows/{show_id}/seasons/{season}/episodes/{number}")).await?;
        let found = TraktIds::from_json(episode.get("ids"));
        if found.is_empty() {
            return Err(TraktError::NotFound);
        }
        lock(&self.episode_ids).insert(key, found.clone());
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_refreshed_well_before_they_expire() {
        let reply = json!({"access_token": "a", "refresh_token": "r", "expires_in": 604_800, "created_at": 1_000_000});
        let week = Connection::from_tokens(&reply, "id", None).unwrap();
        assert_eq!(week.expires_at, 1_604_800);
        assert_eq!(week.refresh_due(), 1_604_800 - 86_400, "a day early for a week-long token");
        let day = Connection::from_tokens(&json!({"access_token": "a", "refresh_token": "r", "expires_in": 86_400, "created_at": 1_000}), "id", None).unwrap();
        assert_eq!(day.refresh_due(), 1_000 + 86_400 - 21_600, "a quarter early for a day-long one");
        let undated = Connection::from_tokens(&json!({"access_token": "a", "refresh_token": "r"}), "id", None).unwrap();
        assert!(undated.created_at >= now() - 5 && undated.expires_at == undated.created_at + 86_400, "a day from now when Trakt doesn't say");
        assert!(Connection::from_tokens(&json!({"access_token": "a"}), "id", None).is_none(), "no refresh token");
    }

    #[test]
    fn failures_are_worded_for_people() {
        assert_eq!(failure(503), TraktError::Unavailable(503));
        assert_eq!(failure(429), TraktError::RateLimited);
        assert!(matches!(failure(420), TraktError::Refused(message) if message.contains("VIP")));
        assert_eq!(TraktError::SignedOut.to_string(), "Trakt signed Xorvio out. Connect again.");
    }
}
