//! Errors a source can run into, worded for the people who'll read them.
//!
//! Messages never contain a URL, a username or a password: providers put
//! logins in their links, and these strings end up in the interface and in
//! logs. Anything that came from outside goes through [`crate::redact`] first.

/// Why a playlist, provider or guide couldn't be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    #[error("This source is missing {0}. Edit the source to add it.")]
    MissingConfiguration(String),
    #[error("The provider rejected the username or password.")]
    AuthenticationFailed,
    #[error("The provider reports that this account has expired.")]
    AccountExpired,
    /// A media server no longer takes the token Xorvio signed in with.
    #[error("The server has signed Xorvio out. Edit this source to sign in again.")]
    SignedOut,
    #[error("The server responded with HTTP {0}.")]
    Http(u16),
    /// A plain-language description of a connection problem.
    #[error("{0}")]
    Network(String),
    #[error("The playlist could not be read: {0}")]
    InvalidPlaylist(String),
    #[error("The guide could not be read: {0}")]
    InvalidGuide(String),
    #[error("The playlist file could not be read. Choose the file again.")]
    FileUnavailable,
    #[error("The refresh was cancelled.")]
    Cancelled,
    /// A media folder (or the drive or share it's on) that can't be reached
    /// right now, said plainly.
    #[error("{0}")]
    Unavailable(String),
    /// Something this version of Xorvio can't do yet, said plainly.
    #[error("{0}")]
    Unsupported(String),
}

impl SourceError {
    /// The provider turned the login down, as opposed to being unreachable.
    pub fn is_authentication(&self) -> bool {
        matches!(self, Self::AuthenticationFailed | Self::AccountExpired | Self::SignedOut)
    }
}

/// The message to show for any error, with a [`SourceError`]'s own wording
/// when there is one somewhere in the chain.
pub(crate) fn describe(error: &anyhow::Error) -> String {
    match error.downcast_ref::<SourceError>() {
        Some(source) => source.to_string(),
        None => error.to_string(),
    }
}
