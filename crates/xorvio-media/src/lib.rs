//! Media for Xorvio: the local gateway every stream plays through, and the
//! recorder.
//!
//! The web view can't play everything (MKV, AC-3/DTS audio, raw MPEG-TS) and
//! can't add a provider's headers, so the [`Gateway`] serves every stream from
//! `127.0.0.1`: HLS with rewritten playlists, MPEG-TS and web-ready files
//! passed through (ranges included), and anything else remuxed by ffmpeg into
//! MPEG-TS for mpegts.js. Live channels can also play from a buffer ffmpeg
//! writes on this computer, so they can be paused and rewound
//! ([`Gateway::timeshift`]). The [`Recorder`] records live channels with ffmpeg on a
//! schedule, reconnecting through drops, schedules series from the guide, and
//! saves programmes that already aired from the provider's catch-up, then
//! finds the ad breaks in what it recorded. Films, episodes and recordings
//! get thumbnails for the seek bar ([`Gateway::thumbnails`]).
//!
//! Stream addresses can hold logins, so they never appear in a gateway
//! address, a log line or an error.

mod adbreaks;
mod facts;
mod gateway;
mod hls;
mod probe;
mod process;
mod recorder;
mod remux;
mod scratch;
mod stills;
mod timeshift;
mod tools;

pub use adbreaks::{AdBreak, AdBreakState, AdBreaks};
pub use facts::file_facts;
pub use gateway::cast::CastMedia;
pub use gateway::thumbnails::{Allowance, ConnectionCheck, ThumbnailSheet, Thumbnails, second_connection};
pub use gateway::{Gateway, PlayHint, PlaybackSupport, StreamOptions};
pub use recorder::{
    Airing, ChannelResolver, NewSeriesRule, Recorder, RecordingFolders, SeriesGuide,
};
pub use scratch::temp_root;
pub use stills::{STILL_SCHEME, Stills, still_url};
pub use tools::Tools;
