//! Xorvio's data layer: sources (M3U playlists, Xtream Codes providers, XMLTV
//! guides and the demo library), the catalog and library databases, search,
//! and the secrets that go with each source.
//!
//! Everything goes through [`Core`], which the app shares as `Arc<Core>`.
//! Database calls are quick and synchronous; anything that talks to a
//! provider is `async`. A refresh downloads and parses everything first and
//! then writes it in one transaction, so a failed refresh never damages the
//! last good catalog.

mod airings;
pub mod backup;
mod browse;
mod classify;
mod continue_watching;
mod details;
mod details_api;
mod edits;
mod error;
mod folders;
pub mod genres;
mod grouping;
mod guides;
mod home_rows;
mod http;
mod images;
mod json;
mod media_library;
pub mod names;
mod new_episodes;
mod next_episode;
pub mod parental;
mod personal;
mod playback;
mod profiles;
mod redact;
mod refresh;
mod reminders;
mod secrets;
mod segments;
mod servers;
mod snapshot;
mod source_sheet;
mod sources;
mod store;
pub mod sync;
mod titles;
mod tmdb;
mod track_choices;
mod trakt;
mod watchlist;
mod wikidata;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use xorvio_model::{
    Category, Channel, ContentGenre, DEMO_SOURCE_ID, LiveStreamFormat, Movie, NewSource, RefreshInterval, Series,
    SourceConfig, SourceKind, SourceSecrets, SourceSummary, Timestamp,
};

pub use airings::Airing;
pub use continue_watching::next_episode_reason;
pub use error::SourceError;
pub use images::{IMAGE_SCHEME, image_url, parse_image_path};
pub use new_episodes::NewEpisodes;
pub use redact::redact;
pub use reminders::Reminder;
pub use secrets::{KEYCHAIN_SERVICE, KeyringSecrets, MemorySecrets, SecretStore};
pub use servers::{PlexServerChoice, PlexSignInStart};
pub use source_sheet::{XtreamAccountCheck, xtream_login_in_link};
pub use sources::folder::{FOLDER_SCHEME, parse_folder_path};
pub use sources::server::{ARTWORK_SCHEME, artwork_url, parse_artwork_path};
pub use sources::xtream::XtreamCredentials;
pub use track_choices::{SubtitleChoice, TrackChoice};
pub use trakt::{AddonReach, TraktEvent, TraktListener};

use crate::http::Http;
use crate::store::catalog::CatalogStore;
use crate::store::config::ConfigFile;
use crate::store::library::LibraryStore;
use crate::store::lock;

pub type Result<T, E = anyhow::Error> = std::result::Result<T, E>;

/// Which channels to list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelFilter {
    pub category_id: Option<String>,
    /// Favorite channels only, in the list's order.
    pub favorites_only: bool,
    /// The channels of one named list (a group), in its order.
    pub list_id: Option<String>,
    /// One source only (even a paused one); otherwise every active source.
    pub source_id: Option<String>,
    /// Hidden channels and categories too (to show them again). Locked ones
    /// still need the PIN.
    pub include_hidden: bool,
    pub limit: Option<usize>,
    /// Similar channels ("News 24 HD", "News 24 4K") as one row, or each
    /// on its own; `None` follows [`Core::set_group_similar_channels`].
    pub group_similar: Option<bool>,
}

/// Which films or series to list, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BrowseQuery {
    pub category_id: Option<String>,
    /// Matches titles whose genre mentions this ("Drama" finds "Crime, Drama").
    pub genre: Option<String>,
    /// Matches titles whose genre field names this genre, or that are in a
    /// category named for it ("EN - ACTION"), for Home's genre rows.
    pub content_genre: Option<ContentGenre>,
    pub sort: BrowseSort,
    pub limit: usize,
    pub offset: usize,
    /// One source's titles only; otherwise every active source's.
    pub source_id: Option<String>,
    /// All titles, or only those not watched or part-way through.
    pub watched: WatchedFilter,
    /// The copies of a film the sources carry (by TMDB or IMDb id, else by
    /// title and year) as one, the first in the list's order standing for
    /// them all. Films only.
    pub group_versions: bool,
}

impl Default for BrowseQuery {
    fn default() -> Self {
        Self {
            category_id: None,
            genre: None,
            content_genre: None,
            sort: BrowseSort::default(),
            limit: 200,
            offset: 0,
            source_id: None,
            watched: WatchedFilter::All,
            group_versions: false,
        }
    }
}

/// Films and series by what's been watched of them. A film counts as
/// watched when any version of it was finished; a series is unwatched until
/// an episode is started, and in progress until every episode its source
/// lists is finished.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WatchedFilter {
    #[default]
    All,
    Unwatched,
    InProgress,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BrowseSort {
    #[default]
    RecentlyAdded,
    Title,
    Year,
    Rating,
}

/// The current time in Unix seconds.
pub(crate) fn now() -> Timestamp {
    chrono::Utc::now().timestamp()
}

/// Xorvio's data layer. `Send + Sync`; share it as `Arc<Core>`.
pub struct Core {
    secrets: Arc<dyn SecretStore>,
    config_file: ConfigFile,
    sources: Mutex<Vec<SourceConfig>>,
    catalog: Arc<CatalogStore>,
    library: Arc<LibraryStore>,
    http: Http,
    /// Sources with a refresh running.
    refreshing: Arc<Mutex<HashSet<String>>>,
    /// Media folders waiting to be read again after a change to their files.
    folder_rereads: Mutex<HashSet<String>>,
    /// How to stop each of those refreshes ([`Core::cancel_refresh`]).
    refresh_stops: refresh::RefreshStops,
    /// Series whose episodes were fetched from the provider this session.
    episodes_fetched: Mutex<HashSet<String>>,
    /// Series already looked up on TVmaze this session, found or not.
    tvmaze_tried: Mutex<HashSet<String>>,
    /// One provider detail request per title at a time.
    detail_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Spacing for provider detail requests, per source.
    provider_turns: Mutex<HashMap<String, Arc<details_api::ProviderTurns>>>,
    tidy_names: AtomicBool,
    group_similar: AtomicBool,
    tvmaze_enabled: AtomicBool,
    tvmaze_base: Mutex<String>,
    /// Episodes' intros, recaps and credits are looked up on IntroDB.
    intro_db_enabled: AtomicBool,
    intro_db_base: Mutex<String>,
    /// Home's Trakt charts and lists load before an account is connected.
    trakt_rows_without_account: AtomicBool,
    /// Names, hiding and locks from the library, read once and kept until
    /// they change.
    edits: Mutex<Option<Arc<edits::Edits>>>,
    /// When the parental PIN was entered, and how long that lasts.
    unlock: Mutex<parental::Unlock>,
    /// Wrong PINs in a row, and when the last one was tried.
    pin_failures: Mutex<(u32, Option<std::time::Instant>)>,
    /// The library profile in use, and what category names say.
    profile_state: profiles::ProfileState,
    /// Approved Plex sign-ins waiting for a server to be picked.
    plex_sign_ins: Mutex<servers::PlexSignIns>,
    /// Where Plex sign-ins and server lists come from (a stand-in in tests).
    plex_tv_base: Mutex<String>,
    /// The Trakt connection, shared with what's sent to Trakt in the background.
    trakt: Arc<trakt::Trakt>,
    tmdb: tmdb::Tmdb,
    /// Filmographies for people's pages when TMDB can't help.
    wikidata: wikidata::Wikidata,
    /// When this was opened: media folders are read again once each launch.
    opened_at: Timestamp,
}

impl Core {
    /// Opens (or creates) Xorvio's data in `data_dir`: `catalog.sqlite`,
    /// `library.sqlite` and `sources.json`. Secrets go to `secrets`.
    pub fn open(data_dir: &Path, secrets: Arc<dyn SecretStore>) -> Result<Arc<Core>> {
        std::fs::create_dir_all(data_dir)?;
        let catalog = CatalogStore::open(&data_dir.join("catalog.sqlite"))?;
        let library = Arc::new(LibraryStore::open(&data_dir.join("library.sqlite"))?);
        let config_file = ConfigFile::new(data_dir);
        let sources = config_file.load();
        // Listings from more than a week ago are never shown; replay windows are shorter.
        if let Err(error) = catalog.prune_guide(now() - 8 * 86_400) {
            tracing::warn!("old guide listings could not be removed: {error}");
        }
        let http = Http::new()?;
        let trakt = Arc::new(trakt::Trakt::new(http.clone(), secrets.clone(), library.clone()));
        let tmdb = tmdb::Tmdb::new(secrets.as_ref());
        Ok(Arc::new(Core {
            secrets,
            config_file,
            sources: Mutex::new(sources),
            catalog: Arc::new(catalog),
            library,
            http,
            trakt,
            tmdb,
            wikidata: wikidata::Wikidata::new(),
            refreshing: Arc::default(),
            folder_rereads: Mutex::default(),
            refresh_stops: Arc::default(),
            episodes_fetched: Mutex::default(),
            tvmaze_tried: Mutex::default(),
            detail_locks: Mutex::default(),
            provider_turns: Mutex::default(),
            tidy_names: AtomicBool::new(true),
            group_similar: AtomicBool::new(true),
            tvmaze_enabled: AtomicBool::new(true),
            tvmaze_base: Mutex::new(sources::tvmaze::DEFAULT_BASE.into()),
            intro_db_enabled: AtomicBool::new(true),
            intro_db_base: Mutex::new(segments::DEFAULT_BASE.into()),
            trakt_rows_without_account: AtomicBool::new(false),
            edits: Mutex::default(),
            unlock: Mutex::default(),
            pin_failures: Mutex::default(),
            profile_state: profiles::ProfileState::default(),
            plex_sign_ins: Mutex::default(),
            plex_tv_base: Mutex::new(sources::plex::PLEX_TV.into()),
            opened_at: now(),
        }))
    }

    // MARK: Settings

    /// Whether channel and category names are tidied for display ("UK | Channel
    /// One FHD" → "Channel One"). On by default; the catalog keeps the originals.
    pub fn set_tidy_names(&self, tidy: bool) {
        self.tidy_names.store(tidy, Ordering::Relaxed);
    }

    /// Whether channel lists show the versions of a channel a provider lists
    /// several times as one row (see grouping.rs). On by default.
    pub fn set_group_similar_channels(&self, group: bool) {
        self.group_similar.store(group, Ordering::Relaxed);
    }

    /// Whether series details missing from the provider are looked up on
    /// TVmaze (only the title is sent). On by default.
    pub fn set_tvmaze_enabled(&self, enabled: bool) {
        self.tvmaze_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether episodes' intros, recaps and end credits are looked up on
    /// IntroDB (the series' IMDb id and the episode's number are sent). On
    /// by default.
    pub fn set_intro_db_enabled(&self, enabled: bool) {
        self.intro_db_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether TMDB is asked anything at all, with whichever key. On by
    /// default; off, what it said before still shows.
    pub fn set_tmdb_enabled(&self, enabled: bool) {
        self.tmdb.set_enabled(enabled);
    }

    /// Whether Home's Trakt charts and public lists load before a Trakt
    /// account is connected. Off by default: nothing goes to Trakt until
    /// someone connects or chooses this.
    pub fn set_trakt_rows_without_account(&self, enabled: bool) {
        self.trakt_rows_without_account.store(enabled, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn set_tvmaze_base(&self, base: &str) {
        *lock(&self.tvmaze_base) = base.to_string();
    }

    #[cfg(test)]
    pub(crate) fn set_plex_tv_base(&self, base: &str) {
        *lock(&self.plex_tv_base) = base.to_string();
    }

    // MARK: Sources

    pub fn list_sources(&self) -> Result<Vec<SourceSummary>> {
        let statuses = self.catalog.statuses()?;
        let refreshing = lock(&self.refreshing).clone();
        Ok(self
            .configs()
            .into_iter()
            .map(|config| SourceSummary {
                status: statuses.get(&config.id).cloned().unwrap_or_default(),
                refreshing: refreshing.contains(&config.id),
                config,
            })
            .collect())
    }

    /// Checks the login or playlist, then saves the source and its secrets.
    /// The catalog is read by a separate [`Core::refresh_source`].
    pub async fn add_source(&self, new: NewSource) -> Result<SourceConfig> {
        let now = now();
        let mut secrets = trimmed(new.secrets.clone());
        let user_agent = new.user_agent.as_deref().map(str::trim).filter(|agent| !agent.is_empty()).map(str::to_string);
        if let Some(guide) = &secrets.epg_url
            && !is_usable_link(guide)
        {
            return Err(SourceError::MissingConfiguration("a valid guide address".into()).into());
        }
        let mut file_path = None;
        let display_host = match new.kind {
            SourceKind::Xtream => {
                let server = secrets
                    .server_url
                    .as_deref()
                    .and_then(XtreamCredentials::normalize_server)
                    .ok_or_else(|| SourceError::MissingConfiguration("a server address".into()))?;
                secrets.server_url = Some(server);
                let credentials = XtreamCredentials::from_secrets(&secrets)
                    .ok_or_else(|| SourceError::MissingConfiguration("a username and password".into()))?;
                let client = sources::xtream::XtreamClient {
                    http: &self.http,
                    credentials: &credentials,
                    user_agent: user_agent.as_deref(),
                };
                client.authenticate(now).await?;
                redact::host_description(&credentials.server)
            }
            SourceKind::M3uUrl => {
                let link = secrets
                    .playlist_url
                    .clone()
                    .filter(|link| is_usable_link(link))
                    .ok_or_else(|| SourceError::MissingConfiguration("a playlist URL".into()))?;
                let bytes = self.http.bytes(&link, user_agent.as_deref()).await?;
                sources::m3u::parse_bytes(&bytes)?;
                redact::host_description(&link)
            }
            SourceKind::M3uFile => {
                let path = new
                    .file_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                    .ok_or_else(|| SourceError::MissingConfiguration("a playlist file".into()))?;
                let bytes = tokio::fs::read(path).await.map_err(|_| SourceError::FileUnavailable)?;
                sources::m3u::parse_bytes(&bytes)?;
                file_path = Some(path.to_string());
                None
            }
            SourceKind::Demo => {
                self.add_demo_source()?;
                return self.config(DEMO_SOURCE_ID);
            }
            SourceKind::Folder => {
                let folders: Vec<String> = new.file_path.iter().cloned().collect();
                return self.add_folder_source(&new.name, &folders);
            }
            SourceKind::Plex | SourceKind::Jellyfin | SourceKind::Emby => {
                let (kept, server_name) = self.sign_in_to_server(new.kind, &secrets).await?;
                let name = match new.name.trim() {
                    "" => server_name.unwrap_or_else(|| default_name(new.kind).into()),
                    name => name.to_string(),
                };
                return self.save_new_source(new.kind, name, &kept);
            }
        };

        let name = match new.name.trim() {
            "" => display_host.clone().unwrap_or_else(|| default_name(new.kind).into()),
            name => name.to_string(),
        };
        let config = SourceConfig {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            kind: new.kind,
            display_host,
            file_path,
            folders: Vec::new(),
            is_paused: false,
            live_format: new.live_format,
            refresh_interval: RefreshInterval::default(),
            user_agent,
            created_at: now,
            sort_index: self.next_sort_index(),
            guide_offset_minutes: 0,
        };
        secrets::save(self.secrets.as_ref(), &config.id, &secrets)?;
        self.save_configs(|configs| configs.push(config.clone()))?;
        Ok(config)
    }

    /// Saves a media server source that's been signed in to, shown by its
    /// server's host.
    pub(crate) fn save_new_source(&self, kind: SourceKind, name: String, secrets: &SourceSecrets) -> Result<SourceConfig> {
        let config = SourceConfig {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            kind,
            display_host: secrets.server_url.as_deref().and_then(redact::host_description),
            file_path: None,
            folders: Vec::new(),
            is_paused: false,
            live_format: LiveStreamFormat::default(),
            refresh_interval: RefreshInterval::default(),
            user_agent: None,
            created_at: now(),
            sort_index: self.next_sort_index(),
            guide_offset_minutes: 0,
        };
        secrets::save(self.secrets.as_ref(), &config.id, secrets)?;
        self.save_configs(|configs| configs.push(config.clone()))?;
        Ok(config)
    }

    /// Saves edited settings, and new secrets when given. The id, kind and
    /// creation time stay as they were, and the host shown is always read
    /// from the secrets, so it can never carry a login.
    pub fn update_source(&self, config: SourceConfig, secrets: Option<SourceSecrets>) -> Result<()> {
        let existing = self.config(&config.id)?;
        let mut updated = SourceConfig {
            id: existing.id.clone(),
            kind: existing.kind,
            created_at: existing.created_at,
            display_host: existing.display_host.clone(),
            user_agent: config.user_agent.as_deref().map(str::trim).filter(|agent| !agent.is_empty()).map(str::to_string),
            ..config
        };
        // A media folder's folders are checked like new ones, and it has no secrets.
        if existing.kind == SourceKind::Folder {
            updated.folders = self.checked_folders(&updated.folders, Some(&existing))?;
            updated.display_host = sources::folder::naming::describe(&updated.folders);
            updated.file_path = None;
        }
        if let Some(secrets) = secrets.filter(|_| existing.kind != SourceKind::Folder) {
            let mut secrets = trimmed(secrets);
            if existing.kind == SourceKind::Xtream {
                secrets.server_url = secrets.server_url.as_deref().and_then(XtreamCredentials::normalize_server);
            }
            if existing.kind.is_media_server() {
                // The interface never sees the token, so it keeps the one there is;
                // a new address is the same server moved.
                let kept = self.load_secrets(&existing.id)?;
                secrets.server_url = secrets.server_url.as_deref().and_then(sources::server::normalize_address).or(kept.server_url);
                secrets.token = secrets.token.or(kept.token);
                secrets.user_id = secrets.user_id.or(kept.user_id);
                secrets.password = None;
            }
            updated.display_host = match existing.kind {
                SourceKind::Xtream | SourceKind::Plex | SourceKind::Jellyfin | SourceKind::Emby => {
                    secrets.server_url.as_deref().and_then(redact::host_description)
                }
                SourceKind::M3uUrl => secrets.playlist_url.as_deref().and_then(redact::host_description),
                _ => None,
            };
            secrets::save(self.secrets.as_ref(), &existing.id, &secrets)?;
        }
        self.save_configs(|configs| {
            if let Some(slot) = configs.iter_mut().find(|slot| slot.id == updated.id) {
                *slot = updated.clone();
            }
        })?;
        // A dropped playlist replaced by another file leaves no copy behind.
        if updated.file_path != existing.file_path {
            self.forget_playlist_copy(&existing);
        }
        Ok(())
    }

    /// Puts the sources in a new order, the one Live TV lists channels in
    /// across sources (and the one Home, search and the guide follow). Ids
    /// left out keep their order after the ones given; unknown ids are
    /// ignored.
    pub fn reorder_sources(&self, ids: &[String]) -> Result<()> {
        self.save_configs(|configs| {
            let place = |config: &SourceConfig| ids.iter().position(|id| *id == config.id).unwrap_or(ids.len());
            // A stable sort keeps the ones left out in the order they had.
            configs.sort_by_key(place);
            for (index, config) in configs.iter_mut().enumerate() {
                config.sort_index = index as i64;
            }
        })
    }

    /// Removes a source with its secrets, its catalog and everything personal
    /// tied to it.
    pub fn remove_source(&self, id: &str) -> Result<()> {
        let removed = self.config(id)?;
        self.save_configs(|configs| configs.retain(|config| config.id != id))?;
        self.forget_playlist_copy(&removed);
        self.secrets.delete(id)?;
        self.catalog.remove_source(id)?;
        self.library.remove_source(id)?;
        self.forget_folder_scan(id);
        // Channels elsewhere that were matched to its guide by hand find their own again.
        self.catalog.relink_guides(None, &self.guide_links()?)?;
        let prefix = format!("{id}|");
        lock(&self.episodes_fetched).retain(|series| !series.starts_with(&prefix));
        lock(&self.tvmaze_tried).retain(|series| !series.starts_with(&prefix));
        Ok(())
    }

    /// `text` with every source's login and link tokens masked, as well as
    /// anything link-shaped, for a diagnostic log someone exports.
    pub fn redact_known(&self, text: &str) -> String {
        let mut known = Vec::new();
        for config in self.configs() {
            if let Ok(secrets) = secrets::load(self.secrets.as_ref(), &config.id) {
                known.extend(redact::secret_values(&secrets));
            }
        }
        redact::redact(text, &known)
    }

    /// A source's secrets for its edit sheet. A media server's token stays here.
    pub fn source_secrets(&self, id: &str) -> Result<SourceSecrets> {
        let secrets = secrets::load(self.secrets.as_ref(), id)?;
        if self.config(id)?.kind.is_media_server() {
            return Ok(SourceSecrets { server_url: secrets.server_url, username: secrets.username, ..Default::default() });
        }
        Ok(secrets)
    }

    /// Adds the demonstration source and fills it at once (it's all local),
    /// guide included. Does nothing when it's already there.
    pub fn add_demo_source(&self) -> Result<()> {
        if self.config(DEMO_SOURCE_ID).is_ok() {
            return Ok(());
        }
        let config = SourceConfig {
            id: DEMO_SOURCE_ID.into(),
            name: "Demo".into(),
            kind: SourceKind::Demo,
            display_host: None,
            file_path: None,
            folders: Vec::new(),
            is_paused: false,
            live_format: LiveStreamFormat::Hls,
            refresh_interval: RefreshInterval::Daily,
            user_agent: None,
            created_at: now(),
            sort_index: self.next_sort_index(),
            guide_offset_minutes: 0,
        };
        self.save_configs(|configs| configs.push(config))?;
        self.install_demo(now())?;
        Ok(())
    }

    // MARK: Helpers

    fn configs(&self) -> Vec<SourceConfig> {
        lock(&self.sources).clone()
    }

    fn config(&self, id: &str) -> Result<SourceConfig> {
        lock(&self.sources)
            .iter()
            .find(|config| config.id == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("This source no longer exists."))
    }

    /// Ids of sources that aren't paused, in the user's order.
    fn active_source_ids(&self) -> Vec<String> {
        lock(&self.sources).iter().filter(|config| !config.is_paused).map(|config| config.id.clone()).collect()
    }

    fn next_sort_index(&self) -> i64 {
        lock(&self.sources).iter().map(|config| config.sort_index + 1).max().unwrap_or(0)
    }

    /// Changes the source list and writes it out; the list in memory changes
    /// only if the file was written.
    fn save_configs(&self, change: impl FnOnce(&mut Vec<SourceConfig>)) -> Result<()> {
        let mut sources = lock(&self.sources);
        let mut updated = sources.clone();
        change(&mut updated);
        self.config_file.save(&updated)?;
        *sources = updated;
        Ok(())
    }

    fn load_secrets(&self, id: &str) -> Result<SourceSecrets> {
        secrets::load(self.secrets.as_ref(), id)
    }

    // MARK: Presentation

    /// A channel as the interface shows it: under the name someone gave it,
    /// or the provider's, tidied, with its catch-up offset.
    fn present_channel(&self, channel: Channel) -> Channel {
        self.present_channel_with(&self.current_edits(), channel)
    }

    fn present_channel_with(&self, edits: &edits::Edits, mut channel: Channel) -> Channel {
        match edits.name(&channel.id) {
            Some(name) => channel.name = name.to_string(),
            None => channel.name = self.provider_name(&channel.name),
        }
        channel.catchup_offset_minutes = edits.catchup_offset(&channel.id);
        channel
    }

    fn present_channels(&self, channels: Vec<Channel>) -> Vec<Channel> {
        let edits = self.current_edits();
        channels.into_iter().map(|channel| self.present_channel_with(&edits, channel)).collect()
    }

    fn present_category_with(&self, edits: &edits::Edits, mut category: Category) -> Category {
        match edits.name(&category.id) {
            Some(name) => category.name = name.to_string(),
            None => category.name = self.provider_name(&category.name),
        }
        category
    }

    /// A channel or category's provider name, tidied when that's on.
    fn provider_name(&self, raw: &str) -> String {
        if self.tidy_names.load(Ordering::Relaxed) { names::display_name(raw) } else { raw.to_string() }
    }

    /// Film titles lose provider tags ("4K-TOP - ") whatever the setting:
    /// they're never part of the title. A media folder's or media server's
    /// are the person's own, so "XTC - Dear God" keeps its band.
    fn present_movie(mut movie: Movie) -> Movie {
        if !matches!(movie.stream, xorvio_model::StreamLocator::File { .. } | xorvio_model::StreamLocator::Server { .. }) {
            movie.title = names::media_title(&movie.title);
        }
        movie
    }

    fn present_series(mut series: Series) -> Series {
        series.title = names::media_title(&series.title);
        series
    }
}

/// Secrets with surrounding spaces removed and empty values dropped.
fn trimmed(secrets: SourceSecrets) -> SourceSecrets {
    let clean = |value: Option<String>| value.map(|text| text.trim().to_string()).filter(|text| !text.is_empty());
    SourceSecrets {
        playlist_url: clean(secrets.playlist_url),
        epg_url: clean(secrets.epg_url),
        server_url: clean(secrets.server_url),
        username: clean(secrets.username),
        // Passwords keep their spaces; only an empty one is dropped.
        password: secrets.password.filter(|text| !text.is_empty()),
        token: clean(secrets.token),
        user_id: clean(secrets.user_id),
    }
}

/// A web link, or a `file://` link to something on this computer.
fn is_usable_link(link: &str) -> bool {
    url::Url::parse(link).is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https" | "file"))
}

fn default_name(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::M3uUrl | SourceKind::M3uFile => "Playlist",
        SourceKind::Xtream => "Provider",
        SourceKind::Demo => "Demo",
        SourceKind::Folder => "Media Folder",
        SourceKind::Plex => "Plex",
        SourceKind::Jellyfin => "Jellyfin",
        SourceKind::Emby => "Emby",
    }
}

/// `Core` is shared across threads; this stops compiling if that ever changes.
const _: () = {
    const fn check<T: Send + Sync>() {}
    check::<Core>();
};
