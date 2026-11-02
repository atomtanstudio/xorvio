//! Chromecast and Google TV for Xorvio: finding devices on the network
//! ([`Discovery`], mDNS `_googlecast._tcp`) and playing on one ([`Session`],
//! Cast v2 over TLS on port 8009) with Google's Default Media Receiver.
//!
//! Cast v2 is small: framed protobuf messages, each carrying JSON on a
//! namespace (connection, heartbeat, receiver, media). It's implemented
//! here rather than through a crate, since the ones about are synchronous
//! and leave out what the Default Media Receiver needs for HLS made of
//! fragmented MP4. What the device plays is an address the media gateway
//! serves to it; this crate never sees a provider's address.

mod connection;
mod discovery;
mod proto;
mod session;

pub use discovery::{Device, Discovery};
pub use session::{DEFAULT_MEDIA_RECEIVER, IdleReason, Media, PlayerState, Session, SessionState};

/// Why talking to a device didn't work. The interface says these in the
/// device's name ("Living Room TV didn't answer"), so they're kept short.
#[derive(Debug, thiserror::Error)]
pub enum CastError {
    #[error("the device couldn't be reached: {0}")]
    Unreachable(#[source] std::io::Error),
    #[error("the device didn't answer")]
    NoAnswer,
    #[error("the connection to the device closed")]
    Closed,
    #[error("the device couldn't open its media player ({0})")]
    LaunchFailed(String),
    #[error("the device couldn't play it{}", reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default())]
    LoadFailed { reason: Option<String> },
    #[error("the device refused ({0})")]
    Refused(String),
}

/// For the tests: the wire format, so a fake receiver can speak it.
#[doc(hidden)]
pub mod wire {
    pub use crate::proto::{CastMessage, Payload, read_message, write_message};
}
