//! The watchlist, favorite channels and named channel lists, playback
//! progress and recently watched channels: things people make themselves,
//! kept in `library.sqlite`.

use std::collections::HashMap;

use xorvio_model::{Channel, FavoriteList, HistoryEntry, MediaKind, Movie, PlaybackProgress, Series, WatchlistEntry};

use crate::continue_watching::episode_subtitle;
use crate::sources::ids;
use crate::store::library::DEFAULT_LIST;
use crate::watchlist::{self, SOON_SECONDS};
use crate::{Core, Result, names, now};

impl Core {
    /// The watchlist in Home's order: what's on now, programmes starting
    /// soon, films and series (newest first), then later programmes.
    /// Programmes that have finished airing leave the watchlist.
    pub fn watchlist(&self) -> Result<Vec<WatchlistEntry>> {
        let now = now();
        self.library.remove_ended_programs(now)?;
        Ok(watchlist::ordered(&self.library.watchlist()?, now, SOON_SECONDS))
    }

    /// The watchlist as the Watchlist screen and Home show it: as
    /// [`Core::watchlist`], less films, series and programmes that are
    /// hidden, locked or left out by the profile. Entries the catalog no
    /// longer has (add-on titles among them) stay.
    pub fn watchlist_shown(&self) -> Result<Vec<WatchlistEntry>> {
        let entries = self.watchlist()?;
        let excluded = self.excluded(false)?;
        let ids_of = |kind: MediaKind| -> Vec<String> {
            entries.iter().filter(|entry| entry.kind == kind).map(|entry| entry.id.clone()).collect()
        };
        let movies: HashMap<String, Movie> =
            self.catalog.movies_by_ids(&ids_of(MediaKind::Movie))?.into_iter().map(|movie| (movie.id.clone(), movie)).collect();
        let series: HashMap<String, Series> =
            self.catalog.series_by_ids(&ids_of(MediaKind::Series))?.into_iter().map(|show| (show.id.clone(), show)).collect();
        let channel_ids: Vec<String> = entries.iter().filter_map(|entry| entry.channel_id.clone()).collect();
        let channels: HashMap<String, Channel> =
            self.catalog.channels_by_ids(&channel_ids)?.into_iter().map(|channel| (channel.id.clone(), channel)).collect();
        Ok(entries
            .into_iter()
            .filter(|entry| match entry.kind {
                MediaKind::Movie => movies.get(&entry.id).is_none_or(|movie| !excluded.movie(movie)),
                MediaKind::Series => series.get(&entry.id).is_none_or(|show| !excluded.series(show)),
                _ => entry.channel_id.as_ref().and_then(|id| channels.get(id)).is_none_or(|channel| !excluded.channel(channel)),
            })
            .collect())
    }

    /// Saves or removes an entry. Saving one that's already there updates its
    /// details and keeps when it was first saved.
    /// Films and series also go to (or leave) the Trakt watchlist when that's on.
    pub fn set_watchlisted(&self, mut entry: WatchlistEntry, saved: bool) -> Result<()> {
        let was_saved = self.library.is_watchlisted(&entry.id)?;
        if !saved {
            self.library.remove_from_watchlist(&entry.id)?;
            if was_saved {
                self.tell_trakt_watchlist(&entry, false);
            }
            return Ok(());
        }
        if entry.added_at <= 0 {
            entry.added_at = now();
        }
        self.library.add_to_watchlist(&entry)?;
        if !was_saved {
            self.tell_trakt_watchlist(&entry, true);
        }
        Ok(())
    }

    /// Favorite channels in the list's order (as added, unless rearranged).
    pub fn favorite_channel_ids(&self) -> Result<Vec<String>> {
        self.library.list_items(DEFAULT_LIST)
    }

    pub fn set_favorite(&self, channel_id: &str, favorite: bool) -> Result<()> {
        self.library.set_in_list(channel_id, MediaKind::Channel, DEFAULT_LIST, favorite, now())
    }

    // MARK: Channel lists

    /// Lists people named ("Sports", "News"), in the order they were made.
    pub fn favorite_lists(&self) -> Result<Vec<FavoriteList>> {
        self.library
            .lists()?
            .into_iter()
            .map(|(id, name)| Ok(FavoriteList { channel_ids: self.library.list_items(&id)?, id, name }))
            .collect()
    }

    /// Makes a named list, starting with a channel when one is given.
    pub fn create_favorite_list(&self, name: &str, channel_id: Option<&str>) -> Result<FavoriteList> {
        let name = self.list_name(name, None)?;
        let id = uuid::Uuid::new_v4().to_string();
        self.library.create_list(&id, &name, now())?;
        if let Some(channel_id) = channel_id {
            self.library.set_in_list(channel_id, MediaKind::Channel, &id, true, now())?;
        }
        Ok(FavoriteList { id, name, channel_ids: channel_id.map(str::to_string).into_iter().collect() })
    }

    pub fn rename_favorite_list(&self, id: &str, name: &str) -> Result<()> {
        let name = self.list_name(name, Some(id))?;
        if !self.library.rename_list(id, &name)? {
            anyhow::bail!("This group no longer exists.");
        }
        Ok(())
    }

    /// Deletes a named list. Its channels stay where they are.
    pub fn delete_favorite_list(&self, id: &str) -> Result<()> {
        if id == DEFAULT_LIST {
            anyhow::bail!("Favorites can't be deleted.");
        }
        self.library.delete_list(id)
    }

    /// Adds a channel to the end of a list (Favorites or a named one), or
    /// takes it out.
    pub fn set_in_favorite_list(&self, list_id: &str, channel_id: &str, member: bool) -> Result<()> {
        self.check_list(list_id)?;
        self.library.set_in_list(channel_id, MediaKind::Channel, list_id, member, now())
    }

    /// Puts a list's channels in a new order. Channels not named (hidden, or
    /// from a paused source) keep their places.
    pub fn reorder_favorite_list(&self, list_id: &str, channel_ids: &[String]) -> Result<()> {
        self.check_list(list_id)?;
        self.library.reorder_list(list_id, channel_ids)
    }

    /// A trimmed name no other list has (`renaming` aside).
    fn list_name(&self, name: &str, renaming: Option<&str>) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("A group needs a name.");
        }
        let taken = self.library.lists()?.into_iter().any(|(id, existing)| {
            Some(id.as_str()) != renaming && existing.to_lowercase() == name.to_lowercase()
        });
        if taken {
            anyhow::bail!("There's already a group called “{name}”.");
        }
        Ok(name.to_string())
    }

    fn check_list(&self, list_id: &str) -> Result<()> {
        if list_id == DEFAULT_LIST || self.library.lists()?.iter().any(|(id, _)| id == list_id) {
            Ok(())
        } else {
            anyhow::bail!("This group no longer exists.")
        }
    }

    /// Marks films or episodes watched (finished now) or unwatched (their
    /// progress forgotten), as a whole season or series at once. A media
    /// server's titles are marked on the server too, and Trakt's history
    /// follows when that's on.
    pub fn set_watched(&self, items: &[PlaybackProgress], watched: bool) -> Result<()> {
        let at = now();
        for item in items {
            if watched {
                self.library.save_progress(&PlaybackProgress { finished: true, updated_at: at, ..item.clone() })?;
            } else {
                self.library.remove_progress(&item.item_id)?;
            }
            self.send_watched_to_server(item, watched);
        }
        self.tell_trakt_watched(items, watched);
        if watched {
            // A whole season or series marked watched follows it for new episodes.
            let mut series: Vec<String> = items.iter().filter_map(|item| item.series_id.clone()).collect();
            series.sort();
            series.dedup();
            if let Err(error) = self.follow_caught_up(&series, at) {
                tracing::warn!("a series caught up on couldn't be followed for new episodes: {error}");
            }
        }
        Ok(())
    }

    /// Stores where someone got to. An older write never replaces a newer one.
    pub fn save_progress(&self, mut progress: PlaybackProgress) -> Result<()> {
        if progress.updated_at <= 0 {
            progress.updated_at = now();
        }
        self.library.save_progress(&progress)?;
        self.send_progress_to_server(&progress);
        Ok(())
    }

    pub fn progress(&self, item_id: &str) -> Result<Option<PlaybackProgress>> {
        self.library.progress(item_id)
    }

    /// Notes a channel was watched, for Recent Channels. Tuning back within an
    /// hour counts once.
    pub fn record_channel_watched(&self, channel_id: &str) -> Result<()> {
        let Some(channel) = self.catalog.channel(channel_id)? else {
            return self.library.add_history(channel_id, MediaKind::Channel, ids::source_of(channel_id), now());
        };
        self.library.add_history(channel_id, MediaKind::Channel, &channel.source_id, now())?;
        self.library.note_history_title(channel_id, &channel.name, None)
    }

    /// Notes a film or episode started playing, for Viewing History, with
    /// what it's called: add-on titles aren't in the catalog to look up.
    pub fn record_watched(&self, item_id: &str, kind: MediaKind, source_id: &str, title: &str, subtitle: Option<&str>) -> Result<()> {
        self.library.add_history(item_id, kind, source_id, now())?;
        self.library.note_history_title(item_id, title, subtitle)
    }

    /// Viewing History: what was watched, newest first, named as the
    /// catalog names it now, or as it was called when it played. Hidden and
    /// locked channels and titles are left out, as everywhere else.
    pub fn history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        let rows = self.library.history(limit)?;
        let excluded = self.excluded(false)?;
        let ids = |kind: MediaKind| -> Vec<String> { rows.iter().filter(|row| row.kind == kind).map(|row| row.item_id.clone()).collect() };
        let channels: HashMap<String, Channel> =
            self.present_channels(self.catalog.channels_by_ids(&ids(MediaKind::Channel))?).into_iter().map(|channel| (channel.id.clone(), channel)).collect();
        let movies: HashMap<_, _> = self.catalog.movies_by_ids(&ids(MediaKind::Movie))?.into_iter().map(|movie| (movie.id.clone(), movie)).collect();
        let episodes: HashMap<_, _> = self.catalog.episodes_by_ids(&ids(MediaKind::Episode))?.into_iter().map(|episode| (episode.id.clone(), episode)).collect();
        let shows: Vec<String> = episodes.values().map(|episode| episode.series_id.clone()).collect();
        let shows: HashMap<_, _> = self.catalog.series_by_ids(&shows)?.into_iter().map(|show| (show.id.clone(), show)).collect();

        let mut entries = Vec::new();
        for row in rows {
            let named = match row.kind {
                MediaKind::Channel => channels.get(&row.item_id).map(|channel| (excluded.channel(channel), channel.name.clone(), None)),
                MediaKind::Movie => movies
                    .get(&row.item_id)
                    .map(|movie| (excluded.title(&movie.id, movie.category_id.as_deref()), names::media_title(&movie.title), None)),
                MediaKind::Episode => episodes.get(&row.item_id).and_then(|episode| {
                    let show = shows.get(&episode.series_id)?;
                    Some((excluded.title(&show.id, show.category_id.as_deref()), names::media_title(&show.title), Some(episode_subtitle(episode))))
                }),
                _ => None,
            };
            let (title, subtitle) = match named {
                Some((true, ..)) => continue,
                Some((false, title, subtitle)) => (title, subtitle),
                // Gone from the catalog, or never in it: as it was called.
                None => match row.title {
                    Some(title) => (title, row.subtitle),
                    None => continue,
                },
            };
            entries.push(HistoryEntry { item_id: row.item_id, kind: row.kind, source_id: row.source_id, title, subtitle, watched_at: row.watched_at });
        }
        Ok(entries)
    }

    /// Forgets Viewing History and Recent Channels. Progress, Continue
    /// Watching and what's marked watched stay.
    pub fn clear_history(&self) -> Result<()> {
        self.library.clear_history()
    }

    /// The channels watched most recently, newest first, still in the
    /// catalog and neither hidden nor locked.
    pub fn recent_channels(&self, limit: usize) -> Result<Vec<Channel>> {
        let recent = self.library.recent_channel_ids(limit)?;
        let excluded = self.excluded(false)?;
        let channels = self.catalog.channels_by_ids(&recent)?.into_iter().filter(|channel| !excluded.channel(channel)).collect();
        Ok(self.present_channels(channels))
    }
}
