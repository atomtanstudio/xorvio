// TypeScript twins of crates/xorvio-model/src/lib.rs. Change both together.
// Times are Unix seconds.

export type SourceId = string;
export type Timestamp = number;

export type SourceKind = "m3uUrl" | "m3uFile" | "xtream" | "demo" | "folder" | "plex" | "jellyfin" | "emby";
export type LiveStreamFormat = "hls" | "mpegts";
export type RefreshInterval = "manual" | "every6Hours" | "daily";

export interface SourceConfig {
  id: SourceId;
  name: string;
  kind: SourceKind;
  displayHost: string | null;
  filePath: string | null;
  /** A media folder source's folders: on this computer, a drive, or a network share. */
  folders?: string[];
  isPaused: boolean;
  liveFormat: LiveStreamFormat;
  refreshInterval: RefreshInterval;
  userAgent: string | null;
  createdAt: Timestamp;
  sortIndex: number;
  /** Moves every programme in the source's guide; applied when the guide loads. */
  guideOffsetMinutes: number;
}

export interface SourceSecrets {
  playlistUrl?: string | null;
  epgUrl?: string | null;
  serverUrl?: string | null;
  username?: string | null;
  password?: string | null;
  /** A media server's token: sent when connecting to Plex by address, never read back. */
  token?: string | null;
}

export interface SourceCapabilities {
  live: boolean;
  movies: boolean;
  series: boolean;
  guide: boolean;
  replay: boolean;
}

export interface SourceStatus {
  lastSuccess: Timestamp | null;
  lastAttempt: Timestamp | null;
  lastError: string | null;
  guideLastSuccess: Timestamp | null;
  guideLastError: string | null;
  channelCount: number;
  movieCount: number;
  seriesCount: number;
  programCount: number;
  capabilities: SourceCapabilities;
  maxConnections: number | null;
  accountExpires: Timestamp | null;
  authenticationExpired: boolean;
  serverTimeZone: string | null;
  catalogWarning: string | null;
}

export interface SourceSummary {
  config: SourceConfig;
  status: SourceStatus;
  refreshing: boolean;
}

export interface NewSource {
  name: string;
  kind: SourceKind;
  secrets: SourceSecrets;
  filePath: string | null;
  liveFormat: LiveStreamFormat;
  userAgent: string | null;
}

export interface RefreshProgress {
  sourceId: SourceId;
  phase: string;
  fraction: number | null;
}

export type CategoryKind = "live" | "movie" | "series";

export type StreamLocator =
  | { type: "url"; url: string }
  | { type: "xtream"; path: "live" | "movie" | "series"; streamId: string; extension: string }
  | { type: "file"; path: string }
  | { type: "addon"; mediaType: string; mediaId: string }
  | { type: "server"; key: string; part: string | null; extension: string | null };

/** Trakt: whether it can be used, who's connected, and how the last sync went. */
export interface TraktStatus {
  /** A Trakt app's client id is available (built in, or entered in Settings). */
  configured: boolean;
  /** The app's client id was entered in Settings rather than built in. */
  customApp: boolean;
  connected: boolean;
  /** The Trakt username, once connected. */
  username: string | null;
  lastSync: Timestamp | null;
  /** Why the last sync failed, in plain words. */
  lastError: string | null;
  syncing: boolean;
  options: TraktOptions;
}

export interface TraktOptions {
  /** Tell Trakt what's playing as it plays (start, pause, stop). */
  scrobble: boolean;
  /** Watched films and episodes, and places left part-way, both ways. */
  syncHistory: boolean;
  /** The watchlist's films and series, both ways. */
  syncWatchlist: boolean;
}

/** A Trakt sign-in waiting for the code to be entered at trakt.tv/activate. */
export interface TraktSignInStart {
  userCode: string;
  verificationUrl: string;
  /** Seconds until the code expires. */
  expiresIn: number;
  /** Seconds to wait between checks. */
  interval: number;
}

export type TraktScrobbleAction = "start" | "pause" | "stop";

export type TraktMedia = "movies" | "shows";

/** One of Trakt's charts: being watched now, most popular of all time, most watched this week, and this weekend's US box office (films only). */
export type TraktChart = "trending" | "popular" | "watched" | "boxOffice";

/** What a Trakt row on Home shows. A list is named by its owner's slug (empty for an official list) and its own id or slug. */
export type TraktRowSource =
  | { type: "chart"; chart: TraktChart; media: TraktMedia }
  | { type: "list"; user: string; list: string }
  | { type: "recommended"; media: TraktMedia };

/** A Trakt chart, list or recommendations, as far as it can be played. */
export interface TraktRow {
  items: TraktRowItem[];
  /** How many films and series Trakt listed, playable or not. */
  listed: number;
}

/** A title in a Trakt row (a catalog title, or an add-on's when the sources don't carry it), with its place among the films and series Trakt listed, from 1. */
export type TraktRowItem = { rank: number } & ({ movie: Movie } | { series: Series });

/** A Trakt row as Home keeps it: what it shows, its title, and who made a list, kept for when Trakt can't be asked. */
export interface TraktRowRef {
  source: TraktRowSource;
  title: string;
  owner?: string;
}

/** A Trakt list that can become a row. */
export interface TraktListInfo {
  user: string;
  list: string;
  name: string;
  description: string | null;
  /** Who made it, as Trakt shows them. */
  owner: string;
  itemCount: number;
  likes: number;
}

/** Which Trakt lists to show for choosing: the account's own, those it liked, the most liked, or those matching words (or the one a trakt.tv address names). */
export type TraktListQuery = { type: "mine" } | { type: "liked" } | { type: "popular" } | { type: "search"; text: string };

/** A Plex sign-in waiting for approval on plex.tv. */
export interface PlexSignInStart {
  pinId: number;
  code: string;
  /** The page to open in the browser. */
  authUrl: string;
}

/** A server the signed-in Plex account can add. */
export interface PlexServerChoice {
  id: string;
  name: string;
  /** The account's own server, rather than one shared with it. */
  owned: boolean;
}

export interface Category {
  id: string;
  sourceId: SourceId;
  kind: CategoryKind;
  name: string;
  sortIndex: number;
}

export interface Channel {
  id: string;
  sourceId: SourceId;
  name: string;
  number: number | null;
  logoUrl: string | null;
  categoryId: string | null;
  epgId: string | null;
  stream: StreamLocator;
  catchupDays: number;
  catchupSource: string | null;
  sortIndex: number;
  /** Minutes this channel's replays are moved by (its catch-up offset); 0 for none. */
  catchupOffsetMinutes: number;
  /**
   * With similar channels grouped, every version this row stands for, in the
   * provider's order, this channel (the one that plays) among them. Missing
   * for a channel listed once.
   */
  versions?: ChannelVersion[];
}

/** One version of a channel the provider lists several times. */
export interface ChannelVersion {
  id: string;
  /** The provider's name for it ("US: News 24 FHD"), or the one someone gave it. */
  name: string;
  /** "4K", "FHD", "HD" or "SD", as the name says. */
  quality: string | null;
  /** Someone chose to always play this version. */
  preferred: boolean;
}

export interface Movie {
  id: string;
  sourceId: SourceId;
  title: string;
  year: number | null;
  posterUrl: string | null;
  backdropUrl: string | null;
  categoryId: string | null;
  rating: number | null;
  addedAt: Timestamp | null;
  plot: string | null;
  genre: string | null;
  tmdbId: string | null;
  imdbId: string | null;
  durationSeconds: number | null;
  stream: StreamLocator;
}

export interface Series {
  id: string;
  sourceId: SourceId;
  title: string;
  year: number | null;
  posterUrl: string | null;
  backdropUrl: string | null;
  categoryId: string | null;
  rating: number | null;
  addedAt: Timestamp | null;
  plot: string | null;
  genre: string | null;
  tmdbId: string | null;
  imdbId: string | null;
  remoteId: string | null;
}

export interface Episode {
  id: string;
  seriesId: string;
  sourceId: SourceId;
  season: number;
  number: number;
  title: string;
  plot: string | null;
  durationSeconds: number | null;
  stillUrl: string | null;
  stream: StreamLocator;
  airDate: Timestamp | null;
}

export interface ProgramFlags {
  isNew: boolean;
  isLive: boolean;
  isPremiere: boolean;
  isRepeat: boolean;
}

export interface Program {
  id: string;
  sourceId: SourceId;
  epgId: string;
  start: Timestamp;
  end: Timestamp;
  title: string;
  subtitle: string | null;
  summary: string | null;
  category: string | null;
  imageUrl: string | null;
  episode: string | null;
  flags: ProgramFlags;
  ageRating: string | null;
  starRating: number | null;
  year: number | null;
  /** Read from `category`, for colouring the guide. */
  genre: ProgramGenre | null;
}

/** A programme's broad kind, read from its guide category in several languages. */
export type ProgramGenre = "sport" | "news" | "film" | "kids" | "factual" | "entertainment" | "music";

export interface GuideRow {
  channel: Channel;
  programs: Program[];
}

/** An XMLTV guide added on its own. Its link stays in the keychain. */
export interface GuideSource {
  id: string;
  name: string;
  displayHost: string | null;
  offsetMinutes: number;
  createdAt: Timestamp;
  sortIndex: number;
}

/** The status uses SourceStatus's guide fields. */
export interface GuideSourceSummary {
  guide: GuideSource;
  status: SourceStatus;
  refreshing: boolean;
}

export interface NewGuideSource {
  name: string;
  url: string;
  offsetMinutes: number;
}

/** A channel in one of the guides, offered when matching a channel by hand. */
export interface GuideChannel {
  guideId: string;
  guideName: string;
  epgId: string;
  name: string | null;
  nowTitle: string | null;
}

export interface GuideMatch {
  channelId: string;
  current: GuideChannel | null;
  /** Chosen by hand: it wins over the channel's own guide id and name. */
  chosen: boolean;
}

export interface CastMember {
  name: string;
  role: string | null;
  photoUrl: string | null;
  /** TMDB's id for the person, when the cast came from TMDB, for their page. */
  tmdbId: string | null;
}

/** Someone from a cast, from TMDB: who they are, and what of theirs the sources carry. */
export interface PersonDetails {
  tmdbId: string;
  name: string;
  photoUrl: string | null;
  biography: string | null;
  /** What they're known for ("Acting", "Directing"). */
  department: string | null;
  /** "1956-07-09". */
  birthday: string | null;
  deathday: string | null;
  placeOfBirth: string | null;
  /** Their films and series in the catalog, best known first. */
  movies: Movie[];
  series: Series[];
  /** A line about them ("Canadian actor"), when there's no biography. */
  summary: string | null;
  /** Where the page came from: "TMDB" or "Wikidata". */
  attribution: string | null;
  /** Who took the photo and its licence, when its licence asks for credit ("Gage Skidmore, CC BY-SA 3.0"). */
  photoCredit?: string | null;
  /** The photo's own page, with its credit and licence in full. */
  photoPage?: string | null;
}

/** Whether Xorvio can ask TMDB for details. */
export interface TmdbStatus {
  /** A key is in use (the person's own, or one built into this copy). */
  configured: boolean;
  /** The key in use is the person's own, from Settings. */
  customKey: boolean;
  /** This copy has a key built in. */
  builtIn: boolean;
  /** TMDB turned the key down since it was saved. */
  rejected: boolean;
}

/** Something watched, for Viewing History. */
export interface HistoryEntry {
  itemId: string;
  kind: MediaKind;
  sourceId: string;
  title: string;
  /** "S01 E03 · What the Nets Bring" */
  subtitle: string | null;
  watchedAt: number;
}

export type DiagnosticLevel = "info" | "warning" | "error";

/** One line of the diagnostic log (Settings › Diagnostics), already redacted. */
export interface DiagnosticEvent {
  id: number;
  /** Unix milliseconds. */
  at: number;
  level: DiagnosticLevel;
  /** "Sources", "Guide", "Playback", "Recording", "Network", "Services", "Add-ons", "Storage" or "App". */
  area: string;
  message: string;
}

export interface VideoFormat {
  width: number;
  height: number;
  codec: string | null;
  isHdr: boolean;
}

export interface SeasonInfo {
  number: number;
  name: string | null;
  overview: string | null;
  airDate: string | null;
  coverUrl: string | null;
  episodeCount: number | null;
}

export interface MediaDetails {
  backdropUrls: string[];
  logoUrl: string | null;
  people: CastMember[];
  director: string | null;
  creators: string[];
  releaseDate: string | null;
  country: string | null;
  ageRating: string | null;
  trailerYoutubeId: string | null;
  genres: string[];
  runtimeMinutes: number | null;
  imdbId: string | null;
  tagline: string | null;
  overview: string | null;
  network: string | null;
  video: VideoFormat | null;
  audioChannels: number | null;
  seasons: SeasonInfo[];
  attribution: string | null;
  /** Scores from where the details came (TMDB's users, an add-on's IMDb rating). */
  ratings: TitleRating[];
  /** Trailers, teasers and clips on YouTube, the trailer first. */
  videos: VideoLink[];
  /** What it's called where it was made, when that differs. */
  originalTitle: string | null;
}

export type RatingSource = "tmdb" | "imdb";

/** A score out of 10. */
export interface TitleRating {
  source: RatingSource;
  score: number;
  votes: number | null;
}

export interface VideoLink {
  youtubeId: string;
  title: string;
  /** "Trailer", "Teaser", "Clip", "Featurette". */
  kind: string | null;
}

/** What a poster in a list shows besides its picture. */
export interface TitleMark {
  id: string;
  /** Copies of the film across the sources (1 for a series). */
  versions: number;
  /** A copy of the film, or every episode the source lists, finished. */
  watched: boolean;
  /** How far through, 0…1: the film part-watched, or a series' share of episodes finished. */
  progress: number | null;
}

/** One of the copies of a film the sources carry, for the Versions picker. */
export interface TitleVersion {
  movie: Movie;
  sourceName: string;
  categoryName: string | null;
  /** "4K", "FHD", "HD" or "SD", when the provider's title says. */
  quality: string | null;
  /** The file's container ("mkv", "mp4"). */
  extension: string | null;
}

export type MediaKind = "channel" | "movie" | "series" | "episode" | "program" | "recording";

export interface WatchlistEntry {
  id: string;
  kind: MediaKind;
  sourceId: SourceId;
  title: string;
  subtitle: string | null;
  artworkUrl: string | null;
  addedAt: Timestamp;
  channelId: string | null;
  start: Timestamp | null;
  end: Timestamp | null;
  remind: boolean;
}

export interface PlaybackProgress {
  itemId: string;
  kind: MediaKind;
  sourceId: SourceId;
  seriesId: string | null;
  positionSeconds: number;
  durationSeconds: number;
  finished: boolean;
  updatedAt: Timestamp;
}

export interface ContinueItem {
  itemId: string;
  kind: MediaKind;
  /** "addon" for add-on titles. */
  sourceId: string;
  title: string;
  subtitle: string | null;
  artworkUrl: string | null;
  progress: number;
  remainingSeconds: number | null;
  upNext: boolean;
  /** For an episode not started yet: why it's here, which its card says. */
  reason?: UpNextReason | null;
  updatedAt: Timestamp;
}

/** "New Episode", "Up Next", "Latest Episode" or "Start Watching", as Xorvio for Mac labels them. */
export type UpNextReason = "newEpisode" | "next" | "latest" | "start";

/** Whether the system lets Xorvio show notifications (Settings › Reminders). */
export type NotificationPermission = "allowed" | "denied" | "notDetermined" | "unknown";

/** A list of channels someone named ("Sports"), shown under My Groups. Favorites isn't one. */
export interface FavoriteList {
  id: string;
  name: string;
  /** In the list's order, hidden and paused channels included. */
  channelIds: string[];
}

/** The list id of the built-in Favorites. */
export const FAVORITES_LIST = "default";

export type EditKind = "channel" | "liveCategory" | "movieCategory" | "seriesCategory" | "movie" | "series";

/** A channel or category someone renamed, hid or locked, or one locked because its name looks adult. */
export interface ItemEdit {
  itemId: string;
  kind: EditKind;
  sourceId: string;
  /** The provider's name, tidied. */
  originalName: string;
  displayName: string | null;
  hidden: boolean;
  locked: boolean;
  /** Locked only because its name looks adult. */
  lockedAutomatically: boolean;
}

export interface ParentalStatus {
  hasPin: boolean;
  /** The PIN was entered, and hasn't locked again yet. */
  unlocked: boolean;
  lockedCount: number;
  /** When an entered PIN locks again by itself; null while locked, or when it lasts until Xorvio quits. */
  locksAt?: Timestamp | null;
}

/** What a provider says about an Xtream account, for Test Connection. Never the login. */
export interface XtreamAccountCheck {
  maxConnections: number | null;
  expires: Timestamp | null;
}

/** What the menu bar's items can do now (src-tauri/src/menu.rs). The main window sends `app`; the window the video is in sends `playback`. */
export interface MenuState {
  app?: { hasSources: boolean; canGoBack: boolean; sidebarHidden: boolean; multiview: boolean };
  playback?: {
    playing: boolean;
    paused: boolean;
    muted: boolean;
    seekable: boolean;
    behindLive: boolean;
    channel: boolean;
    lastChannel: boolean;
    multiview: boolean;
    poppedOut: boolean;
  };
}

export interface ProgramHit {
  program: Program;
  channel: Channel;
}

export interface SearchResults {
  channels: Channel[];
  movies: Movie[];
  series: Series[];
  programs: ProgramHit[];
  /** People the search names, with titles of theirs the sources carry. */
  people: SearchPerson[];
  /** Why a film or series was found when its name isn't it ("With Tom Hanks", "Comedy", a few words of its story), by id. */
  reasons: Record<string, string>;
}

/** Someone a search names: in, or behind, films and series the sources carry. */
export interface SearchPerson {
  name: string;
  photoUrl: string | null;
  tmdbId: string | null;
  /** A title of theirs, which tells people of one name apart on their page. */
  fromId: string;
  /** How many of the titles found they're in. */
  titles: number;
}

export type PlaybackMode = "native" | "hls" | "mpegts" | "remux";

/** What the web view's Media Source decodes; remuxing copies these as they are. */
export interface PlaybackSupport {
  hevc: boolean;
  ac3: boolean;
  eac3: boolean;
  /** The <video> element plays VP9 and AV1 files by itself (AV1 needs the hardware to). */
  vp9: boolean;
  av1: boolean;
}

/** Another player on this computer a stream can be handed to (IINA, VLC, Infuse). */
export interface ExternalPlayer {
  id: string;
  name: string;
}

/** A Chromecast or Google TV found on the network. */
export interface CastDevice {
  id: string;
  /** The name its owner gave it ("Living Room TV"). */
  name: string;
  model: string | null;
}

export type CastState = "ready" | "buffering" | "playing" | "paused" | "finished" | "stopped" | "failed";

/** What a TV is doing, in the title's own time. */
export interface CastStatus {
  deviceId: string;
  deviceName: string;
  state: CastState;
  /** Where the title is when this was sent; it moves on by itself while playing. */
  positionSeconds: number;
  durationSeconds: number | null;
  live: boolean;
  canSeek: boolean;
  /** The TV's volume, 0 to 1. */
  volume: number;
  muted: boolean;
  /** Why it stopped or failed. */
  message: string | null;
}

/** What the TV shows about the title. */
export interface CastDetails {
  title: string;
  /** "S01 E03 · Name", the year, or the programme on a channel. */
  subtitle: string | null;
  artworkUrl: string | null;
}

export interface ResolvedStream {
  url: string;
  mode: PlaybackMode;
  isLive: boolean;
  durationSeconds: number | null;
  startOffsetSeconds: number;
  /** A remuxed stream's audio tracks (other streams' are the web view's own). */
  audioTracks: MediaTrack[];
  subtitleTracks: MediaTrack[];
  audioIndex: number | null;
  /** Positive plays the audio later. */
  audioDelaySeconds: number;
  /** The picture subtitle track drawn into the video, in a remuxed stream. */
  burnedSubtitle?: number | null;
  info: StreamInfo | null;
  /** A live channel playing from a buffer kept on disk, which can pause and seek. */
  timeshift?: { windowSeconds: number } | null;
}

export interface MediaTrack {
  index: number;
  codec: string;
  /** ISO 639-2 ("eng", "fre"). */
  language: string | null;
  title: string | null;
  channels: number | null;
  default: boolean;
  forced: boolean;
  /** Subtitles: the WebVTT address; none for picture subtitles. */
  url: string | null;
}

export interface StreamInfo {
  container: string;
  video: string | null;
  audio: string | null;
  notes: string[];
}

/** Audio and subtitles chosen while watching a series, remembered for its other episodes (track_choices.rs). */
export interface TrackChoice {
  /** The audio language chosen, ISO 639-2 ("spa"). */
  audioLanguage?: string | null;
  /** Off, or a track by its language and the name the player showed; none leaves them to the settings. */
  subtitles?: { kind: "off" } | { kind: "track"; language: string | null; label: string } | null;
}

/** Changes to a playing stream; it restarts where it is. */
export interface StreamOptions {
  audioIndex?: number | null;
  audioDelaySeconds?: number | null;
  /** A picture subtitle track (PGS, VobSub, DVB) to draw into the video, which encodes it; -1 stops. */
  burnSubtitle?: number | null;
}

export interface Segment {
  startSeconds: number;
  endSeconds: number;
}

/** A title's frames for the seek bar: sheets of tiles, each read left to right and top to bottom. */
export interface ThumbnailSheet {
  /** The sheets' addresses, in order. */
  images: string[];
  /** Tile `k` shows the title about `(k + 0.5) × intervalSeconds` in. */
  intervalSeconds: number;
  count: number;
  /** Tiles across and down a full sheet; the last sheet can have fewer rows. */
  columns: number;
  rows: number;
  /** One tile's size in pixels. */
  width: number;
  height: number;
}

/** A title's seek-bar thumbnails: ready, being made (ask again shortly), or not to be had. */
export type Thumbnails = { state: "ready"; sheet: ThumbnailSheet } | { state: "making" } | { state: "unavailable"; reason: string };

/** Adverts in a recording, in seconds from its start (as the player counts them). */
export interface AdBreak {
  startSeconds: number;
  endSeconds: number;
}

/** What's known of a recording's ad breaks. Breaks found before stay while they're looked for again. */
export interface AdBreaks {
  state: "never" | "finding" | "found" | "failed";
  breaks: AdBreak[];
  /** "comskip" or "Xorvio", once found. */
  foundBy: string | null;
  error: string | null;
}

/** What to skip in an episode, from IntroDB. */
export interface EpisodeSegments {
  intro: Segment | null;
  recap: Segment | null;
  credits: Segment | null;
}

export type RecordingStatus = "scheduled" | "recording" | "completed" | "partial" | "failed" | "cancelled";

export interface Recording {
  id: string;
  sourceId: SourceId;
  channelId: string;
  channelName: string;
  channelLogoUrl: string | null;
  title: string;
  subtitle: string | null;
  summary: string | null;
  artworkUrl: string | null;
  programId: string | null;
  start: Timestamp;
  end: Timestamp;
  paddingBeforeSeconds: number;
  paddingAfterSeconds: number;
  status: RecordingStatus;
  filePath: string | null;
  fileSizeBytes: number | null;
  recordedSeconds: number | null;
  error: string | null;
  createdAt: Timestamp;
  /** The series recording that scheduled it, when one did. */
  ruleId: string | null;
  /** The guide's episode number ("S2 E5"), when it gives one. */
  episode: string | null;
  /** Saved from the provider's catch-up after it aired. */
  fromCatchup: boolean;
  /** Worth knowing but not a fault, such as being saved in another folder because the chosen one wasn't there. */
  note: string | null;
  /** Set while more recordings from its source overlap it than the provider allows streams at a time. */
  conflict: string | null;
}

/** What can be chosen about a series recording. */
export interface SeriesRuleOptions {
  /** Every channel showing the title, not only the one it was made on. */
  anyChannel: boolean;
  /** Skips repeats and episodes already recorded. */
  newOnly: boolean;
  /** Finished recordings to keep: 0 for all, otherwise the latest this many. */
  keep: number;
  /** null follows Settings › Recording. */
  paddingBeforeSeconds: number | null;
  paddingAfterSeconds: number | null;
}

/** "Record every episode of …": a programme's title on one channel, or on any. */
export interface SeriesRule extends SeriesRuleOptions {
  id: string;
  title: string;
  sourceId: SourceId;
  /** The channel it was made on. */
  channelId: string;
  channelName: string;
  channelLogoUrl: string | null;
  artworkUrl: string | null;
  createdAt: Timestamp;
  nextAiring: Timestamp | null;
  nextChannelName: string | null;
  scheduledCount: number;
  /** Finished recordings still kept. */
  recordedCount: number;
  /** Scheduled airings that clash with others for the provider's streams. */
  conflictCount: number;
}

export interface ScheduleRequest {
  sourceId: SourceId;
  channelId: string;
  channelName: string;
  channelLogoUrl: string | null;
  title: string;
  subtitle: string | null;
  summary: string | null;
  artworkUrl: string | null;
  programId: string | null;
  start: Timestamp;
  end: Timestamp;
  paddingBeforeSeconds: number;
  paddingAfterSeconds: number;
}

export interface AddonCatalogInfo {
  mediaType: string;
  id: string;
  name: string | null;
  genres: string[];
  supportsSearch: boolean;
  browsable: boolean;
}

export interface AddonInfo {
  id: string;
  name: string;
  version: string | null;
  description: string | null;
  logoUrl: string | null;
  host: string;
  enabled: boolean;
  types: string[];
  resources: string[];
  catalogs: AddonCatalogInfo[];
}

export interface StreamTarget {
  url: string;
  userAgent: string | null;
  headers: [string, string][];
  isLive: boolean;
}

export interface AddonStream {
  addonId: string;
  addonName: string;
  name: string;
  description: string | null;
  quality: string | null;
  sizeBytes: number | null;
  tags: string[];
  target: StreamTarget | null;
}

export interface AddonSubtitle {
  addonName: string;
  language: string;
  url: string;
}

export interface AddonCatalogRow {
  addonId: string;
  catalogId: string;
  mediaType: string;
  title: string;
  movies: Movie[];
  series: Series[];
}

export interface AddonMeta {
  movie: Movie | null;
  series: Series | null;
  episodes: Episode[];
  details: MediaDetails;
}

// MARK: - A media folder's own library page

/** What a video file holds, read once in the background. */
export interface FileFacts {
  durationSeconds: number | null;
  width: number | null;
  height: number | null;
  audio: boolean;
  sizeBytes: number | null;
}

/** A GIF made into a video (silent, short and small), another short silent clip (an animation), or a video (with sound, or longer). */
export type ClipKind = "gif" | "animation" | "video";
/** Under a minute, or a minute or longer. */
export type ClipLength = "short" | "long";
export type FolderSort = "added" | "name" | "length" | "size";

export interface FolderQuery {
  sourceId: string;
  /** A folder's full path; null for the source's own. */
  folder: string | null;
  /** Everything below the folder, not only what's directly in it. */
  below: boolean;
  kind: ClipKind | null;
  length: ClipLength | null;
  sort: FolderSort;
  limit: number;
  offset: number;
}

export interface FolderVideo {
  movie: Movie;
  /** Below the source's folder ("imported/2026/clip.mp4"). */
  path: string;
  facts: FileFacts | null;
  kind: ClipKind | null;
}

export interface FolderEntry {
  name: string;
  path: string;
  /** Videos in it and below it. */
  videos: number;
}

export interface FolderListing {
  /** The folder shown; null for the source's own when it has several. */
  folder: string | null;
  /** The way down from the source's own folder. */
  trail: FolderEntry[];
  /** On the first page only. */
  folders: FolderEntry[];
  videos: FolderVideo[];
  /** Videos matching, on every page. */
  total: number;
  /** Videos here whose files haven't been read yet: a type or length filter can't place them until they are. */
  unread: number;
}

/** One time something was deleted from a media folder, waiting in its Recently Deleted. */
export interface DeletedBatch {
  id: string;
  root: string;
  deletedAt: Timestamp;
  entries: { path: string; folder: boolean }[];
}

// MARK: - Interface-only types (commands.rs builds these too)

export type BrowseSort = "recentlyAdded" | "title" | "year" | "rating";

export interface BrowseQuery {
  categoryId?: string | null;
  genre?: string | null;
  /** Home's genre rows: titles whose genre field names it, or in a category named for it ("EN - ACTION"). */
  contentGenre?: ContentGenre | null;
  sort: BrowseSort;
  limit: number;
  offset: number;
  /** One source's titles only. */
  sourceId?: string | null;
  watched?: WatchedFilter;
  /** A film's copies as one poster (Movies). */
  groupVersions?: boolean;
}

/** All titles, or only those not watched or part-way through. */
export type WatchedFilter = "all" | "unwatched" | "inProgress";

export interface ChannelFilter {
  categoryId?: string | null;
  favoritesOnly?: boolean;
  /** A named list's channels, in its order. */
  listId?: string | null;
  sourceId?: string | null;
  /** Hidden channels and categories too. Locked ones still need the PIN. */
  includeHidden?: boolean;
  limit?: number | null;
  /** Similar channels as one row, or each on its own; left out, the setting decides. */
  groupSimilar?: boolean | null;
}

/** What to play. The backend turns it into a gateway stream. */
export type PlayRequest =
  | { kind: "channel"; channelId: string }
  | { kind: "replay"; channelId: string; start: Timestamp; end: Timestamp }
  | { kind: "movie"; movieId: string }
  | { kind: "episode"; episodeId: string }
  | { kind: "recording"; recordingId: string }
  | { kind: "target"; target: StreamTarget };

/** What "Import from Xorvio for Mac" brought over. */
export interface LibraryImport {
  addons: string[];
  addonProblems: string[];
  watchlistAdded: number;
  watchlistSkipped: string[];
  progressImported: number;
}

/** How much space Xorvio takes, for Settings › Storage. */
export interface StorageUsage {
  /** catalog.sqlite: the sources' channels, films, series, details and guide listings. */
  catalogBytes: number;
  guideListings: number;
  /** library.sqlite: the watchlist, progress, favorites, groups and names. */
  libraryBytes: number;
  /** Recordings whose files are still there, wherever they were saved. */
  recordingsBytes: number;
  recordingCount: number;
  /** Subtitles of streams converted while they play. */
  temporaryBytes: number;
}

export type Appearance = "system" | "dark" | "light";
export type Density = "comfortable" | "compact";
/** What shows when Xorvio opens; "lastChannel" is Live TV, playing the channel watched last. */
export type LaunchScreen = "home" | "live" | "watchlist" | "lastChannel";

export interface Settings {
  appearance: Appearance;
  density: Density;
  launchScreen: LaunchScreen;
  /** Minutes before a programme a recording starts. */
  recordPaddingBefore: number;
  /** Minutes after a programme a recording keeps going. */
  recordPaddingAfter: number;
  recordingsFolder: string | null;
  /** Folders chosen for particular sources' recordings, by source id. */
  sourceRecordingFolders: Record<string, string>;
  /** Keep running in the menu bar while recordings are scheduled. */
  keepRunningForRecordings: boolean;
  /** Recordings' ad breaks: not looked for, marked with a Skip Ad button, or skipped without asking. */
  commercialSkip: "off" | "button" | "automatic";
  /** Minutes before a reminded programme starts that its notification goes out (0 is at the start). */
  reminderMinutes: number;
  /** Add-on titles lead Home, Films and Series (the Personal build's behaviour). */
  addonsLead: boolean;
  guideHours: number;
  guideRowHeight: number;
  showChannelNumbers: boolean;
  /** Guide cells tinted by the programme's kind, or plain. */
  guideColors: "category" | "plain";
  /** Versions of a channel the provider lists several times show as one row. */
  groupSimilarChannels: boolean;
  /** Channel and category names lose providers' prefixes and tags ("UK | Channel One FHD" shows as "Channel One"). */
  tidyNames: boolean;
  /** Minutes the parental PIN lasts once entered; null is until Xorvio quits. */
  parentalLockMinutes: number | null;
  /** First run's welcome has been seen through or skipped. */
  onboardingDone: boolean;
  tvmaze: boolean;
  /** TMDB is asked for details at all (with the person's key or the built-in one). */
  tmdb: boolean;
  /** Every film and series whose source names no one in it is looked up on TMDB in the background, so search finds it by its cast too. */
  tmdbSearch: boolean;
  /** Episodes' intros, recaps and credits are looked up on IntroDB. */
  introDb: boolean;
  /** Home's Trakt charts and lists load before an account is connected. */
  traktRowsWithoutAccount: boolean;
  preferredLiveFormat: LiveStreamFormat;
  /** ISO 639-2 code, or null to play each film's own default. */
  audioLanguage: string | null;
  /** Subtitles to turn on, as an ISO 639-2 code; null is off. */
  subtitleLanguage: string | null;
  subtitleSize: "small" | "medium" | "large";
  autoplayNextEpisode: boolean;
  /** Pause and rewind live TV: never, from a pause, or always (so it can rewind to the tune-in). */
  liveBuffer: "off" | "paused" | "always";
  /** How much live TV the buffer keeps. */
  liveBufferMinutes: number;
  /** How far behind live an unbuffered channel plays: the players' own balance, about two seconds, or as little as they allow. */
  liveLatency: "automatic" | "low" | "minimal";
  /** Keep every event in the diagnostic log, not only warnings and errors. */
  detailedDiagnostics: boolean;
  /** The pop-out player stays above other windows. */
  keepPopOutOnTop: boolean;
  skipIntrosAutomatically: boolean;
  swiftLibraryImported: boolean;
  /** Back up by itself, never with logins. */
  autoBackup: AutoBackup;
  /** Where automatic backups go; null is a Backups folder in Xorvio's data folder. */
  autoBackupFolder: string | null;
  /** Keep the library in step with other computers through a shared folder. */
  syncEnabled: boolean;
  syncFolder: string | null;
  /** A short muted clip of the featured film plays behind Home's hero. Off unless chosen: backdrops otherwise never move. */
  previewClips: boolean;
  /** A title's trailer plays, muted, behind its film or series page (not when the Mac asks for reduced motion). */
  titleTrailers: boolean;
  /** A notification when a followed series has new episodes. */
  newEpisodeAlerts: boolean;
}

export type AutoBackup = "off" | "daily" | "weekly";

/** Settings › Backup. */
export interface BackupStatus {
  computerName: string;
  /** Where automatic backups go. */
  automaticFolder: string;
  lastAutomatic: Timestamp | null;
  /** Why the last automatic backup didn't happen, in plain words. */
  automaticError: string | null;
  /** How the restore before this launch went. */
  restoreNotice: RestoreNotice | null;
  minPassphraseLength: number;
}

export interface RestoreNotice {
  restored: boolean;
  message: string;
}

export interface BackupCounts {
  sources: number;
  addons: number;
  watchlist: number;
  progress: number;
  history: number;
  favorites: number;
  lists: number;
  itemEdits: number;
  recordings: number;
  scheduledRecordings: number;
}

/** A backup picked to restore: its manifest, and what this computer has that it replaces. */
export interface BackupInfo {
  path: string;
  fileName: string;
  appVersion: string;
  createdAt: Timestamp;
  /** The computer it was made on. */
  computer: string;
  counts: BackupCounts;
  /** Logins and links are inside, sealed with a passphrase. */
  includesSecrets: boolean;
  automatic: boolean;
  /** Recordings running here now; a restore waits for them. */
  recordingNow: number;
  /** Recordings scheduled here, which the backup's schedule replaces. */
  scheduledHere: number;
}

/** Another computer seen in the sync folder. */
export interface SyncComputer {
  name: string;
  /** When it last wrote its sync file. */
  lastSeen: Timestamp;
}

/** Settings › Sync. */
export interface SyncStatus {
  enabled: boolean;
  folder: string | null;
  syncing: boolean;
  lastSync: Timestamp | null;
  /** Why the last sync failed, in plain words. */
  lastError: string | null;
  /** Worth knowing but not a failure (a file still arriving). */
  notice: string | null;
  thisComputer: string;
  computers: SyncComputer[];
}

export const defaultSettings: Settings = {
  appearance: "system",
  density: "comfortable",
  launchScreen: "home",
  recordPaddingBefore: 1,
  recordPaddingAfter: 3,
  recordingsFolder: null,
  sourceRecordingFolders: {},
  keepRunningForRecordings: true,
  commercialSkip: "button",
  reminderMinutes: 5,
  addonsLead: false,
  guideHours: 3,
  guideRowHeight: 60,
  showChannelNumbers: true,
  guideColors: "category",
  groupSimilarChannels: true,
  tidyNames: true,
  parentalLockMinutes: 15,
  onboardingDone: false,
  tvmaze: true,
  tmdb: true,
  tmdbSearch: true,
  introDb: true,
  traktRowsWithoutAccount: false,
  preferredLiveFormat: "hls",
  audioLanguage: null,
  subtitleLanguage: null,
  subtitleSize: "medium",
  autoplayNextEpisode: true,
  liveBuffer: "paused",
  liveBufferMinutes: 30,
  liveLatency: "automatic",
  detailedDiagnostics: false,
  keepPopOutOnTop: false,
  skipIntrosAutomatically: false,
  swiftLibraryImported: false,
  autoBackup: "off",
  autoBackupFolder: null,
  syncEnabled: false,
  syncFolder: null,
  previewClips: false,
  titleTrailers: true,
  newEpisodeAlerts: true,
};

// MARK: - Library profiles

/** A language or region a category's name can say ("US|", "QC - FILMS", "TURKISH SERIES"). */
export type ContentLanguage =
  | "english"
  | "french"
  | "spanish"
  | "portuguese"
  | "german"
  | "italian"
  | "dutch"
  | "nordic"
  | "polish"
  | "turkish"
  | "arabic"
  | "southAsian"
  | "russian"
  | "ukrainian"
  | "greek"
  | "albanian"
  | "balkan"
  | "hungarian"
  | "romanian"
  | "czechSlovak"
  | "bulgarian"
  | "persian"
  | "hebrew"
  | "korean"
  | "japanese"
  | "chinese"
  | "filipino"
  | "vietnamese"
  | "thai"
  | "african";

/** A genre a category's name can say ("EN - ACTION", "24/7 COMEDY"). */
export type ContentGenre =
  | "action"
  | "adventure"
  | "animation"
  | "comedy"
  | "crime"
  | "documentary"
  | "drama"
  | "family"
  | "fantasy"
  | "horror"
  | "kids"
  | "music"
  | "mystery"
  | "news"
  | "reality"
  | "romance"
  | "scienceFiction"
  | "sports"
  | "thriller"
  | "western"
  | "faith"
  | "holiday"
  | "anime";

/**
 * A saved view of the library ("English only", "Action"). The active one
 * applies to every list at once; the parental PIN still locks what it locks.
 */
export interface LibraryProfile {
  /** Empty for a profile not saved yet. */
  id: string;
  name: string;
  /** Only categories in these languages; empty: every language. */
  languages: ContentLanguage[];
  /** With languages chosen, categories that name none stay. */
  includeUnlabeled: boolean;
  hideAdult: boolean;
  /** Favourite genres, each a row on Home. */
  genres: ContentGenre[];
  /** Categories naming other genres are left out; those naming none stay. */
  onlyGenres: boolean;
  hiddenKinds: CategoryKind[];
  /** Shown or left out whatever the rules say. */
  includedCategories: string[];
  excludedCategories: string[];
}

export interface Profiles {
  profiles: LibraryProfile[];
  activeId: string;
}

/** A category with what its name says and how much it holds, for the profile editor. */
export interface ProfileCategory {
  id: string;
  sourceId: SourceId;
  kind: CategoryKind;
  name: string;
  itemCount: number;
  languages: ContentLanguage[];
  genres: ContentGenre[];
  adult: boolean;
}
