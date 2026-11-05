// The one way the interface reaches the backend. Inside Tauri every call is a
// command in src-tauri/src/commands.rs; in a plain browser (`npm run dev`) the
// demo library in mock.ts answers instead, so screens can be built and checked
// without the Rust side.

import type { PopOutHandoff } from "./store";
import { windowLabel } from "./windows";
import type {
  AdBreaks,
  AddonCatalogRow,
  AddonInfo,
  AddonMeta,
  AddonStream,
  AddonSubtitle,
  BackupInfo,
  BackupStatus,
  BrowseQuery,
  CastDetails,
  CastDevice,
  CastStatus,
  DiagnosticEvent,
  HistoryEntry,
  Category,
  CategoryKind,
  Channel,
  ChannelFilter,
  ContinueItem,
  DeletedBatch,
  Episode,
  EpisodeSegments,
  ExternalPlayer,
  FavoriteList,
  FolderListing,
  FolderQuery,
  GuideChannel,
  GuideMatch,
  GuideRow,
  GuideSource,
  GuideSourceSummary,
  ItemEdit,
  LibraryImport,
  LibraryProfile,
  MediaDetails,
  MediaKind,
  MenuState,
  Movie,
  NewGuideSource,
  NewSource,
  NotificationPermission,
  ParentalStatus,
  PersonDetails,
  PlayRequest,
  PlaybackProgress,
  PlaybackSupport,
  PlexServerChoice,
  PlexSignInStart,
  ProfileCategory,
  Profiles,
  ProgramHit,
  Recording,
  RefreshProgress,
  ResolvedStream,
  ScheduleRequest,
  SearchResults,
  Series,
  SeriesRule,
  SeriesRuleOptions,
  Settings,
  SourceConfig,
  SourceSecrets,
  SourceStatus,
  SourceSummary,
  StorageUsage,
  TitleMark,
  TitleVersion,
  StreamOptions,
  SyncStatus,
  Timestamp,
  TrackChoice,
  TraktListInfo,
  TraktListQuery,
  TraktOptions,
  TraktRow,
  TraktRowSource,
  TraktScrobbleAction,
  TraktSignInStart,
  TmdbStatus,
  Thumbnails,
  TraktStatus,
  WatchlistEntry,
  XtreamAccountCheck,
} from "./types";

export interface Backend {
  // Sources
  listSources(): Promise<SourceSummary[]>;
  addSource(source: NewSource): Promise<SourceConfig>;
  /** Adds a media folder source with these folders, named after the first unless a name is given. */
  addFolderSource(name: string, folders: string[]): Promise<SourceConfig>;
  /** Starts signing in to Plex: a code and the plex.tv page to approve it on. */
  plexStartSignIn(): Promise<PlexSignInStart>;
  /** The account's servers once the sign-in is approved; null while it waits. */
  plexCheckSignIn(pinId: number): Promise<PlexServerChoice[] | null>;
  addPlexServer(pinId: number, serverId: string, name: string): Promise<SourceConfig>;
  // Trakt
  traktStatus(): Promise<TraktStatus>;
  /** Starts connecting: a code to enter at the verification address. */
  traktStartSignIn(): Promise<TraktSignInStart>;
  /** True once the code has been entered and Xorvio allowed; false while waiting. Fails when the code expired or was refused. */
  traktCheckSignIn(): Promise<boolean>;
  traktDisconnect(): Promise<void>;
  traktSyncNow(): Promise<TraktStatus>;
  traktSetOptions(options: TraktOptions): Promise<void>;
  /** Uses this Trakt app instead of the built-in one; an empty client id goes back to the built-in one. The redirect address is the one it was registered with (empty: http://localhost). No client secret: Trakt asks desktop apps not to use one. */
  traktSetApp(clientId: string, redirectUri: string): Promise<void>;
  /** Tells Trakt what's playing. Progress is a percentage. Never fails: a problem is only logged. */
  traktScrobble(itemId: string, kind: MediaKind, action: TraktScrobbleAction, progress: number): Promise<void>;
  /** A Trakt chart, list or recommendations for a row on Home, in Trakt's order: the sources' copies, and add-on titles for the rest when an add-on can play them. */
  traktRow(source: TraktRowSource, limit: number): Promise<TraktRow>;
  traktLists(query: TraktListQuery): Promise<TraktListInfo[]>;
  /** Where the page that plays a title's YouTube trailer behind its page is (served by the media gateway, as YouTube's player needs). */
  trailerPage(youtubeId: string): Promise<string>;
  onTraktChanged(handler: (status: TraktStatus) => void): () => void;
  // TMDB
  tmdbStatus(): Promise<TmdbStatus>;
  /** Saves the person's own TMDB key (an API key or a read access token) once TMDB takes it; empty removes it. */
  tmdbSetKey(key: string): Promise<TmdbStatus>;
  /** How many titles are still to be looked up on TMDB for search, or null when they aren't being looked up (switched off, or no key). */
  searchLookupsLeft(): Promise<number | null>;
  /** Signs a Jellyfin or Emby source in again as the same user. */
  signInToSourceAgain(id: string, password: string): Promise<void>;
  /** Gives a Plex source the token from a new Plex sign-in. */
  replacePlexSignIn(id: string, pinId: number): Promise<void>;
  updateSource(config: SourceConfig, secrets: SourceSecrets | null): Promise<void>;
  removeSource(id: string): Promise<void>;
  sourceSecrets(id: string): Promise<SourceSecrets>;
  refreshSource(id: string): Promise<SourceStatus>;
  /** Asks a running refresh to stop; false when none is running. The refresh then returns with the source as it was. */
  cancelRefresh(id: string): Promise<boolean>;
  /** The sources in a new order, which Live TV lists channels in. */
  reorderSources(ids: string[]): Promise<void>;
  /** Signs in to an Xtream provider with what's in the sheet, saving nothing: what the provider says about the account. */
  testXtreamLogin(secrets: SourceSecrets, userAgent: string | null): Promise<XtreamAccountCheck>;
  /** The Xtream login inside a playlist link (…/get.php?username=…&password=…), or null. */
  xtreamLoginInLink(link: string): Promise<SourceSecrets | null>;
  /** Keeps a playlist dropped onto the add sheet (the web view gives only its contents) and resolves to where, for an M3U file source. */
  keepDroppedPlaylist(name: string, contents: string): Promise<string>;
  addDemoSource(): Promise<void>;
  /** True when the Swift Xorvio for Mac has sources that can be brought over. */
  swiftImportAvailable(): Promise<boolean>;
  importSwiftSources(): Promise<SourceConfig[]>;
  /** True when the Swift app has add-ons or a watchlist not yet brought over. */
  swiftLibraryAvailable(): Promise<boolean>;
  importSwiftLibrary(): Promise<LibraryImport>;
  /** Whether the Swift app's data is on this Mac at all. */
  swiftDataAvailable(): Promise<boolean>;
  importSwiftProgress(): Promise<{ imported: number; skipped: number }>;

  // Catalog
  /** Categories that hold something; with `includeHidden`, hidden ones too. */
  categories(kind: CategoryKind, includeHidden?: boolean): Promise<Category[]>;
  channels(filter: ChannelFilter): Promise<Channel[]>;
  channel(id: string): Promise<Channel | null>;
  movies(query: BrowseQuery): Promise<Movie[]>;
  movie(id: string): Promise<Movie | null>;
  seriesList(query: BrowseQuery): Promise<Series[]>;
  series(id: string): Promise<Series | null>;
  episodes(seriesId: string): Promise<Episode[]>;
  details(itemId: string): Promise<MediaDetails>;
  /** Films TMDB recommends with this one that the sources carry; none without a TMDB key. */
  relatedMovies(movieId: string): Promise<Movie[]>;
  relatedSeries(seriesId: string): Promise<Series[]>;
  /** Versions and what's been watched, for the posters of a list (films or series). */
  titleMarks(kind: CategoryKind, ids: string[]): Promise<TitleMark[]>;
  /** Every copy of a film the sources carry, the one asked for first. */
  movieVersions(movieId: string): Promise<TitleVersion[]>;
  /** TMDB's posters for titles shown without one, as found (none without a key). Ask for a few at a time. */
  fillPosters(ids: string[]): Promise<{ id: string; posterUrl: string }[]>;
  /** Someone from a cast, from TMDB (or Wikidata without a key), with their titles the sources carry; null when neither knows them. */
  person(name: string, tmdbId: string | null, itemId: string | null): Promise<PersonDetails | null>;
  genres(kind: CategoryKind): Promise<string[]>;
  topTitles(kind: CategoryKind, limit: number): Promise<{ movies: Movie[]; series: Series[] }>;
  search(text: string): Promise<SearchResults>;

  // Guide
  guide(channelIds: string[], from: Timestamp, to: Timestamp): Promise<GuideRow[]>;
  nowNext(channelIds: string[]): Promise<GuideRow[]>;
  program(programId: string): Promise<ProgramHit | null>;
  /** Films, sport and events starting in a window, for Home's "Tonight on TV". */
  upcomingPrograms(from: Timestamp, to: Timestamp, limit: number): Promise<ProgramHit[]>;
  /** Programmes by title ending after `from`, for jumping the guide to one: still to come first, then what aired. */
  searchGuide(text: string, from: Timestamp): Promise<ProgramHit[]>;
  /** About when the loaded listings begin and end. */
  guideSpan(): Promise<{ from: Timestamp; to: Timestamp } | null>;

  // Added guides, and guide channels chosen by hand
  guideSources(): Promise<GuideSourceSummary[]>;
  addGuideSource(guide: NewGuideSource): Promise<GuideSource>;
  /** A new link only when given; null keeps the one in the keychain. */
  updateGuideSource(guide: GuideSource, url: string | null): Promise<void>;
  removeGuideSource(id: string): Promise<void>;
  guideSourceLink(id: string): Promise<string | null>;
  refreshGuideSource(id: string): Promise<SourceStatus>;
  /** Channels in every guide whose id or name contains the text, closest first. */
  guideChannels(text: string): Promise<GuideChannel[]>;
  guideMatch(channelId: string): Promise<GuideMatch>;
  matchGuide(channelId: string, guideId: string, epgId: string): Promise<void>;
  resetGuideMatch(channelId: string): Promise<void>;

  // Library
  watchlist(): Promise<WatchlistEntry[]>;
  setWatchlisted(entry: WatchlistEntry, saved: boolean): Promise<void>;
  favoriteChannelIds(): Promise<string[]>;
  setFavorite(channelId: string, favorite: boolean): Promise<void>;
  saveProgress(progress: PlaybackProgress): Promise<void>;

  // Channel groups, names, hiding and the parental PIN
  favoriteLists(): Promise<FavoriteList[]>;
  createFavoriteList(name: string, channelId: string | null): Promise<FavoriteList>;
  renameFavoriteList(id: string, name: string): Promise<void>;
  deleteFavoriteList(id: string): Promise<void>;
  /** Adds a channel to a list (FAVORITES_LIST or a group) or takes it out. */
  setInFavoriteList(listId: string, channelId: string, member: boolean): Promise<void>;
  /** A list's channels in a new order; ones left out (hidden) keep their places. */
  reorderFavoriteList(listId: string, channelIds: string[]): Promise<void>;
  itemEdits(): Promise<ItemEdit[]>;
  /** A channel's or category's own name; null goes back to the provider's. */
  renameItem(itemId: string, name: string | null): Promise<void>;
  setHidden(itemId: string, hidden: boolean): Promise<void>;
  /** Moves a channel's replays by some minutes (−30…30), for providers whose replays start early or late; 0 for none. */
  setCatchupOffset(channelId: string, minutes: number): Promise<void>;
  /** "Always play this version" for a channel listed several times; false goes back to the best version. */
  setPreferredVersion(channelId: string, preferred: boolean): Promise<void>;
  showAllHidden(): Promise<void>;
  setLocked(itemId: string, locked: boolean): Promise<void>;
  parentalStatus(): Promise<ParentalStatus>;
  /** Creates, changes or (newPin null) removes the PIN. */
  setPin(currentPin: string | null, newPin: string | null): Promise<void>;
  /** Enters the PIN for this session; false when it's wrong. */
  unlockParental(pin: string): Promise<boolean>;
  lockParental(): Promise<void>;
  /** Marks films or episodes watched or unwatched, a season or series at once. */
  setWatched(items: PlaybackProgress[], watched: boolean): Promise<void>;
  removeFromContinueWatching(itemId: string): Promise<void>;
  /** Progress for many items in one call (a series' episodes). */
  seriesProgress(itemIds: string[]): Promise<PlaybackProgress[]>;
  progress(itemId: string): Promise<PlaybackProgress | null>;
  continueWatching(limit: number): Promise<ContinueItem[]>;
  recentChannels(limit: number): Promise<Channel[]>;
  recordChannelWatched(channelId: string): Promise<void>;
  /** Notes a film or episode started playing, for Viewing History. */
  recordWatched(itemId: string, kind: MediaKind, sourceId: string, title: string, subtitle: string | null): Promise<void>;
  /** Viewing History, newest first. */
  history(limit?: number): Promise<HistoryEntry[]>;
  /** Forgets Viewing History and Recent Channels; progress and Continue Watching stay. */
  clearHistory(): Promise<void>;

  // Playback
  /** `direct` plays a live channel straight from its source, never from the live buffer (Multiview's tiles). */
  play(request: PlayRequest, direct?: boolean): Promise<ResolvedStream>;
  seekStream(url: string, offsetSeconds: number): Promise<ResolvedStream>;
  releaseStream(url: string): Promise<void>;
  restream(url: string, offsetSeconds: number, options: StreamOptions): Promise<ResolvedStream>;
  /** The error status the player was last answered with for a stream (a provider's 403 or 404, the gateway's 502); null while all is well. */
  streamFailure(url: string): Promise<number | null>;
  /** Where the player is in a remuxed stream (its own `currentTime`), so the gateway keeps only a minute or so ahead. */
  streamPosition(url: string, seconds: number): Promise<void>;
  /** The audio and subtitles chosen for a series, for its other episodes. */
  trackChoice(seriesId: string): Promise<TrackChoice | null>;
  /** Remembers them for the series; null forgets. */
  setTrackChoice(seriesId: string, choice: TrackChoice | null): Promise<void>;
  /** Moves a live channel playing directly onto a buffer, from now. */
  startTimeshift(url: string): Promise<ResolvedStream>;
  /**
   * The seek bar's thumbnails for the film, episode or recording playing at
   * `url` (`itemId` and `sourceId` say what it is). The first ask starts
   * making them; ask again while they're "making".
   */
  thumbnails(url: string, itemId: string, sourceId: string): Promise<Thumbnails>;
  addonSubtitleText(url: string): Promise<string>;
  /** A media folder's own library page: its folders (on the first page) and videos, filtered. */
  folderListing(query: FolderQuery): Promise<FolderListing>;
  /** Every video a listing shows on all its pages, for Select All. */
  folderVideoIds(query: FolderQuery): Promise<string[]>;
  /** Moves videos (with their pictures and subtitles) and folders to the folder's Recently Deleted; how many went. */
  deleteFolderItems(sourceId: string, videos: string[], folders: string[]): Promise<number>;
  recentlyDeleted(sourceId: string): Promise<DeletedBatch[]>;
  restoreDeleted(sourceId: string, batchId: string): Promise<void>;
  /** Deletes for good these deletions; all of Recently Deleted with none named. */
  emptyRecentlyDeleted(sourceId: string, batchIds: string[] | null): Promise<void>;
  /** Moves videos and folders into a folder of the same source. */
  moveFolderItems(sourceId: string, videos: string[], folders: string[], to: string): Promise<void>;
  /** A new name for a video's file (its extension stays). */
  renameFolderVideo(sourceId: string, itemId: string, name: string): Promise<void>;
  /** Makes a folder; its path. */
  createMediaFolder(sourceId: string, parent: string, name: string): Promise<string>;
  renameMediaFolder(sourceId: string, path: string, name: string): Promise<void>;
  /** More of the media folders' video files were read: how many. */
  onFolderFacts(handler: (count: number) => void): () => void;
  /** Subtitle files beside a media folder's film or episode. */
  folderSubtitles(itemId: string): Promise<AddonSubtitle[]>;
  /** One of those subtitle files, as WebVTT. */
  folderSubtitleText(url: string): Promise<string>;
  episodeSegments(imdbId: string, season: number, episode: number): Promise<EpisodeSegments | null>;
  setPlaybackSupport(support: PlaybackSupport): Promise<void>;
  /** IINA, VLC and Infuse, those that are installed. */
  externalPlayers(): Promise<ExternalPlayer[]>;
  /**
   * Hands a stream to another player: the one playing (`streamUrl`), or what
   * `request` names. `title` is what that player shows; `positionSeconds` is
   * where a film or episode starts, in players that take a start time.
   */
  openInPlayer(player: string, streamUrl: string | null, request: PlayRequest | null, title: string, positionSeconds: number | null): Promise<void>;

  // Chromecast and Google TV
  /** The TVs found on the network so far. The first call starts looking (and the system may ask about the local network). */
  castDevices(): Promise<CastDevice[]>;
  /** Why what `request` names can't go to a TV; null when it can. */
  castCheck(request: PlayRequest): Promise<string | null>;
  /** Connects to a TV and opens its player. Whatever another TV was playing stops. */
  castConnect(deviceId: string): Promise<CastStatus>;
  /** Plays on the connected TV, from `positionSeconds` for a film or an episode. Let the player's own stream of a channel go first. */
  castLoad(request: PlayRequest, details: CastDetails, positionSeconds: number | null): Promise<CastStatus>;
  castPlay(): Promise<void>;
  castPause(): Promise<void>;
  /** A film or an episode on the TV, to `positionSeconds` in the title. */
  castSeek(positionSeconds: number): Promise<void>;
  /** The TV's volume, 0 to 1. */
  castVolume(level: number): Promise<void>;
  castMute(muted: boolean): Promise<void>;
  /** Stops casting; where a film or an episode was on the TV, to carry on from here. */
  castStop(): Promise<number | null>;

  // Recording
  recordings(): Promise<Recording[]>;
  /**
   * What to warn about before one more stream from a source plays (a
   * Multiview tile), with `watching` of its streams playing already and its
   * recordings running; null when the provider's limit allows it.
   */
  connectionWarning(sourceId: string, watching: number): Promise<string | null>;
  scheduleRecording(request: ScheduleRequest): Promise<Recording>;
  recordNow(request: ScheduleRequest): Promise<Recording>;
  stopRecording(id: string): Promise<void>;
  cancelRecording(id: string): Promise<void>;
  deleteRecording(id: string, deleteFile: boolean): Promise<void>;
  revealRecording(id: string): Promise<void>;
  /** Saves a programme that already aired from the channel's catch-up, with what padding the archive allows. */
  recordFromCatchup(request: ScheduleRequest): Promise<Recording>;
  /** What's known of a recording's ad breaks. */
  adBreaks(recordingId: string): Promise<AdBreaks>;
  /** Looks for a finished recording's ad breaks now, in the background. */
  findAdBreaks(recordingId: string): Promise<AdBreaks>;
  seriesRules(): Promise<SeriesRule[]>;
  /** Records every airing (or every new one) of a guide programme's title. */
  createSeriesRule(programId: string, channelId: string, options: SeriesRuleOptions): Promise<SeriesRule>;
  updateSeriesRule(id: string, options: SeriesRuleOptions): Promise<SeriesRule>;
  /** Its recordings go to the Trash with `deleteRecordings`, or stay as recordings of their own. */
  deleteSeriesRule(id: string, deleteRecordings: boolean): Promise<void>;

  // Add-ons
  addons(): Promise<AddonInfo[]>;
  installAddon(manifestUrl: string): Promise<AddonInfo>;
  removeAddon(id: string): Promise<void>;
  setAddonEnabled(id: string, enabled: boolean): Promise<void>;
  discoverRows(): Promise<AddonCatalogRow[]>;
  addonCatalog(addonId: string, mediaType: string, catalogId: string, genre: string | null, skip: number): Promise<AddonCatalogRow>;
  addonMeta(mediaType: string, mediaId: string): Promise<AddonMeta>;
  addonStreams(mediaType: string, mediaId: string): Promise<AddonStream[]>;
  addonSubtitles(mediaType: string, mediaId: string): Promise<AddonSubtitle[]>;
  addonSearch(text: string): Promise<{ movies: Movie[]; series: Series[] }>;

  // Settings
  settings(): Promise<Settings>;
  saveSettings(settings: Settings): Promise<void>;
  storageUsage(): Promise<StorageUsage>;
  /** Deletes every guide listing and gives the space back; refreshes load them again. */
  clearGuideListings(): Promise<void>;
  /** Deletes converted streams' subtitles, except those of the stream addresses given (what's playing). */
  clearTemporaryFiles(inUse: string[]): Promise<void>;
  /** Opens the folder new recordings go to. */
  showRecordingsFolder(): Promise<void>;
  chooseFolder(): Promise<string | null>;
  chooseFile(extensions: string[]): Promise<string | null>;
  /** Folders of films or series, several at once if wanted; empty when none were chosen. */
  chooseMediaFolders(): Promise<string[]>;
  openExternal(url: string): Promise<void>;
  /** A line in the app's log, for problems inside the web view. */
  log(level: "info" | "warn" | "error", message: string): Promise<void>;

  // The pop-out player
  /** Plays this in the pop-out player's window, opening it when it isn't open. */
  popOut(handoff: PopOutHandoff, onTop: boolean): Promise<void>;
  /** In the pop-out window, once: what the main window handed over. */
  takePopOut(): Promise<PopOutHandoff | null>;
  /** From the pop-out window: plays this in the main window again, and closes the pop-out. */
  dock(handoff: PopOutHandoff): Promise<void>;
  setPopOutOnTop(on: boolean): Promise<void>;
  /** In the pop-out window: something else to play, from the main window. */
  onPopOutPlay(handler: (handoff: PopOutHandoff) => void): () => void;
  /** In the main window: the pop-out handed its video back. */
  onDock(handler: (handoff: PopOutHandoff) => void): () => void;
  /** In the main window: the pop-out window closed. */
  onPopOutClosed(handler: () => void): () => void;
  /** The diagnostic log, newest first. */
  diagnostics(): Promise<DiagnosticEvent[]>;
  clearDiagnostics(): Promise<void>;
  /** Saves the log where the person chooses; null when they cancel. */
  exportDiagnostics(): Promise<string | null>;
  // Library profiles and Home
  profiles(): Promise<Profiles>;
  /** Switches profile: every list follows at once. */
  setActiveProfile(id: string): Promise<void>;
  /** Adds a profile (an empty id) or saves changes to one; resolves with it as kept. */
  saveProfile(profile: LibraryProfile): Promise<LibraryProfile>;
  /** Deletes a profile and its Home layouts; the last one can't go. */
  deleteProfile(id: string): Promise<void>;
  /** Every category with what its name says and how much it holds (locked ones wait for the PIN). */
  profileCategories(): Promise<ProfileCategory[]>;
  /** The active profile's Home layout for when the sources or add-ons lead; null for the standard one. */
  homeLayout(mode: "sources" | "addons"): Promise<unknown | null>;
  /** Saves it; null goes back to the standard one. */
  saveHomeLayout(mode: "sources" | "addons", layout: unknown | null): Promise<void>;
  /** What's airing now: favourite and recently watched channels first, then others. */
  onNow(limit: number): Promise<ProgramHit[]>;
  /** Home's New Releases: films from this year and last, this year's first, one card each. */
  newReleases(limit: number): Promise<Movie[]>;
  /** Home's New Episodes: followed series with new episodes, then series with episodes just aired. */
  newEpisodeSeries(limit: number): Promise<Series[]>;
  /** Series with new episodes not looked at yet, by id, and how many. */
  newEpisodeCounts(): Promise<Record<string, number>>;
  /** Opening a series clears its "New Episode" badge; true when it had one. */
  markEpisodesSeen(seriesId: string): Promise<boolean>;

  // Notifications (Settings › Reminders)
  notificationPermission(): Promise<NotificationPermission>;
  /** Shows a sample reminder a few seconds from now; fails when notifications are off. */
  sendTestReminder(): Promise<void>;
  /** Opens Xorvio's page in the system's notification settings. */
  openNotificationSettings(): Promise<void>;

  // Backup and sync
  backupStatus(): Promise<BackupStatus>;
  /** Saves a backup where the person chooses; with a passphrase its logins and links go in too, sealed. Null when they cancel. */
  backUpNow(passphrase: string | null): Promise<string | null>;
  /** Asks for a backup to restore and says what's in it; null when they cancel. */
  chooseBackup(): Promise<BackupInfo | null>;
  /** Stages the backup and restarts Xorvio to put it in place. The passphrase opens its logins; without one they stay as they are here. */
  restoreBackup(path: string, passphrase: string | null): Promise<void>;
  showBackupsFolder(): Promise<void>;
  syncStatus(): Promise<SyncStatus>;
  /** Syncs at once and says how it went. */
  syncNow(): Promise<SyncStatus>;

  // The menu bar
  /** What the menu bar's items can do now, from this window. */
  setMenuState(state: MenuState): Promise<void>;
  /** A menu bar item was chosen, by its id (src-tauri/src/menu.rs). */
  onMenu(handler: (id: string) => void): () => void;

  // Events
  onRecordingChanged(handler: (recording: Recording) => void): () => void;
  /** Recordings came or went other than one by one (a series scheduled some, or cleared out old ones). */
  onRecordingsChanged(handler: () => void): () => void;
  onRefreshProgress(handler: (progress: RefreshProgress) => void): () => void;
  /** A refresh ended, however it went, by hand or in the background: the source's id. */
  onRefreshFinished(handler: (sourceId: string) => void): () => void;
  onLibraryChanged(handler: () => void): () => void;
  /** The tray menu asks to show a screen. */
  onNavigate(handler: (tab: string) => void): () => void;
  /** A reminder was clicked: tune its channel. */
  onPlayChannel(handler: (channelId: string) => void): () => void;
  /** TVs appeared on the network or left it. */
  onCastDevices(handler: (devices: CastDevice[]) => void): () => void;
  /** The TV being cast to changed state (or stopped by itself). */
  onCastChanged(handler: (status: CastStatus) => void): () => void;
  onSyncChanged(handler: (status: SyncStatus) => void): () => void;
  /** An automatic backup was made, or couldn't be. */
  onBackupChanged(handler: () => void): () => void;
  /** A new-episode notification was clicked: show the series. */
  onOpenSeries(handler: (seriesId: string) => void): () => void;
  /** A new-episode notification's Play button: play its first new episode. */
  onPlayEpisode(handler: (target: { seriesId: string; episodeId: string }) => void): () => void;
}

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Command names are the method names in snake_case. */
function snake(name: string) {
  return name.replace(/[A-Z]/g, (c) => "_" + c.toLowerCase());
}

async function createTauriBackend(): Promise<Backend> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");

  const call =
    (name: string, ...argNames: string[]) =>
    (...args: unknown[]) =>
      invoke(snake(name), Object.fromEntries(argNames.map((a, i) => [a, args[i] ?? null])));

  // A listener with no target hears an event whichever window it was sent
  // to; `target` keeps to those sent to this window.
  const subscribe =
    <T,>(event: string, target?: string) =>
    (handler: (payload: T) => void) => {
      let unlisten: (() => void) | undefined;
      let cancelled = false;
      listen<T>(event, (e) => handler(e.payload), target ? { target } : undefined).then((u) => {
        if (cancelled) u();
        else unlisten = u;
      });
      return () => {
        cancelled = true;
        unlisten?.();
      };
    };

  return {
    listSources: call("listSources"),
    addSource: call("addSource", "source"),
    addFolderSource: call("addFolderSource", "name", "folders"),
    plexStartSignIn: call("plexStartSignIn"),
    plexCheckSignIn: call("plexCheckSignIn", "pinId"),
    addPlexServer: call("addPlexServer", "pinId", "serverId", "name"),
    traktStatus: call("traktStatus"),
    traktStartSignIn: call("traktStartSignIn"),
    traktCheckSignIn: call("traktCheckSignIn"),
    traktDisconnect: call("traktDisconnect"),
    traktSyncNow: call("traktSyncNow"),
    traktSetOptions: call("traktSetOptions", "options"),
    traktSetApp: call("traktSetApp", "clientId", "redirectUri"),
    traktScrobble: call("traktScrobble", "itemId", "kind", "action", "progress"),
    traktRow: call("traktRow", "source", "limit"),
    traktLists: call("traktLists", "query"),
    trailerPage: call("trailerPage", "youtubeId"),
    onTraktChanged: subscribe<TraktStatus>("trakt-changed"),
    tmdbStatus: call("tmdbStatus"),
    tmdbSetKey: call("tmdbSetKey", "key"),
    searchLookupsLeft: call("searchLookupsLeft"),
    signInToSourceAgain: call("signInToSourceAgain", "id", "password"),
    replacePlexSignIn: call("replacePlexSignIn", "id", "pinId"),
    updateSource: call("updateSource", "config", "secrets"),
    removeSource: call("removeSource", "id"),
    sourceSecrets: call("sourceSecrets", "id"),
    refreshSource: call("refreshSource", "id"),
    cancelRefresh: call("cancelRefresh", "id"),
    reorderSources: call("reorderSources", "ids"),
    testXtreamLogin: call("testXtreamLogin", "secrets", "userAgent"),
    xtreamLoginInLink: call("xtreamLoginInLink", "link"),
    keepDroppedPlaylist: call("keepDroppedPlaylist", "name", "contents"),
    addDemoSource: call("addDemoSource"),
    swiftImportAvailable: call("swiftImportAvailable"),
    importSwiftSources: call("importSwiftSources"),
    swiftLibraryAvailable: call("swiftLibraryAvailable"),
    importSwiftLibrary: call("importSwiftLibrary"),
    swiftDataAvailable: call("swiftDataAvailable"),
    importSwiftProgress: call("importSwiftProgress"),

    categories: call("categories", "kind", "includeHidden"),
    channels: call("channels", "filter"),
    channel: call("channel", "id"),
    movies: call("movies", "query"),
    movie: call("movie", "id"),
    seriesList: call("seriesList", "query"),
    series: call("series", "id"),
    episodes: call("episodes", "seriesId"),
    details: call("details", "itemId"),
    relatedMovies: call("relatedMovies", "movieId"),
    relatedSeries: call("relatedSeries", "seriesId"),
    titleMarks: call("titleMarks", "kind", "ids"),
    movieVersions: call("movieVersions", "movieId"),
    fillPosters: call("fillPosters", "ids"),
    person: call("person", "name", "tmdbId", "itemId"),
    genres: call("genres", "kind"),
    topTitles: call("topTitles", "kind", "limit"),
    search: call("search", "text"),

    guide: call("guide", "channelIds", "from", "to"),
    nowNext: call("nowNext", "channelIds"),
    program: call("program", "programId"),
    upcomingPrograms: call("upcomingPrograms", "from", "to", "limit"),
    searchGuide: call("searchGuide", "text", "from"),
    guideSpan: call("guideSpan"),

    guideSources: call("guideSources"),
    addGuideSource: call("addGuideSource", "guide"),
    updateGuideSource: call("updateGuideSource", "guide", "url"),
    removeGuideSource: call("removeGuideSource", "id"),
    guideSourceLink: call("guideSourceLink", "id"),
    refreshGuideSource: call("refreshGuideSource", "id"),
    guideChannels: call("guideChannels", "text"),
    guideMatch: call("guideMatch", "channelId"),
    matchGuide: call("matchGuide", "channelId", "guideId", "epgId"),
    resetGuideMatch: call("resetGuideMatch", "channelId"),

    watchlist: call("watchlist"),
    setWatchlisted: call("setWatchlisted", "entry", "saved"),
    favoriteChannelIds: call("favoriteChannelIds"),
    setFavorite: call("setFavorite", "channelId", "favorite"),
    saveProgress: call("saveProgress", "progress"),

    favoriteLists: call("favoriteLists"),
    createFavoriteList: call("createFavoriteList", "name", "channelId"),
    renameFavoriteList: call("renameFavoriteList", "id", "name"),
    deleteFavoriteList: call("deleteFavoriteList", "id"),
    setInFavoriteList: call("setInFavoriteList", "listId", "channelId", "member"),
    reorderFavoriteList: call("reorderFavoriteList", "listId", "channelIds"),
    itemEdits: call("itemEdits"),
    renameItem: call("renameItem", "itemId", "name"),
    setHidden: call("setHidden", "itemId", "hidden"),
    setCatchupOffset: call("setCatchupOffset", "channelId", "minutes"),
    setPreferredVersion: call("setPreferredVersion", "channelId", "preferred"),
    showAllHidden: call("showAllHidden"),
    setLocked: call("setLocked", "itemId", "locked"),
    parentalStatus: call("parentalStatus"),
    setPin: call("setPin", "currentPin", "newPin"),
    unlockParental: call("unlockParental", "pin"),
    lockParental: call("lockParental"),
    setWatched: call("setWatched", "items", "watched"),
    removeFromContinueWatching: call("removeFromContinueWatching", "itemId"),
    seriesProgress: call("seriesProgress", "itemIds"),
    progress: call("progress", "itemId"),
    continueWatching: call("continueWatching", "limit"),
    recentChannels: call("recentChannels", "limit"),
    recordChannelWatched: call("recordChannelWatched", "channelId"),
    recordWatched: call("recordWatched", "itemId", "kind", "sourceId", "title", "subtitle"),
    history: call("history", "limit"),
    clearHistory: call("clearHistory"),

    play: call("play", "request", "direct"),
    seekStream: call("seekStream", "url", "offsetSeconds"),
    releaseStream: call("releaseStream", "url"),
    restream: call("restream", "url", "offsetSeconds", "options"),
    streamFailure: call("streamFailure", "url"),
    streamPosition: call("streamPosition", "url", "seconds"),
    trackChoice: call("trackChoice", "seriesId"),
    setTrackChoice: call("setTrackChoice", "seriesId", "choice"),
    startTimeshift: call("startTimeshift", "url"),
    thumbnails: call("thumbnails", "url", "itemId", "sourceId"),
    addonSubtitleText: call("addonSubtitleText", "url"),
    folderListing: call("folderListing", "query"),
    folderVideoIds: call("folderVideoIds", "query"),
    deleteFolderItems: call("deleteFolderItems", "sourceId", "videos", "folders"),
    recentlyDeleted: call("recentlyDeleted", "sourceId"),
    restoreDeleted: call("restoreDeleted", "sourceId", "batchId"),
    emptyRecentlyDeleted: call("emptyRecentlyDeleted", "sourceId", "batchIds"),
    moveFolderItems: call("moveFolderItems", "sourceId", "videos", "folders", "to"),
    renameFolderVideo: call("renameFolderVideo", "sourceId", "itemId", "name"),
    createMediaFolder: call("createMediaFolder", "sourceId", "parent", "name"),
    renameMediaFolder: call("renameMediaFolder", "sourceId", "path", "name"),
    onFolderFacts: subscribe<number>("folder-facts"),
    folderSubtitles: call("folderSubtitles", "itemId"),
    folderSubtitleText: call("folderSubtitleText", "url"),
    episodeSegments: call("episodeSegments", "imdbId", "season", "episode"),
    setPlaybackSupport: call("setPlaybackSupport", "support"),
    externalPlayers: call("externalPlayers"),
    openInPlayer: call("openInPlayer", "player", "streamUrl", "request", "title", "positionSeconds"),
    castDevices: call("castDevices"),
    castCheck: call("castCheck", "request"),
    castConnect: call("castConnect", "deviceId"),
    castLoad: call("castLoad", "request", "details", "positionSeconds"),
    castPlay: call("castPlay"),
    castPause: call("castPause"),
    castSeek: call("castSeek", "positionSeconds"),
    castVolume: call("castVolume", "level"),
    castMute: call("castMute", "muted"),
    castStop: call("castStop"),

    recordings: call("recordings"),
    connectionWarning: call("connectionWarning", "sourceId", "watching"),
    scheduleRecording: call("scheduleRecording", "request"),
    recordNow: call("recordNow", "request"),
    stopRecording: call("stopRecording", "id"),
    cancelRecording: call("cancelRecording", "id"),
    deleteRecording: call("deleteRecording", "id", "deleteFile"),
    revealRecording: call("revealRecording", "id"),
    recordFromCatchup: call("recordFromCatchup", "request"),
    adBreaks: call("adBreaks", "recordingId"),
    findAdBreaks: call("findAdBreaks", "recordingId"),
    seriesRules: call("seriesRules"),
    createSeriesRule: call("createSeriesRule", "programId", "channelId", "options"),
    updateSeriesRule: call("updateSeriesRule", "id", "options"),
    deleteSeriesRule: call("deleteSeriesRule", "id", "deleteRecordings"),

    addons: call("addons"),
    installAddon: call("installAddon", "manifestUrl"),
    removeAddon: call("removeAddon", "id"),
    setAddonEnabled: call("setAddonEnabled", "id", "enabled"),
    discoverRows: call("discoverRows"),
    addonCatalog: call("addonCatalog", "addonId", "mediaType", "catalogId", "genre", "skip"),
    addonMeta: call("addonMeta", "mediaType", "mediaId"),
    addonStreams: call("addonStreams", "mediaType", "mediaId"),
    addonSubtitles: call("addonSubtitles", "mediaType", "mediaId"),
    addonSearch: call("addonSearch", "text"),

    settings: call("settings"),
    saveSettings: call("saveSettings", "settings"),
    storageUsage: call("storageUsage"),
    clearGuideListings: call("clearGuideListings"),
    clearTemporaryFiles: call("clearTemporaryFiles", "inUse"),
    showRecordingsFolder: call("showRecordingsFolder"),
    chooseFolder: call("chooseFolder"),
    chooseFile: call("chooseFile", "extensions"),
    chooseMediaFolders: call("chooseMediaFolders"),
    openExternal: call("openExternal", "url"),
    log: call("clientLog", "level", "message"),
    diagnostics: call("diagnostics"),
    clearDiagnostics: call("clearDiagnostics"),
    exportDiagnostics: call("exportDiagnostics"),
    popOut: call("popOut", "handoff", "onTop"),
    takePopOut: call("takePopOut"),
    dock: call("dock", "handoff"),
    setPopOutOnTop: call("setPopOutOnTop", "on"),
    onPopOutPlay: subscribe<PopOutHandoff>("popout-play"),
    onDock: subscribe<PopOutHandoff>("dock"),
    onPopOutClosed: subscribe<null>("popout-closed"),

    profiles: call("profiles"),
    setActiveProfile: call("setActiveProfile", "id"),
    saveProfile: call("saveProfile", "profile"),
    deleteProfile: call("deleteProfile", "id"),
    profileCategories: call("profileCategories"),
    homeLayout: call("homeLayout", "mode"),
    saveHomeLayout: call("saveHomeLayout", "mode", "layout"),
    onNow: call("onNow", "limit"),
    backupStatus: call("backupStatus"),
    backUpNow: call("backUpNow", "passphrase"),
    chooseBackup: call("chooseBackup"),
    restoreBackup: call("restoreBackup", "path", "passphrase"),
    showBackupsFolder: call("showBackupsFolder"),
    syncStatus: call("syncStatus"),
    syncNow: call("syncNow"),
    newReleases: call("newReleases", "limit"),
    newEpisodeSeries: call("newEpisodeSeries", "limit"),
    newEpisodeCounts: call("newEpisodeCounts"),
    markEpisodesSeen: call("markEpisodesSeen", "seriesId"),
    notificationPermission: call("notificationPermission"),
    sendTestReminder: call("sendTestReminder"),
    openNotificationSettings: call("openNotificationSettings"),

    setMenuState: call("setMenuState", "state"),
    // Each item goes to the window that acts on it (menu.rs).
    onMenu: subscribe<string>("menu", windowLabel),

    onRecordingChanged: subscribe<Recording>("recording-changed"),
    onRecordingsChanged: subscribe<null>("recordings-changed"),
    onRefreshProgress: subscribe<RefreshProgress>("refresh-progress"),
    onRefreshFinished: subscribe<string>("refresh-finished"),
    onLibraryChanged: subscribe<null>("library-changed"),
    onNavigate: subscribe<string>("navigate"),
    onPlayChannel: subscribe<string>("play-channel"),
    onCastDevices: subscribe<CastDevice[]>("cast-devices"),
    onCastChanged: subscribe<CastStatus>("cast-changed"),
    onSyncChanged: subscribe<SyncStatus>("sync-changed"),
    onBackupChanged: subscribe<null>("backup-changed"),
    onOpenSeries: subscribe<string>("open-series"),
    onPlayEpisode: subscribe<{ seriesId: string; episodeId: string }>("play-episode"),
  } as Backend;
}

let backend: Backend;

/** Loads the right backend once, before the interface first renders. */
export async function initBackend() {
  backend = isTauri ? await createTauriBackend() : (await import("./mock")).createMockBackend();
}

export const api = new Proxy({} as Backend, {
  get: (_, key: string) => (backend as unknown as Record<string, unknown>)[key],
});

/** Plain-language text for a failed command. */
export function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Something went wrong.";
}
