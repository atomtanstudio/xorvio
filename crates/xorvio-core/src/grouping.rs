//! Similar channels as one row. Providers list one channel several times
//! ("News 24 HD", "News 24 FHD", "US: News 24", "News 24 (backup)"); Live TV
//! can show them as one row, under the name they share, that plays the best
//! version or the one someone chose.
//!
//! Grouping is only a way of listing: favourites, groups, history,
//! recordings and every other choice keep pointing at real channel ids. It
//! happens here rather than in the interface because channel up and down in
//! the player walks the same `channels` list, and the version someone chose
//! is kept in the library by the group's key.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use xorvio_model::{Channel, ChannelVersion};

use crate::edits::Edits;
use crate::names::{self, VersionName};
use crate::{Core, Result};

/// The group a channel belongs to: versions of one name within its category
/// (or, without one, its source). Starts with the source id, so removing a
/// source can find its choices.
pub(crate) fn group_key(channel: &Channel, version: &VersionName) -> String {
    let scope = channel.category_id.as_deref().unwrap_or(&channel.source_id);
    format!("{scope}|{}", version.key)
}

/// The version that plays when no one chose: best quality, then a main feed
/// over a backup, then the first listed.
fn best_version(versions: &[(Channel, VersionName)]) -> usize {
    let mut best = 0;
    for (index, (_, version)) in versions.iter().enumerate().skip(1) {
        let (_, leader) = &versions[best];
        if (version.quality, !version.backup) > (leader.quality, !leader.backup) {
            best = index;
        }
    }
    best
}

impl Core {
    pub(crate) fn groups_similar_channels(&self) -> bool {
        self.group_similar.load(Ordering::Relaxed)
    }

    /// Always plays this channel for its group of similar channels, or goes
    /// back to the best version (`preferred` false).
    pub fn set_preferred_version(&self, channel_id: &str, preferred: bool) -> Result<()> {
        let channel = self.catalog.channel(channel_id)?.ok_or_else(|| anyhow::anyhow!("This channel is no longer available."))?;
        let key = group_key(&channel, &names::version_name(&channel.name));
        self.library.set_preferred_version(&key, preferred.then_some(channel_id))?;
        self.edits_changed();
        Ok(())
    }

    /// Channels as listed, each group of similar ones folded into one row in
    /// the place of its first. A row stands for its versions and is the one
    /// that plays, named as the versions are (unless someone renamed it).
    /// Channels listed once are presented as they are.
    pub(crate) fn group_similar(&self, channels: Vec<Channel>, edits: &Edits) -> Vec<Channel> {
        let mut rows: Vec<(String, Vec<(Channel, VersionName)>)> = Vec::new();
        let mut row_of: HashMap<String, usize> = HashMap::with_capacity(channels.len());
        for channel in channels {
            let version = names::version_name(&channel.name);
            let key = group_key(&channel, &version);
            match row_of.get(&key) {
                Some(&row) => rows[row].1.push((channel, version)),
                None => {
                    row_of.insert(key.clone(), rows.len());
                    rows.push((key, vec![(channel, version)]));
                }
            }
        }

        rows.into_iter()
            .map(|(key, mut versions)| {
                if versions.len() == 1 {
                    let (channel, _) = versions.pop().expect("one version");
                    return self.present_channel_with(edits, channel);
                }
                let chosen = edits.preferred_version(&key).and_then(|id| versions.iter().position(|(channel, _)| channel.id == id));
                let playing = chosen.unwrap_or_else(|| best_version(&versions));
                let listed = versions
                    .iter()
                    .enumerate()
                    .map(|(index, (channel, version))| ChannelVersion {
                        id: channel.id.clone(),
                        name: edits.name(&channel.id).map_or_else(|| channel.name.trim().to_string(), str::to_string),
                        quality: version.quality.label().map(str::to_string),
                        preferred: chosen == Some(index),
                    })
                    .collect();
                let (channel, version) = versions.swap_remove(playing);
                let mut row = self.present_channel_with(edits, channel);
                if edits.name(&row.id).is_none() {
                    row.name = version.name;
                }
                row.versions = listed;
                row
            })
            .collect()
    }
}
