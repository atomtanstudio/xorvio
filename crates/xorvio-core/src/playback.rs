//! Making the real address to play. Logins are applied only here, at the
//! moment of playback, and the result never goes to the interface or a log.

use xorvio_model::{SourceKind, StreamLocator, StreamTarget, Timestamp, XtreamPath};

use crate::error::SourceError;
use crate::sources::stream;
use crate::sources::xtream::XtreamCredentials;
use crate::{Core, Result, now};

/// The furthest a channel's replays can be moved, in minutes either way.
const MAX_CATCHUP_OFFSET: i64 = 30;

impl Core {
    /// The address and headers to open a stream with.
    pub fn stream_target(&self, source_id: &str, locator: &StreamLocator, is_live: bool) -> Result<StreamTarget> {
        let config = self.config(source_id)?;
        match locator {
            StreamLocator::Url { url } => self.url_target(source_id, url, config.user_agent.clone(), is_live),
            StreamLocator::Xtream { path, stream_id, extension } => {
                let credentials = self.credentials(source_id)?;
                // Live channels follow the source's current format choice.
                let extension = match path {
                    XtreamPath::Live => config.live_format.extension().to_string(),
                    _ => extension.clone(),
                };
                Ok(StreamTarget {
                    url: credentials.stream_url(*path, stream_id, &extension),
                    user_agent: config.user_agent,
                    headers: Vec::new(),
                    is_live,
                })
            }
            StreamLocator::File { path } => {
                // A media folder's drive unplugged or share not connected: said plainly, rather than a player error.
                if !std::path::Path::new(path).is_file() {
                    return Err(SourceError::Unavailable(
                        "This file isn't available right now. If it's on a drive or a network share, connect it and try again.".into(),
                    )
                    .into());
                }
                let url = url::Url::from_file_path(path)
                    .map_err(|()| anyhow::anyhow!("This file's location isn't valid."))?
                    .to_string();
                Ok(StreamTarget { url, user_agent: None, headers: Vec::new(), is_live })
            }
            StreamLocator::Addon { .. } => {
                anyhow::bail!("Add-on titles are played through the add-on that lists them.")
            }
            StreamLocator::Server { key, part, .. } => {
                let session = self.server_session(source_id)?;
                Ok(crate::sources::server::stream_target(&session, key, part.as_deref())?)
            }
        }
    }

    /// The provider's replay of a past programme on a channel. Only channels
    /// that advertise replay, and only within their window: a guide listing
    /// alone never means a programme can be replayed.
    ///
    /// The channel's catch-up offset moves the start and end asked for (the
    /// length stays), so a provider whose replays begin early or late
    /// delivers the programme from its own start, which is where the player's
    /// timeline begins.
    pub fn replay_target(&self, channel_id: &str, start: Timestamp, end: Timestamp) -> Result<StreamTarget> {
        let channel = self.catalog.channel(channel_id)?.ok_or_else(|| anyhow::anyhow!("This channel is no longer available."))?;
        if !stream::can_replay(&channel, start, now()) {
            anyhow::bail!("This programme can't be replayed: the channel doesn't keep recordings from that far back.");
        }
        let shift = self.load_edits()?.catchup_offset(channel_id) * 60;
        let (start, end) = (start + shift, end + shift);
        let config = self.config(&channel.source_id)?;
        match &channel.stream {
            StreamLocator::Xtream { stream_id, .. } if config.kind == SourceKind::Xtream => {
                let credentials = self.credentials(&channel.source_id)?;
                let zone = self.catalog.status(&channel.source_id)?.and_then(|status| status.server_time_zone);
                let url = credentials.replay_url(
                    stream_id,
                    start,
                    end - start,
                    config.live_format.extension(),
                    zone.as_deref(),
                );
                Ok(StreamTarget { url, user_agent: config.user_agent, headers: Vec::new(), is_live: false })
            }
            StreamLocator::Url { url } => {
                let template = channel
                    .catchup_source
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("This channel doesn't say how to replay its programmes."))?;
                let replay = stream::playlist_replay_url(url, template, start, end, now());
                // The live link's player options apply to its replays too.
                let mut target = self.url_target(&channel.source_id, url, config.user_agent, false)?;
                target.url = replay;
                Ok(target)
            }
            _ => anyhow::bail!("This channel can't be replayed."),
        }
    }

    /// Moves a channel's replays by `minutes` (−30…30), for providers whose
    /// replays start early (below zero) or late; 0 takes the offset away.
    pub fn set_catchup_offset(&self, channel_id: &str, minutes: i64) -> Result<()> {
        if crate::edits::edit_kind(channel_id) != Some(xorvio_model::EditKind::Channel) {
            anyhow::bail!("Only channels have a catch-up offset.");
        }
        self.library.set_catchup_offset(channel_id, minutes.clamp(-MAX_CATCHUP_OFFSET, MAX_CATCHUP_OFFSET))?;
        self.edits_changed();
        Ok(())
    }

    /// A playlist link with the headers its playlist asked for.
    fn url_target(&self, source_id: &str, url: &str, source_agent: Option<String>, is_live: bool) -> Result<StreamTarget> {
        let (agent, referrer) = self.catalog.stream_options(source_id, url)?.unwrap_or_default();
        let headers = referrer.map(|referrer| vec![("Referer".to_string(), referrer)]).unwrap_or_default();
        Ok(StreamTarget { url: url.to_string(), user_agent: agent.or(source_agent), headers, is_live })
    }

    pub(crate) fn credentials(&self, source_id: &str) -> Result<XtreamCredentials> {
        let secrets = self.load_secrets(source_id)?;
        XtreamCredentials::from_secrets(&secrets).ok_or_else(|| {
            SourceError::MissingConfiguration("a server address, username and password".into()).into()
        })
    }
}
