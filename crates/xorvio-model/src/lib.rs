//! Shared data types for Xorvio. Every type here crosses the boundary to the
//! web interface as JSON (camelCase), so the TypeScript twins in
//! `src/lib/types.ts` must change together with them.
//!
//! Times are Unix seconds (`i64`) everywhere: the interface formats them in the
//! viewer's time zone, and nothing needs sub-second precision.

use serde::{Deserialize, Serialize};

pub type SourceId = String;
/// Unix time in seconds.
pub type Timestamp = i64;

/// The built-in demonstration source. It is always labeled as such.
pub const DEMO_SOURCE_ID: &str = "demo";

// MARK: - Sources

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    M3uUrl,
    M3uFile,
    Xtream,
    Demo,
    /// A folder of the user's own video files.
    Folder,
    /// A person's own media servers: their films and series, with artwork,
    /// details and what's been watched.
    Plex,
    Jellyfin,
    Emby,
}

impl SourceKind {
    /// A playlist or provider login, rather than the demo, a folder or a media server.
    pub fn is_provider(self) -> bool {
        matches!(self, Self::M3uUrl | Self::M3uFile | Self::Xtream)
    }

    /// Plex, Jellyfin or Emby.
    pub fn is_media_server(self) -> bool {
        matches!(self, Self::Plex | Self::Jellyfin | Self::Emby)
    }
}

/// Transport preference for providers that offer both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LiveStreamFormat {
    #[default]
    Hls,
    Mpegts,
}

impl LiveStreamFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Hls => "m3u8",
            Self::Mpegts => "ts",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RefreshInterval {
    Manual,
    Every6Hours,
    #[default]
    Daily,
}

impl RefreshInterval {
    pub fn seconds(self) -> Option<i64> {
        match self {
            Self::Manual => None,
            Self::Every6Hours => Some(6 * 3600),
            Self::Daily => Some(24 * 3600),
        }
    }
}

/// Non-secret, user-visible configuration. Credential-bearing values live in
/// the operating system's keychain as [`SourceSecrets`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceConfig {
    /// Stable identity, never derived from a URL, so editing an address keeps
    /// favorites, edits and history.
    pub id: SourceId,
    pub name: String,
    pub kind: SourceKind,
    /// Host shown in the UI ("provider.example"). Never holds credentials.
    pub display_host: Option<String>,
    /// For playlist files.
    pub file_path: Option<String>,
    /// A media folder source's folders: on this computer, a drive, or a
    /// network share mounted in Finder.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,
    #[serde(default)]
    pub is_paused: bool,
    #[serde(default)]
    pub live_format: LiveStreamFormat,
    #[serde(default)]
    pub refresh_interval: RefreshInterval,
    /// Sent with every request to this provider; many panels insist on a known player.
    #[serde(default)]
    pub user_agent: Option<String>,
    pub created_at: Timestamp,
    #[serde(default)]
    pub sort_index: i64,
    /// Moves every programme in the source's guide by this many minutes, for
    /// guides written for the wrong time zone. Applied when the guide loads.
    #[serde(default)]
    pub guide_offset_minutes: i64,
    /// Media servers: their live channels and guide are read along with
    /// their films and series. On unless turned off.
    #[serde(default = "included")]
    pub include_live_tv: bool,
}

fn included() -> bool {
    true
}

/// Everything credential-bearing about a source. Stored only in the keychain.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSecrets {
    pub playlist_url: Option<String>,
    pub epg_url: Option<String>,
    pub server_url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    /// A media server's access token. Jellyfin and Emby keep this rather
    /// than the password; it never goes to the interface.
    #[serde(default)]
    pub token: Option<String>,
    /// Jellyfin and Emby: the signed-in user's id. Plex: the server's
    /// machine identifier, so a new address can be found if it moves.
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCapabilities {
    pub live: bool,
    pub movies: bool,
    pub series: bool,
    pub guide: bool,
    pub replay: bool,
}

/// Result of the most recent refresh attempts. A failed refresh records its
/// error but never clears `last_success` or the catalog it produced.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub last_success: Option<Timestamp>,
    pub last_attempt: Option<Timestamp>,
    pub last_error: Option<String>,
    pub guide_last_success: Option<Timestamp>,
    pub guide_last_error: Option<String>,
    pub channel_count: i64,
    pub movie_count: i64,
    pub series_count: i64,
    pub program_count: i64,
    pub capabilities: SourceCapabilities,
    /// The provider's simultaneous-connection allowance, when it reports
    /// one; for a media server, how many tuners its live TV has.
    pub max_connections: Option<i64>,
    pub account_expires: Option<Timestamp>,
    pub authentication_expired: bool,
    /// Provider server time zone (Xtream replay URLs use server-local time).
    pub server_time_zone: Option<String>,
    /// Set when a refresh succeeded but part of the catalog was kept from before.
    pub catalog_warning: Option<String>,
}

/// A source as the interface lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSummary {
    pub config: SourceConfig,
    pub status: SourceStatus,
    /// True while a refresh is running.
    pub refreshing: bool,
}

/// What the "Add Source" form sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSource {
    pub name: String,
    pub kind: SourceKind,
    pub secrets: SourceSecrets,
    pub file_path: Option<String>,
    #[serde(default)]
    pub live_format: LiveStreamFormat,
    #[serde(default)]
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshProgress {
    pub source_id: SourceId,
    /// "Connecting", "Reading catalog", "Organizing content", "Loading guide".
    pub phase: String,
    /// 0…1 when known.
    pub fraction: Option<f64>,
}

// MARK: - Catalog

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CategoryKind {
    Live,
    Movie,
    Series,
}

impl CategoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Movie => "movie",
            Self::Series => "series",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum XtreamPath {
    Live,
    Movie,
    Series,
}

/// Where a stream comes from. Xtream streams are stored without credentials
/// and resolved at playback time with the keychain secrets.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StreamLocator {
    Url { url: String },
    #[serde(rename_all = "camelCase")]
    Xtream { path: XtreamPath, stream_id: String, extension: String },
    /// A file on this computer: a media folder item or a recording.
    File { path: String },
    /// A title from add-ons (Stremio protocol). Copies are listed by the
    /// add-ons' stream resource when it's played.
    #[serde(rename_all = "camelCase")]
    Addon { media_type: String, media_id: String },
    /// A film or episode on a media server, by the server's own keys; the
    /// address and token are added at playback.
    #[serde(rename_all = "camelCase")]
    Server {
        /// Jellyfin and Emby: the item id. Plex: the rating key.
        key: String,
        /// The file to play when the server needs more than the key: Plex's
        /// part path ("/library/parts/…/file.mkv"), Jellyfin's media source id.
        part: Option<String>,
        /// The file's container ("mkv", "mp4"), when the server says.
        extension: Option<String>,
    },
    /// A live channel from a media server's tuners, by the server's own
    /// keys. It's tuned on the server only while it's being read.
    #[serde(rename_all = "camelCase")]
    ServerChannel {
        /// Jellyfin and Emby: the channel's item id. Plex: the channel's
        /// identifier in its DVR's lineup.
        key: String,
        /// Plex: the key of the DVR that tunes it.
        dvr: Option<String>,
    },
}

impl StreamLocator {
    pub fn extension(&self) -> Option<String> {
        let path = match self {
            Self::Url { url } => url.split(['?', '#']).next().unwrap_or(url),
            Self::File { path } => path,
            Self::Xtream { extension, .. } => return Some(extension.to_lowercase()),
            Self::Addon { .. } => return None,
            Self::Server { extension, .. } => return extension.as_deref().map(str::to_lowercase),
            // Always MPEG-TS, however the server sends it (see xorvio-core's tuner relay).
            Self::ServerChannel { .. } => return Some("ts".into()),
        };
        let last = path.rsplit('/').next().unwrap_or(path);
        last.rsplit_once('.').map(|(_, ext)| ext.to_lowercase())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    /// "<source>|<kind>cat|<key>"
    pub id: String,
    pub source_id: SourceId,
    pub kind: CategoryKind,
    pub name: String,
    pub sort_index: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    /// "<source>|ch|<key>"
    pub id: String,
    pub source_id: SourceId,
    pub name: String,
    pub number: Option<i64>,
    pub logo_url: Option<String>,
    pub category_id: Option<String>,
    /// XMLTV channel id (tvg-id or the provider's epg id).
    pub epg_id: Option<String>,
    pub stream: StreamLocator,
    /// Provider replay window in days; 0 when it doesn't advertise replay.
    pub catchup_days: i64,
    /// M3U `catchup-source` template, when present.
    pub catchup_source: Option<String>,
    pub sort_index: i64,
    /// Minutes replays of this channel are moved by, for providers whose
    /// replays start early or late; 0 for none. Someone's own setting.
    #[serde(default)]
    pub catchup_offset_minutes: i64,
    /// With similar channels grouped ("News 24 HD", "News 24 4K"), every
    /// version this row stands for, in the provider's order, this channel
    /// (the one that plays) among them. Empty for a channel listed once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<ChannelVersion>,
}

/// One version of a channel the provider lists several times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelVersion {
    pub id: String,
    /// The provider's name for it ("US: News 24 FHD"), or the one someone gave it.
    pub name: String,
    /// "4K", "FHD", "HD" or "SD", as the name says.
    pub quality: Option<String>,
    /// Someone chose to always play this version.
    pub preferred: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Movie {
    /// "<source>|mv|<key>"
    pub id: String,
    pub source_id: SourceId,
    pub title: String,
    pub year: Option<i64>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub category_id: Option<String>,
    /// On a 10-point scale.
    pub rating: Option<f64>,
    pub added_at: Option<Timestamp>,
    pub plot: Option<String>,
    pub genre: Option<String>,
    /// External identity used to group versions of one film across sources.
    pub tmdb_id: Option<String>,
    pub imdb_id: Option<String>,
    pub duration_seconds: Option<i64>,
    pub stream: StreamLocator,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    /// "<source>|se|<key>"
    pub id: String,
    pub source_id: SourceId,
    pub title: String,
    pub year: Option<i64>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub category_id: Option<String>,
    pub rating: Option<f64>,
    pub added_at: Option<Timestamp>,
    pub plot: Option<String>,
    pub genre: Option<String>,
    pub tmdb_id: Option<String>,
    pub imdb_id: Option<String>,
    /// Xtream series id or add-on meta id; episodes are fetched on demand.
    pub remote_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    /// "<series>|ep|<key>"
    pub id: String,
    pub series_id: String,
    pub source_id: SourceId,
    pub season: i64,
    pub number: i64,
    pub title: String,
    pub plot: Option<String>,
    pub duration_seconds: Option<i64>,
    pub still_url: Option<String>,
    pub stream: StreamLocator,
    pub air_date: Option<Timestamp>,
}

// MARK: - Guide

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramFlags {
    pub is_new: bool,
    pub is_live: bool,
    pub is_premiere: bool,
    pub is_repeat: bool,
}

/// A broad kind of programme, read from its guide category, for colouring
/// the guide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProgramGenre {
    Sport,
    News,
    Film,
    Kids,
    /// Documentaries, science, nature and history.
    Factual,
    /// Series, comedy, drama, game and talk shows.
    Entertainment,
    Music,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Program {
    /// "<source>|<epg id>|<start>"
    pub id: String,
    pub source_id: SourceId,
    pub epg_id: String,
    pub start: Timestamp,
    pub end: Timestamp,
    pub title: String,
    pub subtitle: Option<String>,
    pub summary: Option<String>,
    pub category: Option<String>,
    pub image_url: Option<String>,
    /// Display form of the episode number, such as "S2 E5".
    pub episode: Option<String>,
    pub flags: ProgramFlags,
    pub age_rating: Option<String>,
    /// On a 10-point scale.
    pub star_rating: Option<f64>,
    pub year: Option<i64>,
    /// Read from `category` whenever the guide is read, so it's never stored.
    #[serde(default)]
    pub genre: Option<ProgramGenre>,
}

impl Program {
    pub fn make_id(source_id: &str, epg_id: &str, start: Timestamp) -> String {
        format!("{source_id}|{epg_id}|{start}")
    }

    pub fn is_airing(&self, now: Timestamp) -> bool {
        self.start <= now && now < self.end
    }
}

/// One channel's row in the guide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuideRow {
    pub channel: Channel,
    pub programs: Vec<Program>,
}

/// An XMLTV guide added on its own, beside the guides sources bring. It fills
/// in channels their own guides miss. Its link can carry a login, so the link
/// lives in the keychain like a source's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuideSource {
    /// "guide-<uuid>"
    pub id: String,
    pub name: String,
    /// Host shown in the UI ("epg.example"). Never holds credentials.
    pub display_host: Option<String>,
    /// Moves every programme by this many minutes. Applied when the guide loads.
    pub offset_minutes: i64,
    pub created_at: Timestamp,
    pub sort_index: i64,
}

/// An added guide as the interface lists it. Its status uses the guide fields
/// of [`SourceStatus`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuideSourceSummary {
    pub guide: GuideSource,
    pub status: SourceStatus,
    pub refreshing: bool,
}

/// What the "Add Guide" form sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewGuideSource {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub offset_minutes: i64,
}

/// A channel in one of the guides, offered when matching a channel by hand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuideChannel {
    /// The guide it's in: a source's id, or an added guide's.
    pub guide_id: String,
    /// The source's or added guide's name.
    pub guide_name: String,
    pub epg_id: String,
    /// Its first display name, when the guide gives one.
    pub name: Option<String>,
    /// What's on it now, to tell look-alike channels apart.
    pub now_title: Option<String>,
}

/// The guide channel a channel shows, and whether someone chose it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuideMatch {
    pub channel_id: String,
    /// `None` when no guide has the channel.
    pub current: Option<GuideChannel>,
    /// Chosen by hand: it wins over the channel's own guide id and name.
    pub chosen: bool,
}

// MARK: - Rich details

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CastMember {
    pub name: String,
    pub role: Option<String>,
    pub photo_url: Option<String>,
    /// TMDB's id for the person, when the cast came from TMDB, for their page.
    #[serde(default)]
    pub tmdb_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoFormat {
    pub width: i64,
    pub height: i64,
    pub codec: Option<String>,
    pub is_hdr: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonInfo {
    pub number: i64,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub cover_url: Option<String>,
    pub episode_count: Option<i64>,
}

/// Rich, optional metadata for a film or series beyond what a list carries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaDetails {
    pub backdrop_urls: Vec<String>,
    pub logo_url: Option<String>,
    pub people: Vec<CastMember>,
    pub director: Option<String>,
    pub creators: Vec<String>,
    pub release_date: Option<String>,
    pub country: Option<String>,
    pub age_rating: Option<String>,
    pub trailer_youtube_id: Option<String>,
    pub genres: Vec<String>,
    pub runtime_minutes: Option<i64>,
    pub imdb_id: Option<String>,
    pub tagline: Option<String>,
    pub overview: Option<String>,
    pub network: Option<String>,
    pub video: Option<VideoFormat>,
    pub audio_channels: Option<i64>,
    pub seasons: Vec<SeasonInfo>,
    /// Where extra details came from, for attribution ("TVmaze").
    pub attribution: Option<String>,
    /// Scores from where the details came (TMDB's vote average, an add-on's
    /// IMDb rating), best known first.
    #[serde(default)]
    pub ratings: Vec<TitleRating>,
    /// Trailers, teasers and clips on YouTube, the trailer first.
    #[serde(default)]
    pub videos: Vec<VideoLink>,
    /// What it's called where it was made, when that differs from its title.
    #[serde(default)]
    pub original_title: Option<String>,
}

/// Who gave a rating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RatingSource {
    Tmdb,
    Imdb,
}

/// A score out of 10, with how many voted when that's known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleRating {
    pub source: RatingSource,
    pub score: f64,
    pub votes: Option<i64>,
}

/// A video about a title on YouTube.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoLink {
    pub youtube_id: String,
    pub title: String,
    /// "Trailer", "Teaser", "Clip", "Featurette", "Behind the Scenes".
    pub kind: Option<String>,
}

/// What a poster in a list shows besides its picture: how many versions of
/// the film the sources carry, and whether (or how far) it's been watched.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleMark {
    pub id: String,
    /// Copies of the film across the sources (1 for a series).
    pub versions: i64,
    /// A version of the film, or every episode the source lists, finished.
    pub watched: bool,
    /// How far through, 0…1: the film's latest version part-watched, or a
    /// series' share of episodes finished. `None` when not started (or done).
    pub progress: Option<f64>,
}

/// One of the copies of a film the sources carry, for the Versions picker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleVersion {
    pub movie: Movie,
    pub source_name: String,
    /// The category it's listed in, as Xorvio names it.
    pub category_name: Option<String>,
    /// "4K", "FHD", "HD" or "SD", when the provider's title says.
    pub quality: Option<String>,
    /// The file's container ("mkv", "mp4"), when it's known.
    pub extension: Option<String>,
}

/// Someone from a film's or series' cast, from TMDB: who they are, and
/// what of theirs the sources carry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonDetails {
    pub tmdb_id: String,
    pub name: String,
    pub photo_url: Option<String>,
    pub biography: Option<String>,
    /// What they're known for ("Acting", "Directing").
    pub department: Option<String>,
    /// "1956-07-09".
    pub birthday: Option<String>,
    pub deathday: Option<String>,
    pub place_of_birth: Option<String>,
    /// Their films and series in the catalog, best known first.
    pub movies: Vec<Movie>,
    pub series: Vec<Series>,
    /// A line about them ("American actor"), when there's no biography.
    #[serde(default)]
    pub summary: Option<String>,
    /// Where the page came from, for credit: "TMDB" or "Wikidata".
    #[serde(default)]
    pub attribution: Option<String>,
    /// Who took the photo and its licence, when its licence asks for credit
    /// ("Gage Skidmore, CC BY-SA 3.0").
    #[serde(default)]
    pub photo_credit: Option<String>,
    /// The photo's own page, where its credit and licence are in full.
    #[serde(default)]
    pub photo_page: Option<String>,
}

/// Whether Xorvio can ask TMDB for details.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TmdbStatus {
    /// A key is in use (the person's own, or one built into this copy).
    pub configured: bool,
    /// The key in use is the person's own, from Settings.
    pub custom_key: bool,
    /// This copy has a key built in.
    pub built_in: bool,
    /// TMDB turned the key down since it was saved.
    pub rejected: bool,
}

// MARK: - Diagnostics

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    Info,
    Warning,
    Error,
}

/// One line of the diagnostic log (Settings › Diagnostics), already redacted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEvent {
    pub id: u64,
    /// Unix milliseconds.
    pub at: i64,
    pub level: DiagnosticLevel,
    /// "Sources", "Guide", "Playback", "Recording", "Network", "Services",
    /// "Add-ons", "Storage" or "App".
    pub area: String,
    pub message: String,
}

// MARK: - Library (user-authored state)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind {
    Channel,
    Movie,
    Series,
    Episode,
    Program,
    Recording,
}

/// Something saved to watch: a film, a series, or a programme from the guide.
/// Programmes keep a copy of their listing so the entry reads well after the
/// guide refreshes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistEntry {
    /// The film's or series' id, or the programme's id.
    pub id: String,
    pub kind: MediaKind,
    pub source_id: SourceId,
    pub title: String,
    pub subtitle: Option<String>,
    pub artwork_url: Option<String>,
    pub added_at: Timestamp,
    /// For programmes: the channel to tune to.
    pub channel_id: Option<String>,
    pub start: Option<Timestamp>,
    pub end: Option<Timestamp>,
    /// For programmes: whether a notification goes out before it starts.
    pub remind: bool,
}

/// Where someone got to in a film, episode or recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackProgress {
    pub item_id: String,
    pub kind: MediaKind,
    pub source_id: SourceId,
    /// For episodes, so Continue Watching can show the series.
    pub series_id: Option<String>,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub finished: bool,
    pub updated_at: Timestamp,
}

/// Something watched, for Viewing History (Settings › Privacy & Diagnostics).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub item_id: String,
    pub kind: MediaKind,
    pub source_id: String,
    pub title: String,
    /// "S01 E03 · What the Nets Bring"
    pub subtitle: Option<String>,
    pub watched_at: Timestamp,
}

/// A row of Continue Watching, ready to draw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueItem {
    pub item_id: String,
    pub kind: MediaKind,
    /// Where the film or episode comes from ("addon" for add-on titles).
    pub source_id: String,
    pub title: String,
    /// "S01 E03 · What the Nets Bring"
    pub subtitle: Option<String>,
    pub artwork_url: Option<String>,
    pub progress: f64,
    pub remaining_seconds: Option<f64>,
    /// The next episode rather than one in progress.
    pub up_next: bool,
    /// For an episode not started yet: why it's here, which its card says.
    #[serde(default)]
    pub reason: Option<UpNextReason>,
    pub updated_at: Timestamp,
}

/// Why Continue Watching offers an episode that hasn't been started, as
/// Xorvio for Mac labels its cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpNextReason {
    /// "New Episode": the next or latest one, aired in the last two weeks.
    NewEpisode,
    /// "Up Next": the one after the furthest watched.
    Next,
    /// "Latest Episode": nothing watched yet, and the series is airing.
    Latest,
    /// "Start Watching": nothing watched yet, from the first episode.
    Start,
}

// MARK: - Trakt

/// Trakt: whether it can be used, who's connected, and how the last sync went.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraktStatus {
    /// A Trakt app's client id and secret are available (built in, or entered in Settings).
    pub configured: bool,
    /// The app's credentials were entered in Settings rather than built in.
    pub custom_app: bool,
    pub connected: bool,
    /// The Trakt username, once connected.
    pub username: Option<String>,
    pub last_sync: Option<Timestamp>,
    /// Why the last sync failed, in plain words.
    pub last_error: Option<String>,
    pub syncing: bool,
    pub options: TraktOptions,
}

/// What Xorvio keeps the same on Trakt. All on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TraktOptions {
    /// Tell Trakt what's playing as it plays (start, pause, stop).
    pub scrobble: bool,
    /// Watched films and episodes, and places left part-way, both ways.
    pub sync_history: bool,
    /// The watchlist's films and series, both ways.
    pub sync_watchlist: bool,
}

impl Default for TraktOptions {
    fn default() -> Self {
        Self { scrobble: true, sync_history: true, sync_watchlist: true }
    }
}

/// A Trakt sign-in waiting for the code to be entered at trakt.tv/activate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraktSignInStart {
    pub user_code: String,
    pub verification_url: String,
    /// Seconds until the code expires.
    pub expires_in: i64,
    /// Seconds to wait between checks.
    pub interval: i64,
}

/// What the player tells Trakt as something plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraktScrobbleAction {
    Start,
    Pause,
    Stop,
}

impl TraktScrobbleAction {
    /// The last part of Trakt's scrobble address.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Pause => "pause",
            Self::Stop => "stop",
        }
    }
}

/// Films or shows, as Trakt's charts are split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TraktMedia {
    Movies,
    Shows,
}

/// One of Trakt's charts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TraktChart {
    /// Being watched right now.
    Trending,
    /// Most popular of all time.
    Popular,
    /// Most watched this week.
    Watched,
    /// This weekend's US box office (films only).
    BoxOffice,
}

/// What a Trakt row on Home shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TraktRowSource {
    Chart { chart: TraktChart, media: TraktMedia },
    /// A list, by its owner's slug and its own slug or id ("me" is the
    /// connected account).
    List { user: String, list: String },
    /// What Trakt recommends to the connected account.
    Recommended { media: TraktMedia },
}

/// A Trakt chart, list or recommendations, as far as it can be played.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraktRow {
    pub items: Vec<TraktRowItem>,
    /// How many films and series Trakt listed, playable or not.
    pub listed: i64,
}

/// A title in a Trakt row, with its place among the films and series Trakt
/// listed (from 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraktRowItem {
    pub rank: i64,
    #[serde(flatten)]
    pub title: TraktRowTitle,
}

/// A catalog title, or an add-on's when the sources don't carry it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TraktRowTitle {
    Movie(Movie),
    Series(Series),
}

/// A Trakt list that can become a row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraktListInfo {
    /// The owner's slug, as a row names the list by.
    pub user: String,
    /// The list's slug (or its id, for a list with none).
    pub list: String,
    pub name: String,
    pub description: Option<String>,
    /// Who made it, as Trakt shows them.
    pub owner: String,
    pub item_count: i64,
    pub likes: i64,
}

/// Which Trakt lists to show for choosing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TraktListQuery {
    /// The connected account's own lists.
    Mine,
    /// Lists the connected account has liked.
    Liked,
    /// The lists most liked on Trakt.
    Popular,
    /// Lists matching words, or the one a trakt.tv address names.
    Search { text: String },
}

// MARK: - A media folder's own library

/// What a video file holds, read once with ffprobe: for telling a media
/// folder's GIFs, animations and videos apart, and filtering by length.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileFacts {
    pub duration_seconds: Option<f64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub audio: bool,
    pub size_bytes: Option<i64>,
}

/// What a media folder's video is, from what its file holds: a GIF made
/// into a video (silent, short and small, as GIF sites save them), another
/// short silent clip (an animation), or a video (with sound, or longer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClipKind {
    Gif,
    Animation,
    Video,
}

/// The longest a GIF or an animation runs; anything longer is a video.
pub const LONGEST_CLIP_SECONDS: f64 = 30.0;
/// A GIF's picture is no bigger than this on its longer side...
pub const LARGEST_GIF_SIDE: i64 = 540;
/// ...or its bitrate is below this (bits a second).
pub const GIF_BITRATE: f64 = 600_000.0;

impl FileFacts {
    /// What the file is, once it's known how long it runs.
    pub fn kind(&self) -> Option<ClipKind> {
        let duration = self.duration_seconds.filter(|seconds| *seconds > 0.0)?;
        if self.audio || duration > LONGEST_CLIP_SECONDS {
            return Some(ClipKind::Video);
        }
        let small = matches!((self.width, self.height), (Some(width), Some(height)) if width.max(height) <= LARGEST_GIF_SIDE);
        let thin = self.size_bytes.is_some_and(|bytes| bytes as f64 * 8.0 / duration < GIF_BITRATE);
        Some(if small || thin { ClipKind::Gif } else { ClipKind::Animation })
    }
}

/// How long a media folder's videos to show: under a minute, or a minute or longer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClipLength {
    Short,
    Long,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum FolderSort {
    /// Newest first.
    #[default]
    Added,
    Name,
    /// Longest first.
    Length,
    /// Largest first.
    Size,
}

/// What of a media folder to show: one folder (or everything below it), filtered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderQuery {
    pub source_id: String,
    /// A folder inside the source's folders; none for the source's own.
    pub folder: Option<String>,
    /// Everything below the folder, not only what's directly in it.
    #[serde(default)]
    pub below: bool,
    pub kind: Option<ClipKind>,
    pub length: Option<ClipLength>,
    #[serde(default)]
    pub sort: FolderSort,
    pub limit: i64,
    pub offset: i64,
}

/// A video in a media folder, with what its file holds once that's been read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderVideo {
    pub movie: Movie,
    /// Its path below the source's folder ("imported/2026/clip.mp4").
    pub path: String,
    pub facts: Option<FileFacts>,
    pub kind: Option<ClipKind>,
}

/// A folder inside a media folder source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub name: String,
    /// Its full path, as the library's calls take it.
    pub path: String,
    /// Videos in it and below it.
    pub videos: i64,
}

/// A page of a media folder: the folders in it (on the first page) and its videos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderListing {
    /// The folder shown, none for the source's own (when it has several).
    pub folder: Option<String>,
    /// The way down to it from the source's own folder, that folder first.
    pub trail: Vec<FolderEntry>,
    pub folders: Vec<FolderEntry>,
    pub videos: Vec<FolderVideo>,
    /// Videos matching, on every page.
    pub total: i64,
    /// Videos here whose files haven't been read yet, which a type or length
    /// filter can't place until they are.
    pub unread: i64,
}

/// One time something was deleted from a media folder: it waits in the
/// folder's Recently Deleted, to be put back or deleted for good.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedBatch {
    pub id: String,
    /// The source's folder it was deleted from.
    pub root: String,
    pub deleted_at: Timestamp,
    pub entries: Vec<DeletedEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedEntry {
    /// Where it was, below the source's folder.
    pub path: String,
    pub folder: bool,
}

// MARK: - Channel management (user-authored)

/// A list of channels someone named ("Sports", "News"). Live TV shows each
/// under My Groups. The built-in Favorites list isn't one of these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteList {
    pub id: String,
    pub name: String,
    /// In the list's own order, hidden and paused channels included.
    pub channel_ids: Vec<String>,
}

/// What a renamed, hidden or locked item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditKind {
    Channel,
    LiveCategory,
    MovieCategory,
    SeriesCategory,
    /// A film hidden from the library.
    Movie,
    /// A series hidden from the library.
    Series,
}

/// A channel or category someone renamed, hid or locked, or one locked
/// because its name looks adult. The provider's own name is always kept, so
/// each change can be undone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemEdit {
    pub item_id: String,
    pub kind: EditKind,
    pub source_id: SourceId,
    /// The provider's name, tidied as everywhere else.
    pub original_name: String,
    /// The name someone gave it.
    pub display_name: Option<String>,
    pub hidden: bool,
    /// Kept behind the parental PIN.
    pub locked: bool,
    /// Locked only because its name looks adult; no one chose either way.
    pub locked_automatically: bool,
}

/// The parental PIN's state. Locked channels and categories are left out of
/// every list until the PIN is entered, which lasts until someone locks
/// again, the time chosen in Settings passes, or Xorvio quits.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentalStatus {
    pub has_pin: bool,
    pub unlocked: bool,
    /// Channels and categories behind the PIN.
    pub locked_count: i64,
    /// When an unlocked PIN locks again by itself; none while locked, or
    /// when it lasts until Xorvio quits.
    #[serde(default)]
    pub locks_at: Option<Timestamp>,
}

// MARK: - Search

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResults {
    pub channels: Vec<Channel>,
    pub movies: Vec<Movie>,
    pub series: Vec<Series>,
    pub programs: Vec<ProgramHit>,
    /// People the search names, with titles of theirs the sources carry.
    #[serde(default)]
    pub people: Vec<SearchPerson>,
    /// Why a film or series was found when its name isn't it ("With Tom
    /// Hanks", "Comedy", a few words of its story), by id.
    #[serde(default)]
    pub reasons: std::collections::BTreeMap<String, String>,
}

/// Someone a search names: in, or behind, films and series the sources carry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPerson {
    pub name: String,
    pub photo_url: Option<String>,
    pub tmdb_id: Option<String>,
    /// A title of theirs, which tells people of one name apart on their page.
    pub from_id: String,
    /// How many of the titles found they're in.
    pub titles: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramHit {
    pub program: Program,
    pub channel: Channel,
}

// MARK: - Playback

/// How the web view should play a resolved stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaybackMode {
    /// The `<video>` element plays it as is (MP4, WebM).
    Native,
    /// hls.js
    Hls,
    /// mpegts.js for raw MPEG-TS and FLV.
    Mpegts,
    /// The media gateway remuxes (or transcodes audio) into fragmented MP4 with ffmpeg.
    Remux,
}

/// A stream ready to hand to the player: everything goes through the local
/// media gateway, which adds headers, rewrites HLS playlists and handles CORS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedStream {
    /// A `http://127.0.0.1:<port>/…` gateway URL.
    pub url: String,
    pub mode: PlaybackMode,
    pub is_live: bool,
    /// Known length, for remuxed streams whose timeline the player can't read.
    pub duration_seconds: Option<f64>,
    /// Remuxed streams restart at an offset to seek; this is where this one starts.
    pub start_offset_seconds: f64,
    /// A remuxed stream's audio tracks, to switch between (other streams'
    /// tracks are the web view's own).
    #[serde(default)]
    pub audio_tracks: Vec<MediaTrack>,
    /// A remuxed stream's subtitles.
    #[serde(default)]
    pub subtitle_tracks: Vec<MediaTrack>,
    /// The audio track playing, in a remuxed stream.
    #[serde(default)]
    pub audio_index: Option<i64>,
    /// How far the audio is moved: positive plays it later.
    #[serde(default)]
    pub audio_delay_seconds: f64,
    /// The picture subtitle track drawn into the video, in a remuxed stream.
    #[serde(default)]
    pub burned_subtitle: Option<i64>,
    #[serde(default)]
    pub info: Option<StreamInfo>,
    /// A live channel buffered on this computer so it can be paused and
    /// rewound: HLS whose seekable range is the buffer. None for everything else.
    #[serde(default)]
    pub timeshift: Option<TimeshiftInfo>,
}

/// The live buffer behind a timeshifted channel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeshiftInfo {
    /// How far back the buffer reaches once it has filled (it starts empty).
    pub window_seconds: f64,
}

/// An audio or subtitle track in a remuxed stream, for the player's menus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaTrack {
    /// The track's index in the file.
    pub index: i64,
    pub codec: String,
    /// See [`language_code`].
    pub language: Option<String>,
    pub title: Option<String>,
    pub channels: Option<i64>,
    pub default: bool,
    pub forced: bool,
    /// Subtitles: where their WebVTT is served, growing as the video streams.
    /// None for picture subtitles (PGS, VobSub, DVB), which can only be
    /// drawn into the video (`StreamOptions::burn_subtitle`).
    pub url: Option<String>,
}

/// The parts of an episode worth skipping, in seconds from its start.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeSegments {
    pub intro: Option<Segment>,
    /// "Previously on…"
    pub recap: Option<Segment>,
    pub credits: Option<Segment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub start_seconds: f64,
    pub end_seconds: f64,
}

/// What a stream holds and what was done to it, for the player's stream information.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamInfo {
    /// "Matroska", "MP4", "MPEG-TS", "HLS"…
    pub container: String,
    /// "HEVC Main 10 · 10-bit"
    pub video: Option<String>,
    /// "E-AC-3 · 5.1"
    pub audio: Option<String>,
    /// "Audio converted to AAC", "Video encoded to H.264"…
    #[serde(default)]
    pub notes: Vec<String>,
}

/// A language tag as an ISO 639-2 bibliographic code ("eng", "fre", "ger"),
/// so tags from files ("fra", "deu"), add-ons ("en") and settings compare
/// equal. Tags it doesn't know pass through, lowercased; "und" is none.
pub fn language_code(tag: &str) -> Option<String> {
    let tag = tag.trim().to_ascii_lowercase();
    let tag = tag.split(['-', '_']).next().unwrap_or_default();
    if tag.is_empty() || tag == "und" || tag == "zxx" || tag == "mul" {
        return None;
    }
    const SAME: &[(&str, &str)] = &[
        ("en", "eng"), ("fr", "fre"), ("fra", "fre"), ("es", "spa"), ("de", "ger"), ("deu", "ger"),
        ("it", "ita"), ("pt", "por"), ("nl", "dut"), ("nld", "dut"), ("sv", "swe"), ("no", "nor"),
        ("nb", "nor"), ("nob", "nor"), ("da", "dan"), ("fi", "fin"), ("pl", "pol"), ("ru", "rus"),
        ("uk", "ukr"), ("tr", "tur"), ("el", "gre"), ("ell", "gre"), ("ar", "ara"), ("he", "heb"),
        ("hi", "hin"), ("ja", "jpn"), ("ko", "kor"), ("zh", "chi"), ("zho", "chi"), ("cs", "cze"),
        ("ces", "cze"), ("sk", "slo"), ("slk", "slo"), ("hu", "hun"), ("ro", "rum"), ("ron", "rum"),
        ("bg", "bul"), ("hr", "hrv"), ("sr", "srp"), ("sl", "slv"), ("fa", "per"), ("fas", "per"),
        ("th", "tha"), ("vi", "vie"), ("id", "ind"), ("ms", "may"), ("msa", "may"), ("is", "ice"),
        ("isl", "ice"), ("et", "est"), ("lv", "lav"), ("lt", "lit"), ("ca", "cat"), ("eu", "baq"),
        ("eus", "baq"), ("ga", "gle"), ("cy", "wel"), ("cym", "wel"), ("sq", "alb"), ("sqi", "alb"),
        ("mk", "mac"), ("mkd", "mac"), ("hy", "arm"), ("hye", "arm"), ("ka", "geo"), ("kat", "geo"),
        ("ta", "tam"), ("te", "tel"), ("bn", "ben"), ("ur", "urd"), ("pb", "por"), ("pob", "por"),
    ];
    Some(SAME.iter().find(|(from, _)| *from == tag).map_or(tag, |(_, to)| to).to_string())
}

/// A concrete address to open, with the headers the provider expects. Never
/// shown in the interface or written to logs: it can hold credentials.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamTarget {
    pub url: String,
    pub user_agent: Option<String>,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub is_live: bool,
}

// MARK: - Recording

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingStatus {
    Scheduled,
    Recording,
    Completed,
    /// Ended early or recorded only part of the time (the stream dropped and
    /// couldn't be reopened for a while). The file is still playable.
    Partial,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recording {
    pub id: String,
    pub source_id: SourceId,
    pub channel_id: String,
    pub channel_name: String,
    pub channel_logo_url: Option<String>,
    pub title: String,
    pub subtitle: Option<String>,
    pub summary: Option<String>,
    pub artwork_url: Option<String>,
    /// The guide programme it was scheduled from, when there was one.
    pub program_id: Option<String>,
    /// The programme's own times; recording starts `padding_before` earlier
    /// and stops `padding_after` later.
    pub start: Timestamp,
    pub end: Timestamp,
    pub padding_before_seconds: i64,
    pub padding_after_seconds: i64,
    pub status: RecordingStatus,
    /// Set once recording starts.
    pub file_path: Option<String>,
    pub file_size_bytes: Option<i64>,
    /// Seconds actually captured so far.
    pub recorded_seconds: Option<f64>,
    pub error: Option<String>,
    pub created_at: Timestamp,
    /// The series recording that scheduled it, when one did.
    #[serde(default)]
    pub rule_id: Option<String>,
    /// The guide's episode number ("S2 E5"), when it gives one.
    #[serde(default)]
    pub episode: Option<String>,
    /// Saved from the provider's catch-up after it aired, rather than
    /// recorded as it went out.
    #[serde(default)]
    pub from_catchup: bool,
    /// Something worth knowing that isn't a fault, such as being saved in
    /// another folder because the chosen one wasn't there.
    #[serde(default)]
    pub note: Option<String>,
    /// Set on a scheduled or running recording while more recordings from
    /// its source overlap it than the provider allows streams at a time.
    /// Worked out each time recordings are listed; never stored.
    #[serde(default)]
    pub conflict: Option<String>,
}

/// "Record every episode of …": a programme's title on one channel, or on
/// any. The recorder schedules each matching airing the guide lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesRule {
    pub id: String,
    pub title: String,
    pub source_id: SourceId,
    /// The channel it was made on, which it records from unless `any_channel`.
    pub channel_id: String,
    pub channel_name: String,
    pub channel_logo_url: Option<String>,
    pub artwork_url: Option<String>,
    #[serde(flatten)]
    pub options: SeriesRuleOptions,
    pub created_at: Timestamp,
    /// When the next scheduled airing starts. Worked out when listed, as are
    /// the counts below.
    pub next_airing: Option<Timestamp>,
    pub next_channel_name: Option<String>,
    pub scheduled_count: i64,
    /// Finished recordings still kept.
    pub recorded_count: i64,
    /// Scheduled airings that clash with others for the provider's streams.
    pub conflict_count: i64,
}

/// What can be chosen about a series recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesRuleOptions {
    /// Every channel showing the title, not only the one it was made on.
    pub any_channel: bool,
    /// Skips airings the guide calls repeats and episodes already recorded.
    pub new_only: bool,
    /// Finished recordings to keep: 0 for all, otherwise the latest this many.
    pub keep: i64,
    /// `None` follows Settings › Recording.
    pub padding_before_seconds: Option<i64>,
    pub padding_after_seconds: Option<i64>,
}

impl Default for SeriesRuleOptions {
    fn default() -> Self {
        Self { any_channel: false, new_only: true, keep: 0, padding_before_seconds: None, padding_after_seconds: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRequest {
    pub source_id: SourceId,
    pub channel_id: String,
    pub channel_name: String,
    pub channel_logo_url: Option<String>,
    pub title: String,
    pub subtitle: Option<String>,
    pub summary: Option<String>,
    pub artwork_url: Option<String>,
    pub program_id: Option<String>,
    pub start: Timestamp,
    pub end: Timestamp,
    pub padding_before_seconds: i64,
    pub padding_after_seconds: i64,
}

// MARK: - Add-ons (Stremio protocol)

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonCatalogInfo {
    pub media_type: String,
    pub id: String,
    pub name: Option<String>,
    pub genres: Vec<String>,
    pub supports_search: bool,
    /// Can be listed with no extra parameter.
    pub browsable: bool,
}

/// An installed add-on. Its manifest link can hold an account key, so only
/// the host is ever shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonInfo {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub logo_url: Option<String>,
    pub host: String,
    pub enabled: bool,
    pub types: Vec<String>,
    /// "catalog", "meta", "stream", "subtitles"
    pub resources: Vec<String>,
    pub catalogs: Vec<AddonCatalogInfo>,
}

/// One playable copy offered by a stream add-on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonStream {
    pub addon_id: String,
    pub addon_name: String,
    /// The add-on's own label, first line.
    pub name: String,
    pub description: Option<String>,
    /// Read from the label: "4K", "1080p"…
    pub quality: Option<String>,
    pub size_bytes: Option<i64>,
    /// "HDR", "DV", "Atmos", "5.1"…
    pub tags: Vec<String>,
    /// A direct address. Copies that need a torrent engine have none and are
    /// listed as unavailable.
    pub target: Option<StreamTarget>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonSubtitle {
    pub addon_name: String,
    pub language: String,
    pub url: String,
}

/// A row of titles from an add-on catalog (Discover, Home).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonCatalogRow {
    pub addon_id: String,
    pub catalog_id: String,
    pub media_type: String,
    pub title: String,
    pub movies: Vec<Movie>,
    pub series: Vec<Series>,
}

/// A title's full record from a meta add-on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonMeta {
    pub movie: Option<Movie>,
    pub series: Option<Series>,
    pub episodes: Vec<Episode>,
    pub details: MediaDetails,
}

// MARK: - Library profiles

/// A language or region a category's name can say: a code ("US|", "EN -",
/// "QC - FILMS") or a word ("TURKISH SERIES").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentLanguage {
    English,
    French,
    Spanish,
    Portuguese,
    German,
    Italian,
    Dutch,
    Nordic,
    Polish,
    Turkish,
    Arabic,
    SouthAsian,
    Russian,
    Ukrainian,
    Greek,
    Albanian,
    Balkan,
    Hungarian,
    Romanian,
    CzechSlovak,
    Bulgarian,
    Persian,
    Hebrew,
    Korean,
    Japanese,
    Chinese,
    Filipino,
    Vietnamese,
    Thai,
    African,
}

/// A genre a category's name can say ("EN - ACTION", "24/7 COMEDY").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentGenre {
    Action,
    Adventure,
    Animation,
    Comedy,
    Crime,
    Documentary,
    Drama,
    Family,
    Fantasy,
    Horror,
    Kids,
    Music,
    Mystery,
    News,
    Reality,
    Romance,
    ScienceFiction,
    Sports,
    Thriller,
    Western,
    Faith,
    Holiday,
    Anime,
}

/// A saved view of the library ("English only", "Action"): which languages,
/// genres and kinds of content to show. The active profile applies to every
/// list at once; nothing is deleted, and the watchlist, favourites and
/// history are shared by all profiles. Categories are judged by their names,
/// so a category naming no language or genre stays unless a rule says so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LibraryProfile {
    pub id: String,
    pub name: String,
    /// Only categories in one of these languages. Empty: every language.
    pub languages: Vec<ContentLanguage>,
    /// With languages chosen, categories that name none ("NETFLIX MOVIES") stay.
    pub include_unlabeled: bool,
    /// Categories whose names look adult are left out. A parental PIN locks
    /// them whatever this says.
    pub hide_adult: bool,
    /// Favourite genres: each gets a row on Home.
    pub genres: Vec<ContentGenre>,
    /// Categories naming other genres are left out; those naming none stay,
    /// since they could hold anything.
    pub only_genres: bool,
    /// Live TV, films or series left out entirely.
    pub hidden_kinds: Vec<CategoryKind>,
    /// Categories shown whatever the rules above say (never past a PIN).
    pub included_categories: Vec<String>,
    /// Categories left out whatever the rules above say.
    pub excluded_categories: Vec<String>,
}

impl Default for LibraryProfile {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            languages: Vec::new(),
            include_unlabeled: true,
            hide_adult: false,
            genres: Vec::new(),
            only_genres: false,
            hidden_kinds: Vec::new(),
            included_categories: Vec::new(),
            excluded_categories: Vec::new(),
        }
    }
}

/// Every profile in the user's order, and which one is in use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profiles {
    pub profiles: Vec<LibraryProfile>,
    pub active_id: String,
}

/// A category with what its name says about it and how much it holds, for
/// the profile editor's counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCategory {
    pub id: String,
    pub source_id: SourceId,
    pub kind: CategoryKind,
    /// As shown everywhere: someone's own name, or the provider's tidied.
    pub name: String,
    /// Channels, films or series in it.
    pub item_count: i64,
    pub languages: Vec<ContentLanguage>,
    pub genres: Vec<ContentGenre>,
    pub adult: bool,
}
