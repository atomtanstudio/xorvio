//! The shared networking policy: bounded timeouts, no cookies, capped reply
//! sizes, and errors that never repeat the address they were about (provider
//! links carry logins).

use std::path::PathBuf;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::StatusCode;
use reqwest::header::USER_AGENT;
use tokio::io::AsyncWriteExt;

use crate::error::SourceError;

/// How long a server may take to answer, or to send the next part of a reply.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(20);
/// The longest a whole download may run, however steadily it arrives.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// The most a playlist or provider reply may be. Real ones are well under
/// this; a provider (or anyone on a plain-HTTP connection) sending more is
/// stopped before it fills memory.
pub(crate) const MAX_RESPONSE_BYTES: usize = 512 << 20;
/// The most a downloaded guide file may be, before decompressing.
pub(crate) const MAX_DOWNLOAD_BYTES: u64 = 2 << 30;

const DEFAULT_USER_AGENT: &str = concat!("Xorvio/", env!("CARGO_PKG_VERSION"));

#[derive(Clone)]
pub(crate) struct Http {
    client: reqwest::Client,
    /// For live streams, which run for as long as they're read: no limit on
    /// the whole request, only on connecting (and each wait, see [`Http::open_live`]).
    live: reqwest::Client,
}

impl Http {
    pub fn new() -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(DEFAULT_USER_AGENT)
            .connect_timeout(IDLE_TIMEOUT)
            .timeout(TOTAL_TIMEOUT)
            .build()?;
        let live = reqwest::Client::builder().user_agent(DEFAULT_USER_AGENT).connect_timeout(IDLE_TIMEOUT).build()?;
        Ok(Self { client, live })
    }

    /// A live stream's reply, once its status says it's coming. The caller
    /// reads the body, bounding each wait for more itself.
    pub async fn open_live(&self, url: &str, headers: &[(String, String)]) -> Result<reqwest::Response, SourceError> {
        let mut request = self.live.get(url);
        for (name, value) in headers {
            request = request.header(name.as_str(), value.as_str());
        }
        self.dispatch(request).await.map_err(Failure::into_error)
    }

    /// The whole reply as bytes, retried once after a dropped connection or a
    /// timeout. `file://` links (a playlist or guide the user chose on this
    /// computer) are read from disk.
    pub async fn bytes(&self, link: &str, user_agent: Option<&str>) -> Result<Vec<u8>, SourceError> {
        if let Some(path) = local_path(link) {
            return tokio::fs::read(path).await.map_err(|_| SourceError::FileUnavailable);
        }
        let mut attempt = 0;
        loop {
            match self.fetch(link, user_agent).await {
                Err(Failure::Transient(error)) if attempt == 0 => {
                    tracing::debug!("retrying a request after a transient failure: {error}");
                    attempt += 1;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(Failure::Transient(error) | Failure::Final(error)) => return Err(error),
                Ok(bytes) => return Ok(bytes),
            }
        }
    }

    /// A JSON reply. An empty reply reads as an empty list, which is what
    /// panels mean by it.
    pub async fn json(&self, link: &str, user_agent: Option<&str>) -> Result<serde_json::Value, SourceError> {
        let bytes = self.bytes(link, user_agent).await?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(serde_json::Value::Array(Vec::new()));
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| SourceError::InvalidPlaylist("the server did not return valid data".into()))
    }

    /// Downloads to a temporary file that removes itself, so a large guide
    /// never has to fit in memory.
    pub async fn download(&self, link: &str, user_agent: Option<&str>) -> Result<Downloaded, SourceError> {
        if let Some(path) = local_path(link) {
            return if path.is_file() { Ok(Downloaded::Local(path)) } else { Err(SourceError::FileUnavailable) };
        }
        let response = self.send(link, user_agent).await.map_err(Failure::into_error)?;
        if response.content_length().is_some_and(|length| length > MAX_DOWNLOAD_BYTES) {
            return Err(too_large_guide());
        }
        let file = tempfile::NamedTempFile::new()
            .map_err(|_| SourceError::Network("A temporary file for the download could not be created.".into()))?;
        let writer_handle = file
            .as_file()
            .try_clone()
            .map_err(|_| SourceError::Network("A temporary file for the download could not be created.".into()))?;
        let mut writer = tokio::fs::File::from_std(writer_handle);
        let mut stream = response.bytes_stream();
        let mut written: u64 = 0;
        loop {
            let next = tokio::time::timeout(IDLE_TIMEOUT, stream.next()).await.map_err(|_| timed_out())?;
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(|error| classify(&error).into_error())?;
            written += chunk.len() as u64;
            if written > MAX_DOWNLOAD_BYTES {
                return Err(too_large_guide());
            }
            writer
                .write_all(&chunk)
                .await
                .map_err(|_| SourceError::Network("The download could not be saved.".into()))?;
        }
        writer.flush().await.map_err(|_| SourceError::Network("The download could not be saved.".into()))?;
        Ok(Downloaded::Temporary(file))
    }

    /// A request to a server's API (Plex, Jellyfin, Emby) with its own
    /// method, headers and JSON body, and the reply's content type. Reads
    /// (GET) are retried once like links; writes aren't, since they may
    /// already have happened.
    pub async fn api(&self, call: ApiCall<'_>) -> Result<ApiReply, SourceError> {
        let retry = call.method == reqwest::Method::GET;
        let mut attempt = 0;
        loop {
            let mut request = self.client.request(call.method.clone(), call.url);
            for (name, value) in call.headers {
                request = request.header(name.as_str(), value.as_str());
            }
            if let Some(body) = call.json {
                request = request.json(body);
            }
            let result = match self.dispatch(request).await {
                Ok(response) => {
                    let content_type = response
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_string);
                    read_body(response).await.map(|bytes| ApiReply { bytes, content_type })
                }
                Err(failure) => Err(failure),
            };
            match result {
                Err(Failure::Transient(error)) if retry && attempt == 0 => {
                    tracing::debug!("retrying a request after a transient failure: {error}");
                    attempt += 1;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(Failure::Transient(error) | Failure::Final(error)) => return Err(error),
                Ok(reply) => return Ok(reply),
            }
        }
    }

    /// [`Http::api`]'s reply read as JSON; an empty reply reads as null.
    pub async fn api_json(&self, call: ApiCall<'_>) -> Result<serde_json::Value, SourceError> {
        let reply = self.api(call).await?;
        if reply.bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(&reply.bytes)
            .map_err(|_| SourceError::InvalidPlaylist("the server did not return valid data".into()))
    }

    /// A request whose reply is read whatever its status, with its headers,
    /// for APIs that answer in status codes (Trakt says "still waiting" with
    /// a 400 and "slow down" with a 429 and a Retry-After). Only failing to
    /// reach the server is an error. Reads (GET) are retried once after a
    /// dropped connection; writes aren't, since they may already have happened.
    pub async fn exchange(&self, call: ApiCall<'_>) -> Result<RawReply, SourceError> {
        let retry = call.method == reqwest::Method::GET;
        let mut attempt = 0;
        loop {
            let mut request = self.client.request(call.method.clone(), call.url);
            for (name, value) in call.headers {
                request = request.header(name.as_str(), value.as_str());
            }
            if let Some(body) = call.json {
                request = request.json(body);
            }
            let result = async {
                let response = tokio::time::timeout(IDLE_TIMEOUT, request.send())
                    .await
                    .map_err(|_| Failure::Transient(timed_out()))?
                    .map_err(|error| classify(&error))?;
                let status = response.status().as_u16();
                let headers = response.headers().clone();
                let bytes = read_body(response).await?;
                Ok(RawReply { status, headers, bytes })
            }
            .await;
            match result {
                Err(Failure::Transient(error)) if retry && attempt == 0 => {
                    tracing::debug!("retrying a request after a transient failure: {error}");
                    attempt += 1;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(Failure::Transient(error) | Failure::Final(error)) => return Err(error),
                Ok(reply) => return Ok(reply),
            }
        }
    }

    async fn fetch(&self, link: &str, user_agent: Option<&str>) -> Result<Vec<u8>, Failure> {
        let response = self.send(link, user_agent).await?;
        read_body(response).await
    }

    async fn send(&self, link: &str, user_agent: Option<&str>) -> Result<reqwest::Response, Failure> {
        let mut request = self.client.get(link);
        if let Some(agent) = user_agent.filter(|agent| !agent.trim().is_empty()) {
            request = request.header(USER_AGENT, agent.trim());
        }
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response, Failure> {
        let response = tokio::time::timeout(IDLE_TIMEOUT, request.send())
            .await
            .map_err(|_| Failure::Transient(timed_out()))?
            .map_err(|error| classify(&error))?;
        match response.status() {
            status if status.is_success() => Ok(response),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Failure::Final(SourceError::AuthenticationFailed)),
            status => Err(Failure::Final(SourceError::Http(status.as_u16()))),
        }
    }
}

/// One request to a server's API, for [`Http::api`].
pub(crate) struct ApiCall<'a> {
    pub method: reqwest::Method,
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    pub json: Option<&'a serde_json::Value>,
}

impl<'a> ApiCall<'a> {
    pub fn get(url: &'a str, headers: &'a [(String, String)]) -> Self {
        Self { method: reqwest::Method::GET, url, headers, json: None }
    }

    pub fn post(url: &'a str, headers: &'a [(String, String)], json: Option<&'a serde_json::Value>) -> Self {
        Self { method: reqwest::Method::POST, url, headers, json }
    }

    pub fn put(url: &'a str, headers: &'a [(String, String)]) -> Self {
        Self { method: reqwest::Method::PUT, url, headers, json: None }
    }

    pub fn delete(url: &'a str, headers: &'a [(String, String)]) -> Self {
        Self { method: reqwest::Method::DELETE, url, headers, json: None }
    }
}

/// What [`Http::api`] got back.
pub(crate) struct ApiReply {
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

/// What [`Http::exchange`] got back, whatever the status.
pub(crate) struct RawReply {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub bytes: Vec<u8>,
}

impl RawReply {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body as JSON; an empty or unreadable body reads as null.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.bytes).unwrap_or(serde_json::Value::Null)
    }

    /// A header's value as text.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

/// A reply's whole body, within the size cap and the idle timeout.
async fn read_body(response: reqwest::Response) -> Result<Vec<u8>, Failure> {
    if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
        return Err(Failure::Final(too_large_reply()));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    loop {
        let next = tokio::time::timeout(IDLE_TIMEOUT, stream.next())
            .await
            .map_err(|_| Failure::Transient(timed_out()))?;
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|error| classify(&error))?;
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(Failure::Final(too_large_reply()));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// A downloaded file: a temporary copy, or the user's own file left in place.
pub(crate) enum Downloaded {
    Temporary(tempfile::NamedTempFile),
    Local(PathBuf),
}

impl Downloaded {
    pub fn path(&self) -> &std::path::Path {
        match self {
            Self::Temporary(file) => file.path(),
            Self::Local(path) => path,
        }
    }
}

enum Failure {
    /// Worth one more try: a timeout or a dropped connection.
    Transient(SourceError),
    Final(SourceError),
}

impl Failure {
    fn into_error(self) -> SourceError {
        match self {
            Self::Transient(error) | Self::Final(error) => error,
        }
    }
}

/// A path for `file://` links, which only ever come from the user's own settings.
fn local_path(link: &str) -> Option<PathBuf> {
    if !link.starts_with("file:") {
        return None;
    }
    url::Url::parse(link).ok()?.to_file_path().ok()
}

fn timed_out() -> SourceError {
    SourceError::Network("The server took too long to respond.".into())
}

fn too_large_reply() -> SourceError {
    SourceError::InvalidPlaylist(format!("the reply is larger than Xorvio can read (over {} MB)", MAX_RESPONSE_BYTES >> 20))
}

fn too_large_guide() -> SourceError {
    SourceError::InvalidGuide("the guide file is larger than 2 GB".into())
}

/// A plain-language description of a failed request. reqwest's own messages
/// include the address, which can hold a login, so they're never passed on.
fn classify(error: &reqwest::Error) -> Failure {
    let mut detail = String::new();
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        detail.push_str(&inner.to_string().to_lowercase());
        detail.push(' ');
        source = inner.source();
    }
    if error.is_timeout() {
        return Failure::Transient(timed_out());
    }
    if detail.contains("dns") || detail.contains("lookup") || detail.contains("resolve") {
        return Failure::Transient(SourceError::Network("The server address could not be found.".into()));
    }
    if detail.contains("certificate") || detail.contains("tls") || detail.contains("handshake") {
        return Failure::Final(SourceError::Network(
            "A secure connection to the server could not be established.".into(),
        ));
    }
    if error.is_connect() {
        return Failure::Transient(SourceError::Network(
            "The server is not accepting connections. It may be offline.".into(),
        ));
    }
    if error.is_redirect() {
        return Failure::Final(SourceError::Network("The server redirected too many times.".into()));
    }
    if error.is_builder() {
        return Failure::Final(SourceError::Network("The address isn't valid.".into()));
    }
    if error.is_body() || error.is_decode() || error.is_request() {
        return Failure::Transient(SourceError::Network("The connection to the server was interrupted.".into()));
    }
    Failure::Final(SourceError::Network("The connection to the server failed.".into()))
}
