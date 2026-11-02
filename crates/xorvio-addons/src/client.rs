//! Talks to one add-on over HTTP. An add-on's address can carry an account key
//! in its path, so the address is never logged, shown or put into an error;
//! errors name only the host.

use std::fmt;
use std::time::Duration;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;
use thiserror::Error;

use crate::protocol::{self, Manifest, Meta, Stream, Subtitle};

/// How long one add-on gets to answer one request.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Responses bigger than this aren't add-on JSON.
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// What went wrong talking to add-ons, in words for people. Never holds an
/// add-on's full address.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AddonError {
    #[error(
        "That isn't an add-on address. Paste the manifest link the add-on's settings page gives you."
    )]
    BadAddress,
    #[error("{0} couldn't be reached. Check your internet connection and try again.")]
    Unreachable(String),
    #[error("{0} took too long to answer. Try again in a moment.")]
    TimedOut(String),
    #[error("{0} doesn't have this.")]
    NotFound(String),
    #[error("{0} answered with an error ({1}). Try again later.")]
    Status(String, u16),
    #[error("{0} sent something Xorvio couldn't read.")]
    Unreadable(String),
    #[error("{0} offers no films, series, streams or subtitles, so Xorvio can't use it.")]
    NothingToUse(String),
    #[error("That add-on isn't installed.")]
    NotInstalled,
    #[error("That add-on is turned off. Turn it on in Settings › Add-ons.")]
    Disabled,
    #[error("None of your add-ons has {0} for this. Add one in Settings › Add-ons.")]
    NoAddonFor(&'static str),
    /// An add-on's own explanation in place of streams ("Invalid API key").
    #[error("{0}")]
    Notice(String),
}

/// JavaScript's `encodeURIComponent`, which is how Stremio's own clients
/// encode path parts.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

pub(crate) fn encode(text: &str) -> String {
    utf8_percent_encode(text, COMPONENT).to_string()
}

#[derive(Clone)]
pub(crate) struct AddonClient {
    /// The address without "/manifest.json", exactly as given (its
    /// percent-encoding is kept, since add-ons read their settings from it).
    base: String,
    host: String,
    http: reqwest::Client,
}

/// Prints the host only: the address can hold a key.
impl fmt::Debug for AddonClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AddonClient")
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

impl AddonClient {
    pub fn new(address: &str, http: reqwest::Client) -> Result<Self, AddonError> {
        let base = Self::base_from(address).ok_or(AddonError::BadAddress)?;
        let host = Self::host_of(&base).ok_or(AddonError::BadAddress)?;
        Ok(Self { base, host, http })
    }

    /// Accepts a manifest link as add-ons hand it out (https:// or stremio://,
    /// with or without "/manifest.json") and returns the address requests build on.
    pub fn base_from(address: &str) -> Option<String> {
        let mut text = address.trim().to_string();
        if text
            .get(..10)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("stremio://"))
        {
            text = format!("https://{}", &text[10..]);
        }
        if let Some(cut) = text.find(['?', '#']) {
            text.truncate(cut);
        }
        if let Some(stripped) = text.strip_suffix("/manifest.json") {
            text = stripped.to_string();
        }
        while text.ends_with('/') {
            text.pop();
        }
        Self::host_of(&text)?;
        Some(text)
    }

    fn host_of(base: &str) -> Option<String> {
        let url = url::Url::parse(&format!("{base}/manifest.json")).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let host = url.host_str().filter(|h| !h.is_empty())?;
        Some(match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        })
    }

    /// The address to keep (in the secret store only).
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Where it's hosted, safe to show.
    pub fn host(&self) -> &str {
        &self.host
    }

    pub async fn manifest(&self) -> Result<Manifest, AddonError> {
        let value = self.get("/manifest.json").await?;
        Manifest::from_json(&value).ok_or_else(|| AddonError::Unreadable(self.host.clone()))
    }

    /// A catalog's titles, optionally searched or narrowed (`extra` such as
    /// `[("search", "sopranos")]`).
    pub async fn catalog(
        &self,
        media_type: &str,
        id: &str,
        extra: &[(&str, String)],
    ) -> Result<Vec<Meta>, AddonError> {
        let mut path = format!("/catalog/{}/{}", encode(media_type), encode(id));
        if !extra.is_empty() {
            let pairs: Vec<String> = extra
                .iter()
                .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                .collect();
            path.push('/');
            path.push_str(&pairs.join("&"));
        }
        path.push_str(".json");
        Ok(protocol::metas(&self.get(&path).await?))
    }

    pub async fn meta(&self, media_type: &str, id: &str) -> Result<Option<Meta>, AddonError> {
        let path = format!("/meta/{}/{}.json", encode(media_type), encode(id));
        Ok(protocol::meta(&self.get(&path).await?))
    }

    /// `id` is a film's id ("tt0078788") or an episode's ("tt0141842:6:21").
    pub async fn streams(&self, media_type: &str, id: &str) -> Result<Vec<Stream>, AddonError> {
        let path = format!("/stream/{}/{}.json", encode(media_type), encode(id));
        Ok(protocol::streams(&self.get(&path).await?))
    }

    pub async fn subtitles(&self, media_type: &str, id: &str) -> Result<Vec<Subtitle>, AddonError> {
        let path = format!("/subtitles/{}/{}.json", encode(media_type), encode(id));
        Ok(protocol::subtitles(&self.get(&path).await?))
    }

    async fn get(&self, path: &str) -> Result<Value, AddonError> {
        let host = || self.host.clone();
        let url = url::Url::parse(&format!("{}{}", self.base, path))
            .map_err(|_| AddonError::BadAddress)?;
        let response = self
            .http
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| self.transport_error(error))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(AddonError::NotFound(host()));
        }
        if !status.is_success() {
            return Err(AddonError::Status(host(), status.as_u16()));
        }
        let body = read_capped(response, MAX_RESPONSE_BYTES)
            .await
            .map_err(|error| self.transport_error(error))?
            .ok_or_else(|| AddonError::Unreadable(host()))?;
        serde_json::from_slice(&body).map_err(|_| AddonError::Unreadable(host()))
    }

    fn transport_error(&self, error: reqwest::Error) -> AddonError {
        let timed_out = error.is_timeout();
        // `without_url` keeps the address (and any key in it) out of the log.
        tracing::debug!(host = %self.host, error = %error.without_url(), "add-on request failed");
        if timed_out {
            AddonError::TimedOut(self.host.clone())
        } else {
            AddonError::Unreachable(self.host.clone())
        }
    }
}

/// A reply's body, or None once it passes `limit`. It's read as it arrives
/// and stops there, because a compressed reply's length says nothing about
/// what it unpacks to: a few kilobytes can unpack to gigabytes.
pub(crate) async fn read_capped(
    response: reqwest::Response,
    limit: usize,
) -> Result<Option<Vec<u8>>, reqwest::Error> {
    use futures_util::StreamExt;
    if response.content_length().is_some_and(|len| len > limit as u64) {
        return Ok(None);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len() + chunk.len() > limit {
            return Ok(None);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_manifest_links() {
        let base = |s: &str| AddonClient::base_from(s);
        assert_eq!(
            base("https://a.example/key=1/manifest.json").as_deref(),
            Some("https://a.example/key=1")
        );
        assert_eq!(
            base(" stremio://a.example/x/manifest.json?y=1 ").as_deref(),
            Some("https://a.example/x")
        );
        assert_eq!(
            base("http://127.0.0.1:7000/").as_deref(),
            Some("http://127.0.0.1:7000")
        );
        assert_eq!(base("ftp://a.example/manifest.json"), None);
        assert_eq!(base("not a link"), None);
    }

    #[test]
    fn host_hides_the_path() {
        let client = AddonClient::new(
            "https://a.example/secret-key-123/manifest.json",
            reqwest::Client::new(),
        )
        .unwrap();
        assert_eq!(client.host(), "a.example");
        assert!(!format!("{client:?}").contains("secret"));
    }

    #[test]
    fn encodes_like_javascript() {
        assert_eq!(encode("tt1:2:3"), "tt1%3A2%3A3");
        assert_eq!(encode("a b/c(d)!"), "a%20b%2Fc(d)!");
    }
}
