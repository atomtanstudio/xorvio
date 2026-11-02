//! Guides beyond the ones sources bring: XMLTV guides added on their own,
//! guide channels chosen by hand for channels the guides miss or get wrong,
//! and finding programmes to jump to in the guide.
//!
//! Every channel shows one guide channel. The catalog works out which each
//! time a guide or catalog loads (see `resolve_guide_links`); what's chosen
//! here is kept in the library, so it survives every refresh.

use std::collections::HashMap;

use xorvio_model::{GuideChannel, GuideMatch, GuideSource, GuideSourceSummary, NewGuideSource, ProgramHit, SourceSecrets, Timestamp};

use crate::store::GuideOverride;
use crate::store::catalog::{GuideChannelRow, GuideLinks};
use crate::store::lock;
use crate::{Core, Result, is_usable_link, now, redact, secrets};

/// Offsets beyond a day are typing mistakes, not time zones.
const MAX_OFFSET_MINUTES: i64 = 24 * 60;

const NOT_A_GUIDE_LINK: &str = "That isn't a guide link. Paste the guide's whole address, starting with http:// or https://.";

impl Core {
    // MARK: Added guides

    /// Added guides in the user's order, with how each last loaded.
    pub fn guide_sources(&self) -> Result<Vec<GuideSourceSummary>> {
        let statuses = self.catalog.statuses()?;
        let refreshing = lock(&self.refreshing).clone();
        Ok(self
            .library
            .guide_sources()?
            .into_iter()
            .map(|guide| GuideSourceSummary {
                status: statuses.get(&guide.id).cloned().unwrap_or_default(),
                refreshing: refreshing.contains(&guide.id),
                guide,
            })
            .collect())
    }

    /// Saves a new guide and its link. Its listings load with a separate
    /// [`Core::refresh_guide_source`], as a source's catalog does.
    pub fn add_guide_source(&self, new: NewGuideSource) -> Result<GuideSource> {
        let link = new.url.trim().to_string();
        if !is_usable_link(&link) {
            anyhow::bail!(NOT_A_GUIDE_LINK);
        }
        let display_host = redact::host_description(&link);
        let name = match new.name.trim() {
            "" => display_host.clone().unwrap_or_else(|| "Guide".into()),
            name => name.to_string(),
        };
        let existing = self.library.guide_sources()?;
        let guide = GuideSource {
            id: format!("guide-{}", uuid::Uuid::new_v4()),
            name,
            display_host,
            offset_minutes: new.offset_minutes.clamp(-MAX_OFFSET_MINUTES, MAX_OFFSET_MINUTES),
            created_at: now(),
            sort_index: existing.iter().map(|guide| guide.sort_index + 1).max().unwrap_or(0),
        };
        secrets::save(self.secrets.as_ref(), &guide.id, &SourceSecrets { epg_url: Some(link), ..Default::default() })?;
        self.library.save_guide_source(&guide)?;
        Ok(guide)
    }

    /// Saves an added guide's name and offset, and its link when given. The id,
    /// creation time and the host shown stay the backend's own. A new offset
    /// applies once the guide loads again.
    pub fn update_guide_source(&self, guide: GuideSource, url: Option<String>) -> Result<()> {
        let existing = self.guide_source(&guide.id)?;
        let mut updated = GuideSource {
            id: existing.id.clone(),
            name: match guide.name.trim() {
                "" => existing.name.clone(),
                name => name.to_string(),
            },
            display_host: existing.display_host.clone(),
            offset_minutes: guide.offset_minutes.clamp(-MAX_OFFSET_MINUTES, MAX_OFFSET_MINUTES),
            created_at: existing.created_at,
            sort_index: guide.sort_index,
        };
        if let Some(link) = url.map(|link| link.trim().to_string()).filter(|link| !link.is_empty()) {
            if !is_usable_link(&link) {
                anyhow::bail!(NOT_A_GUIDE_LINK);
            }
            updated.display_host = redact::host_description(&link);
            secrets::save(self.secrets.as_ref(), &existing.id, &SourceSecrets { epg_url: Some(link), ..Default::default() })?;
        }
        self.library.save_guide_source(&updated)
    }

    /// Removes an added guide with its link, its listings and the guide
    /// channels chosen from it; channels it served go back to other guides.
    pub fn remove_guide_source(&self, id: &str) -> Result<()> {
        self.guide_source(id)?;
        self.library.remove_guide_source(id)?;
        self.secrets.delete(id)?;
        self.catalog.remove_source(id)?;
        self.catalog.relink_guides(None, &self.guide_links()?)
    }

    /// An added guide's link, for editing it. It can carry a login, so it's
    /// never shown anywhere else.
    pub fn guide_source_link(&self, id: &str) -> Result<Option<String>> {
        self.guide_source(id)?;
        Ok(self.load_secrets(id)?.epg_url)
    }

    pub(crate) fn guide_source(&self, id: &str) -> Result<GuideSource> {
        self.library
            .guide_sources()?
            .into_iter()
            .find(|guide| guide.id == id)
            .ok_or_else(|| anyhow::anyhow!("This guide no longer exists."))
    }

    /// What the catalog needs to link channels to guides.
    pub(crate) fn guide_links(&self) -> Result<GuideLinks> {
        let guides: Vec<String> = self.library.guide_sources()?.into_iter().map(|guide| guide.id).collect();
        // A choice synced from another computer can name a guide added only
        // there; the channel finds its own guide here instead.
        let sources = self.configs();
        let overrides = self
            .library
            .guide_overrides()?
            .into_iter()
            .filter(|choice| guides.contains(&choice.guide_id) || sources.iter().any(|config| config.id == choice.guide_id))
            .collect();
        Ok(GuideLinks { guides, overrides })
    }

    // MARK: Matching channels by hand

    /// Channels in every guide (active sources' and added ones) whose id or
    /// name contains `text`, closest first, with what each shows now.
    pub fn guide_channels(&self, text: &str, limit: usize) -> Result<Vec<GuideChannel>> {
        let names = self.guide_names()?;
        let guides: Vec<String> = self
            .active_source_ids()
            .into_iter()
            .chain(self.library.guide_sources()?.into_iter().map(|guide| guide.id))
            .collect();
        let rows = self.catalog.guide_channels(text, &guides, now(), limit)?;
        Ok(rows.into_iter().map(|row| guide_channel(row, &names)).collect())
    }

    /// The guide channel a channel shows, and whether it was chosen by hand.
    pub fn guide_match(&self, channel_id: &str) -> Result<GuideMatch> {
        let chosen = self.library.guide_overrides()?.iter().any(|choice| choice.channel_id == channel_id);
        let current = match self.catalog.guide_link(channel_id)? {
            Some((guide_id, epg_id)) if chosen || self.catalog.guide_has_channel(&guide_id, &epg_id)? => {
                Some(guide_channel(self.catalog.guide_channel(&guide_id, &epg_id, now())?, &self.guide_names()?))
            }
            _ => None,
        };
        Ok(GuideMatch { channel_id: channel_id.to_string(), current, chosen })
    }

    /// Has a channel show the chosen guide channel, whatever its own guide id
    /// or name says, through every refresh.
    pub fn match_guide(&self, channel_id: &str, guide_id: &str, epg_id: &str) -> Result<()> {
        let channel = self.catalog.channel(channel_id)?.ok_or_else(|| anyhow::anyhow!("That channel is no longer in your sources."))?;
        let choice = GuideOverride { channel_id: channel.id, guide_id: guide_id.to_string(), epg_id: epg_id.to_string() };
        self.library.set_guide_override(&choice, now())?;
        self.catalog.relink_guides(Some(&channel.source_id), &self.guide_links()?)
    }

    /// Lets a channel find its guide channel by itself again.
    pub fn reset_guide_match(&self, channel_id: &str) -> Result<()> {
        self.library.remove_guide_override(channel_id)?;
        if let Some(channel) = self.catalog.channel(channel_id)? {
            self.catalog.relink_guides(Some(&channel.source_id), &self.guide_links()?)?;
        }
        Ok(())
    }

    /// Source and added guide names by id.
    fn guide_names(&self) -> Result<HashMap<String, String>> {
        let mut names: HashMap<String, String> = self.configs().into_iter().map(|config| (config.id, config.name)).collect();
        names.extend(self.library.guide_sources()?.into_iter().map(|guide| (guide.id, guide.name)));
        Ok(names)
    }

    // MARK: Finding programmes

    /// Programmes whose title contains `text` that end after `from`, for
    /// jumping the guide to them: what's still to come first, soonest first,
    /// then what has aired, latest first.
    pub fn search_guide(&self, text: &str, from: Timestamp, limit: usize) -> Result<Vec<ProgramHit>> {
        // Only channels Live TV lists: not hidden, locked or left out by the profile.
        let excluded = self.excluded(false)?;
        let found = self.catalog.search_programs(text, &self.active_source_ids(), from, now(), limit, &excluded.ids())?;
        Ok(found
            .into_iter()
            .filter(|(_, channel)| !excluded.channel(channel))
            .map(|(program, channel)| ProgramHit { program, channel: self.present_channel(channel) })
            .collect())
    }

    /// About when the loaded listings begin and end, for the guide's day list.
    pub fn guide_span(&self) -> Result<Option<(Timestamp, Timestamp)>> {
        self.catalog.guide_span()
    }

    // MARK: Space

    /// How many guide listings the catalog holds, for Settings › Storage.
    pub fn guide_listing_count(&self) -> Result<i64> {
        self.catalog.program_count()
    }

    /// Deletes every guide listing and gives their space back to the disk.
    /// Channels keep their guides; the next refresh of each source and added
    /// guide loads its listings again. Compacting a big catalog takes a
    /// while, so call this off the main thread.
    pub fn clear_guide_listings(&self) -> Result<()> {
        // A guide import replaces its listings in one long transaction, which
        // would hold this up and then bring them straight back.
        if !lock(&self.refreshing).is_empty() {
            anyhow::bail!("A source or guide is refreshing. Try again once it's finished.");
        }
        self.catalog.clear_guide()?;
        for (id, mut status) in self.catalog.statuses()? {
            if status.program_count != 0 {
                status.program_count = 0;
                self.catalog.save_status(&id, &status)?;
            }
        }
        self.catalog.compact()
    }
}

fn guide_channel(row: GuideChannelRow, names: &HashMap<String, String>) -> GuideChannel {
    GuideChannel {
        guide_name: names.get(&row.guide_id).cloned().unwrap_or_else(|| "Guide".into()),
        guide_id: row.guide_id,
        epg_id: row.epg_id,
        name: row.name,
        now_title: row.now_title,
    }
}
