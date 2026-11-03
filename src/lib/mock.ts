// A stand-in backend for a plain browser: the website's fictional showcase
// library (src/dev/demo-library.json), a guide generated around "now", and
// library state kept in memory. Only `npm run dev` outside Tauri uses it.

import type { Backend } from "./api";
import type {
  BrowseQuery,
  AdBreaks,
  CastDevice,
  CastStatus,
  Category,
  CategoryKind,
  Channel,
  ContentGenre,
  ContinueItem,
  DiagnosticEvent,
  HistoryEntry,
  EditKind,
  Episode,
  FavoriteList,
  GuideChannel,
  GuideRow,
  GuideSourceSummary,
  ItemEdit,
  MediaDetails,
  Movie,
  PlaybackProgress,
  Program,
  ProgramGenre,
  ProgramHit,
  Recording,
  ResolvedStream,
  RefreshProgress,
  ScheduleRequest,
  SearchPerson,
  Series,
  SeriesRule,
  SeriesRuleOptions,
  Settings,
  SourceConfig,
  SourceSecrets,
  SourceStatus,
  SourceSummary,
  SyncStatus,
  TitleMark,
  ThumbnailSheet,
  Thumbnails,
  TmdbStatus,
  TrackChoice,
  TraktListInfo,
  TraktRow,
  TraktRowItem,
  TraktRowSource,
  TraktStatus,
  WatchlistEntry,
} from "./types";
import { FAVORITES_LIST, defaultSettings } from "./types";
import { libraryBackend } from "./mockLibrary";
import { genreCategories, genreMatches, profileBackend, profileExcluded } from "./mockProfiles";
import data from "../dev/demo-library.json";

interface DemoFilm {
  id: string;
  title: string;
  year: number;
  genre: string;
  runtime: number;
  rating: number;
  age: string;
  director: string;
  cast: string[];
  tagline: string;
  plot: string;
  video: string;
  category: string;
  poster: string;
  backdrop: string;
}

interface DemoSeries {
  id: string;
  title: string;
  year: number;
  genre: string;
  rating: number;
  age: string;
  network: string;
  creators: string[];
  cast: string[];
  plot: string;
  category: string;
  poster: string;
  backdrop: string;
  seasons: { title: string; plot: string | null; still: string | null }[][];
}

const S = "demo";
const now = () => Math.floor(Date.now() / 1000);
const day = 86400;
/** `?library=live` previews a library of channels alone (no films or series), as Home shows it. */
const liveOnly = new URLSearchParams(location.search).get("library") === "live";
// ?trailer=<YouTube id> gives every title that trailer, to try trailers behind title pages; the showcase itself has none.
const demoTrailer = new URLSearchParams(location.search).get("trailer");
// The dev server would compile a .ts video segment as TypeScript, so live
// channels play a sample film too.
const sampleFilm = new URL("/Website/showcase/art/media/film-film-1002-121.mp4", location.origin).href;
const sampleLive = sampleFilm;

const films = (liveOnly ? [] : (data.films as DemoFilm[])).map(
  (f, i): Movie => ({
    id: `${S}|mv|${f.id}`,
    sourceId: S,
    title: f.title,
    year: f.year,
    posterUrl: f.poster,
    backdropUrl: f.backdrop,
    categoryId: `${S}|moviecat|${f.category}`,
    rating: f.rating,
    addedAt: now() - i * 3 * day,
    plot: f.plot,
    genre: f.genre,
    tmdbId: null,
    imdbId: null,
    durationSeconds: f.runtime * 60,
    stream: { type: "url", url: sampleFilm },
  }),
);

const shows = (liveOnly ? [] : (data.series as DemoSeries[])).map(
  (s, i): Series => ({
    id: `${S}|se|${s.id}`,
    sourceId: S,
    title: s.title,
    year: s.year,
    posterUrl: s.poster,
    backdropUrl: s.backdrop,
    categoryId: `${S}|seriescat|${s.category}`,
    rating: s.rating,
    addedAt: now() - i * 4 * day,
    plot: s.plot,
    genre: s.genre,
    tmdbId: null,
    imdbId: null,
    remoteId: s.id,
  }),
);

const episodesBySeries = new Map<string, Episode[]>(
  (data.series as DemoSeries[]).map((s) => {
    const seriesId = `${S}|se|${s.id}`;
    const eps: Episode[] = [];
    const seasons = s.seasons.length ? s.seasons : [[1, 2, 3, 4, 5, 6].map((n) => ({ title: `Episode ${n}`, plot: null, still: null }))];
    seasons.forEach((season, si) =>
      season.forEach((e, ei) =>
        eps.push({
          id: `${seriesId}|ep|s${si + 1}e${ei + 1}`,
          seriesId,
          sourceId: S,
          season: si + 1,
          number: ei + 1,
          title: e.title,
          plot: e.plot,
          durationSeconds: (48 + ((ei * 7) % 9)) * 60,
          stillUrl: e.still ?? s.backdrop,
          stream: { type: "url", url: sampleFilm },
          airDate: Date.UTC(s.year + si, 2, 14 + ei * 7) / 1000,
        }),
      ),
    );
    return [seriesId, eps];
  }),
);

// A few episodes aired lately: The Meridian Files' next one (after the one
// watched) and the newest of two series on the watchlist, so Continue
// Watching says "New Episode" and "Latest Episode", and Signal & Noise has a
// new episode found by the six-hourly check.
const airedAgo = (seriesKey: string, episodeKey: string, days: number) => {
  const episode = episodesBySeries.get(`${S}|se|${seriesKey}`)?.find((e) => e.id.endsWith(`|ep|${episodeKey}`));
  if (episode) episode.airDate = now() - days * day;
};
airedAgo("2002", "s1e5", 3);
airedAgo("2005", "s1e8", 2);
airedAgo("2006", "s1e6", 30);
/** Series with new episodes not looked at yet (the check's badges), and when each was found. */
const unseenEpisodes = new Map<string, { count: number; foundAt: number }>(liveOnly ? [] : [[`${S}|se|2005`, { count: 1, foundAt: now() - 3600 }]]);

const categoryList: Category[] = [
  ...(data.channelCategories as [string, string][]).map(([key, name], i) => ({ id: `${S}|livecat|${key}`, sourceId: S, kind: "live" as const, name, sortIndex: i })),
  ...(data.filmCategories as [string, string][]).map(([key, name], i) => ({ id: `${S}|moviecat|${key}`, sourceId: S, kind: "movie" as const, name, sortIndex: i })),
  ...(data.seriesCategories as [string, string][]).map(([key, name], i) => ({ id: `${S}|seriescat|${key}`, sourceId: S, kind: "series" as const, name, sortIndex: i })),
];

// Providers list some channels more than once; a few demo channels come in
// extra versions (same guide), so Live TV shows them grouped.
const extraVersions: Record<string, [key: string, name: string][]> = {
  "101": [
    ["101-fhd", "Meridian 24 FHD"],
    ["101-backup", "UK | Meridian 24 (backup)"],
  ],
  "201": [["201-4k", "Fieldhouse Sports 4K"]],
};

const channelList: Channel[] = data.channels
  .flatMap((c) => [
    { ...c, key: c.id },
    ...(extraVersions[c.id] ?? []).map(([key, name]) => ({ ...c, key, name })),
  ])
  .map((c, i) => ({
    id: `${S}|ch|${c.key}`,
    sourceId: S,
    name: c.name,
    number: c.number,
    logoUrl: c.logo,
    categoryId: `${S}|livecat|${c.category}`,
    epgId: c.id,
    stream: { type: "url", url: sampleLive },
    catchupDays: c.category === "news" || c.category === "sports" ? 3 : 0,
    catchupSource: null,
    sortIndex: i,
    catchupOffsetMinutes: 0,
  }));

/** The channel whose listings a channel shows: an extra version shows its original's. */
const listingsOf = (channelId: string) => {
  const channel = channelList.find((c) => c.id === channelId);
  return channel ? `${S}|ch|${channel.epgId}` : channelId;
};

type Pool = { title: string; minutes: number; summary: string; art: string | null; live: boolean }[];

/** Each demo channel category's guide category, and the kind xorvio-core's genres.rs reads from it. */
const demoCategories: Record<string, [string, ProgramGenre | null]> = {
  news: ["News", "news"],
  sports: ["Sports", "sport"],
  movies: ["Movie", "film"],
  ent: ["Series", "entertainment"],
  kids: ["Children", "kids"],
  doc: ["Documentary", "factual"],
  music: ["Music", "music"],
  life: ["Lifestyle", null],
};

/** Two days of listings either side of now, laid out the same way on every load. */
function buildGuide(): Map<string, Program[]> {
  const guide = new Map<string, Program[]>();
  const start0 = new Date();
  start0.setHours(0, 0, 0, 0);
  const from = Math.floor(start0.getTime() / 1000) - day;
  data.channels.forEach((c, ci) => {
    const pool = (data.programmes as Record<string, Pool>)[c.category] ?? [];
    const programs: Program[] = [];
    let t = from - ((ci * 17) % 50) * 60;
    let i = ci;
    while (t < from + 3 * day && pool.length) {
      const p = pool[i % pool.length];
      const minutes = p.minutes;
      programs.push({
        id: `${S}|${c.id}|${t}`,
        sourceId: S,
        epgId: c.id,
        start: t,
        end: t + minutes * 60,
        title: p.title,
        subtitle: null,
        summary: p.summary,
        category: demoCategories[c.category]?.[0] ?? null,
        imageUrl: p.art,
        episode: null,
        flags: { isNew: i % 5 === 0, isLive: p.live, isPremiere: false, isRepeat: false },
        ageRating: null,
        starRating: null,
        year: null,
        genre: demoCategories[c.category]?.[1] ?? null,
      });
      t += minutes * 60;
      i += 1;
    }
    guide.set(`${S}|ch|${c.id}`, programs);
  });
  return guide;
}

const guideByChannel = buildGuide();

// Guides added in Sources (the mock can't load them) and guide channels
// chosen by hand. The demo library's own guide is the only one with listings.
const guideSources = new Map<string, GuideSourceSummary>();
const guideOverrides = new Map<string, { guideId: string; epgId: string }>();

function emptyStatus(): SourceStatus {
  return {
    lastSuccess: null,
    lastAttempt: null,
    lastError: null,
    guideLastSuccess: null,
    guideLastError: null,
    channelCount: 0,
    movieCount: 0,
    seriesCount: 0,
    programCount: 0,
    capabilities: { live: false, movies: false, series: false, guide: false, replay: false },
    maxConnections: null,
    accountExpires: null,
    authenticationExpired: false,
    serverTimeZone: null,
    catalogWarning: null,
  };
}

// Media folders added in the preview. The mock can't read folders, so a
// refresh makes up what one would find; folders on "Archive" stand for a
// drive that isn't connected.
const folderSources: SourceSummary[] = [];
// With ?library, the preview starts with a media folder of clips (mockLibrary.ts).
if (typeof location !== "undefined" && new URLSearchParams(location.search).has("library")) {
  const folders = ["/Volumes/storage/videos"];
  folderSources.push({
    config: {
      id: "folder-videos",
      name: "videos",
      kind: "folder",
      displayHost: "videos on storage",
      filePath: null,
      folders,
      isPaused: false,
      liveFormat: "hls",
      refreshInterval: "daily",
      userAgent: null,
      createdAt: Math.floor(Date.now() / 1000),
      sortIndex: 1,
      guideOffsetMinutes: 0,
    },
    status: emptyStatus(),
    refreshing: false,
  });
}
const sampleFolders = [["/Users/you/Movies"], ["/Volumes/Media/TV Shows", "/Volumes/Media/Kids"], ["/Volumes/Archive/Films"]];
let folderPicks = 0;

const folderName = (path: string) => path.split("/").filter(Boolean).pop() ?? path;

/** As the app describes them: "~/Movies", "TV Shows on Media", "… and 1 more". */
function describeFolders(folders: string[]) {
  if (!folders.length) return null;
  const [, volume] = folders[0].match(/^\/Volumes\/([^/]+)/) ?? [];
  const first = volume ? (folders[0].split("/").length > 3 ? `${folderName(folders[0])} on ${volume}` : `On ${volume}`) : folders[0].replace(/^\/Users\/[^/]+/, "~");
  return folders.length > 1 ? `${first} and ${folders.length - 1} more` : first;
}

// The preview starts as a first run, with no sources, when its address has
// ?firstRun; the demo library is there once it's "added".
let demoAdded = typeof location === "undefined" || !new URLSearchParams(location.search).has("firstRun");
/** Source ids in the order they were put in, for Live TV's channel order. */
let sourceOrder: string[] = [];
// Refreshes in the preview walk through the app's steps, and can be stopped.
const refreshing = new Set<string>();
const stopping = new Set<string>();
const progressListeners = new Set<(progress: RefreshProgress) => void>();
const finishedListeners = new Set<(sourceId: string) => void>();
let unlockedAt: number | null = null;

/** The twin of xorvio-core's xtream_login_in_link. */
function xtreamLoginIn(link: string): SourceSecrets | null {
  try {
    const url = new URL(link.trim());
    const username = url.searchParams.get("username");
    const password = url.searchParams.get("password");
    if (!url.pathname.endsWith("get.php") || !username || !password) return null;
    return { serverUrl: `${url.origin}${url.pathname.replace(/\/get\.php$/, "")}`, username, password };
  } catch {
    return null;
  }
}

function scanFolders(summary: SourceSummary): SourceStatus {
  const folders = summary.config.folders ?? [];
  const away = folders.filter((f) => f.startsWith("/Volumes/Archive"));
  const here = folders.length - away.length;
  const missing = away.length ? `“${folderName(away[0])}” isn't available right now. Connect the drive or network share it's on, then refresh.` : null;
  const status: SourceStatus = here
    ? { ...summary.status, lastAttempt: now(), lastSuccess: now(), lastError: null, catalogWarning: missing, movieCount: here * 4, seriesCount: here, capabilities: { ...summary.status.capabilities, movies: true, series: true } }
    : { ...summary.status, lastAttempt: now(), lastError: missing };
  summary.status = status;
  return status;
}

/** A channel's listings, following a guide channel chosen by hand. */
function programsFor(channelId: string): Program[] {
  const choice = guideOverrides.get(channelId);
  if (!choice) return guideByChannel.get(listingsOf(channelId)) ?? [];
  const shown = channelList.find((c) => c.epgId === choice.epgId);
  return choice.guideId === S && shown ? (guideByChannel.get(shown.id) ?? []) : [];
}

function guideChannel(channel: Channel): GuideChannel {
  const t = now();
  return {
    guideId: S,
    guideName: "Demo Library",
    epgId: channel.epgId!,
    name: channel.name,
    nowTitle: guideByChannel.get(channel.id)?.find((p) => p.start <= t && t < p.end)?.title ?? null,
  };
}

/** Words as search compares them: in lower case, without accents. */
function searchWords(text: string): string[] {
  return text.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
}

/** Films and series found by the people in them, their genres or a few words of their story, as Core finds them after names: people first, then genres, then stories. */
function termMatches<T extends Movie | Series>(text: string, items: T[]): { found: [T, string][]; people: [string, string][] } {
  const wanted = searchWords(text);
  const letters = wanted.join("").length;
  const everyWord = (name: string) => {
    const have = searchWords(name);
    return wanted.length > 0 && wanted.every((w) => have.some((h) => h.startsWith(w)));
  };
  const byPeople: [T, string][] = [];
  const byGenre: [T, string][] = [];
  const byStory: [T, string][] = [];
  const people: [string, string][] = [];
  if (letters < 3) return { found: [], people };
  for (const item of items) {
    const d = detailsFor(item.id);
    const credits: [string, string][] = [...(d.director ? [[d.director, "Directed by"] as [string, string]] : []), ...d.creators.map((n): [string, string] => [n, "Created by"]), ...d.people.map((p): [string, string] => (p.name.startsWith("Narrated by ") ? [p.name.slice(12), "Narrated by"] : [p.name, "With"]))];
    const person = credits.find(([name]) => everyWord(name));
    const genre = d.genres.find(everyWord);
    if (person) {
      byPeople.push([item, `${person[1]} ${person[0]}`]);
      people.push([person[0], item.id]);
    } else if (genre) byGenre.push([item, genre]);
    else if (letters >= 4 && item.plot && everyWord(item.plot)) {
      const words = item.plot.split(/\s+/);
      const at = Math.max(0, words.findIndex((w) => searchWords(w).some((h) => h.startsWith(wanted[0]))));
      const from = Math.max(0, at - 4);
      byStory.push([item, `${from > 0 ? "…" : ""}${words.slice(from, at + 5).join(" ")}${at + 5 < words.length ? "…" : ""}`]);
    }
  }
  return { found: [...byPeople, ...byGenre, ...byStory], people };
}

function detailsFor(itemId: string): MediaDetails {
  const film = (data.films as DemoFilm[]).find((f) => `${S}|mv|${f.id}` === itemId);
  const show = (data.series as DemoSeries[]).find((s) => `${S}|se|${s.id}` === itemId);
  const people = (film?.cast ?? show?.cast ?? []).map((name) => ({ name, role: null, photoUrl: null, tmdbId: null }));
  const fourK = film?.video.startsWith("4k");
  return {
    backdropUrls: [film?.backdrop ?? show?.backdrop].filter(Boolean) as string[],
    logoUrl: null,
    people,
    director: film?.director ?? null,
    creators: show?.creators ?? [],
    releaseDate: null,
    country: null,
    ageRating: film?.age ?? show?.age ?? null,
    trailerYoutubeId: demoTrailer,
    genres: (film?.genre ?? show?.genre ?? "").split(", ").filter(Boolean),
    runtimeMinutes: film?.runtime ?? null,
    imdbId: null,
    tagline: film?.tagline ?? null,
    overview: null,
    network: show?.network ?? null,
    video: fourK ? { width: 3840, height: 2160, codec: "hevc", isHdr: film!.video.includes("hdr") } : film ? { width: 1920, height: 1080, codec: "h264", isHdr: false } : null,
    audioChannels: fourK ? 6 : 2,
    seasons: show ? show.seasons.map((s, i) => ({ number: i + 1, name: null, overview: null, airDate: `${show.year + i}-03-14`, coverUrl: null, episodeCount: s.length })) : [],
    attribution: null,
    // The demo's own scores; TMDB's (and their videos) never reach it.
    ratings: [],
    videos: [],
    originalTitle: null,
  };
}

// Library state, seeded so Home looks lived-in.
/** Viewing History, newest first, seeded with a few of the showcase's titles. */
const viewingHistory: HistoryEntry[] = [
  ...films.slice(0, 2).map((film, i) => ({ itemId: film.id, kind: "movie" as const, sourceId: S, title: film.title, subtitle: null, watchedAt: now() - (i + 1) * 5400 })),
  ...channelList.slice(0, 2).map((channel, i) => ({ itemId: channel.id, kind: "channel" as const, sourceId: S, title: channel.name, subtitle: null, watchedAt: now() - 86400 - i * 3600 })),
];

/** A few lines like the ones the real log keeps. */
const diagnosticEvents: DiagnosticEvent[] = [
  { id: 1, at: Date.now() - 3_600_000, level: "info", area: "Sources", message: "refreshed Demo: 48 channels, 120 films, 36 series" },
  { id: 2, at: Date.now() - 1_900_000, level: "warning", area: "Guide", message: "the guide at http://guide.example/epg.xml?token=••• took 14 s to answer" },
  { id: 3, at: Date.now() - 600_000, level: "error", area: "Playback", message: "Northline News couldn't be played: the provider closed the connection" },
  { id: 4, at: Date.now() - 120_000, level: "warning", area: "Network", message: "image server http://art.example:8080 didn't answer; posters will show placeholders" },
];

const progress = new Map<string, PlaybackProgress>();
const seedProgress = (itemId: string, kind: "movie" | "episode", position: number, duration: number, seriesId: string | null, ago: number) =>
  progress.set(itemId, { itemId, kind, sourceId: S, seriesId, positionSeconds: position, durationSeconds: duration, finished: false, updatedAt: now() - ago });
if (!liveOnly) {
  seedProgress(`${S}|se|2001|ep|s1e3`, "episode", 22 * 60, 52 * 60, `${S}|se|2001`, 600);
  seedProgress(`${S}|se|2002|ep|s1e4`, "episode", 52 * 60, 52 * 60, `${S}|se|2002`, 1800);
  progress.get(`${S}|se|2002|ep|s1e4`)!.finished = true;
  seedProgress(`${S}|mv|1001`, "movie", 62 * 60 + 2, 138 * 60, null, 3600);
  seedProgress(`${S}|mv|1009`, "movie", 31 * 60, 141 * 60, null, 7200);
}

const watchlist = new Map<string, WatchlistEntry>();
const favorites = new Set<string>([`${S}|ch|101`, `${S}|ch|201`, `${S}|ch|204`]);
const recents: string[] = [`${S}|ch|201`, `${S}|ch|101`];
// A sync folder is already chosen in the demo (choosing one needs the desktop app).
let settings: Settings = { ...defaultSettings, syncFolder: "/Users/you/Library/Mobile Documents/com~apple~CloudDocs/Xorvio" };
const recordings = new Map<string, Recording>();
const recordingListeners = new Set<(r: Recording) => void>();
const libraryListeners = new Set<() => void>();

function seedWatchlist() {
  for (const channelId of [`${S}|ch|204`, `${S}|ch|601`, `${S}|ch|205`]) {
    const next = guideByChannel.get(channelId)?.find((p) => p.start > now() + 1800);
    const channel = channelList.find((c) => c.id === channelId)!;
    if (next)
      watchlist.set(next.id, {
        id: next.id,
        kind: "program",
        sourceId: S,
        title: next.title,
        subtitle: channel.name,
        artworkUrl: next.imageUrl,
        addedAt: now() - 100,
        channelId,
        start: next.start,
        end: next.end,
        remind: true,
      });
  }
  if (liveOnly) return;
  for (const id of ["1007", "1008"]) {
    const m = films.find((f) => f.id === `${S}|mv|${id}`)!;
    watchlist.set(m.id, { id: m.id, kind: "movie", sourceId: S, title: m.title, subtitle: String(m.year), artworkUrl: m.backdropUrl, addedAt: now() - 5000, channelId: null, start: null, end: null, remind: false });
  }
  // Two series, followed for new episodes.
  for (const [id, ago] of [["2005", 7000], ["2006", 9000]] as const) {
    const show = shows.find((s) => s.id === `${S}|se|${id}`)!;
    watchlist.set(show.id, { id: show.id, kind: "series", sourceId: S, title: show.title, subtitle: String(show.year), artworkUrl: show.posterUrl, addedAt: now() - ago, channelId: null, start: null, end: null, remind: false });
  }
}
seedWatchlist();

function changed() {
  libraryListeners.forEach((l) => l());
}

// Channel groups, names, hiding and locks, kept in memory like the rest. The
// real app stores only a salted hash of the PIN; the mock forgets it on reload.
const groups: FavoriteList[] = [];
const edits = new Map<string, { displayName: string | null; hidden: boolean; locked: boolean | null }>();
let pin: string | null = null;
let plexChecks = 0;

// Trakt in the demo: connecting approves on the third check, as "demo-viewer".
let trakt: TraktStatus = {
  configured: true,
  customApp: false,
  connected: false,
  username: null,
  lastSync: null,
  lastError: null,
  syncing: false,
  options: { scrobble: true, syncHistory: true, syncWatchlist: true },
};
let traktChecks = 0;

/** Trakt's lists, as the demo has them: a few to choose from, one of them the connected account's own. */
const traktLists: (TraktListInfo & { mine?: boolean; liked?: boolean })[] = [
  { user: "justin", list: "2142753", name: "IMDb Top 250", description: "The top rated films on IMDb, kept up to date.", owner: "Justin N.", itemCount: 250, likes: 12480, liked: true },
  { user: "garycrawford", list: "1048", name: "Best of 2025", description: "The year's films people loved most.", owner: "Gary Crawford", itemCount: 120, likes: 3204 },
  { user: "sean", list: "88017", name: "Comfort Rewatches", description: null, owner: "Sean", itemCount: 64, likes: 911 },
  { user: "demo-viewer", list: "501", name: "Weekend Picks", description: "Saved for Saturday.", owner: "demo-viewer", itemCount: 12, likes: 0, mine: true },
];

/** A Trakt row from the showcase: charts lean on the newest titles, lists on the best rated, in a steady order. */
function traktRowOf(source: TraktRowSource, limit: number): TraktRow {
  const out = excluded();
  const shown = (t: { id: string; categoryId: string | null }) => !out.has(t.categoryId ?? "") && !out.has(t.id);
  const byRating = <T extends { rating: number | null }>(list: T[]) => [...list].sort((a, b) => (b.rating ?? 0) - (a.rating ?? 0));
  const byYear = <T extends { year: number | null; rating: number | null }>(list: T[]) => [...list].sort((a, b) => (b.year ?? 0) - (a.year ?? 0) || (b.rating ?? 0) - (a.rating ?? 0));
  type Title = { movie: (typeof films)[number] } | { series: (typeof shows)[number] };
  const movies = (list: typeof films) => list.map((movie): Title => ({ movie }));
  const series = (list: typeof shows) => list.map((series): Title => ({ series }));
  // Trakt's list, every third title one the sources lack, ranked in its own order; then what can be played.
  const row = (listed: Title[]): TraktRow => ({
    listed: listed.length + Math.floor(listed.length / 3),
    items: listed
      .map((title, i): TraktRowItem => ({ ...title, rank: i + 1 + Math.floor(i / 2) }))
      .filter((item) => shown("movie" in item ? item.movie : item.series))
      .slice(0, limit),
  });
  if (source.type === "list") {
    if (source.list === "2142753") return row(movies(byRating(films)));
    const mixed = [...movies(byRating(films).slice(4)), ...series(byRating(shows).slice(2))];
    return row(mixed.filter((_, i) => i % ((Number(source.list) % 3) + 2) !== 1));
  }
  if (source.type === "recommended") return row(source.media === "movies" ? movies(byRating(films).reverse()) : series(byRating(shows).reverse()));
  if (source.chart === "boxOffice" && source.media === "shows") return { items: [], listed: 0 };
  const skip = { trending: 0, popular: 3, watched: 1, boxOffice: 2 }[source.chart];
  return row((source.media === "movies" ? movies(byYear(films)) : series(byYear(shows))).slice(skip));
}
const traktListeners = new Set<(status: TraktStatus) => void>();
function traktChanged(patch: Partial<TraktStatus>) {
  trakt = { ...trakt, ...patch };
  traktListeners.forEach((listener) => listener(trakt));
}

// Backup and sync in the demo: two other computers share the sync folder
// once sync is on, and the demo backup to restore opens with "demo
// passphrase".
let lastMockSync: number | null = null;
let mockSyncing = false;
const syncListeners = new Set<(status: SyncStatus) => void>();
function mockSyncStatus(): SyncStatus {
  const on = settings.syncEnabled && !!settings.syncFolder;
  return {
    enabled: settings.syncEnabled,
    folder: settings.syncFolder,
    syncing: mockSyncing,
    lastSync: on ? lastMockSync : null,
    lastError: null,
    notice: null,
    thisComputer: "This Mac",
    computers: on && lastMockSync
      ? [
          { name: "Studio Mac", lastSeen: now() - 40 * 60 },
          { name: "MacBook Air", lastSeen: now() - 2 * day },
        ]
      : [],
  };
}
async function mockSync() {
  mockSyncing = true;
  syncListeners.forEach((listener) => listener(mockSyncStatus()));
  await sleep(700);
  mockSyncing = false;
  lastMockSync = now();
  const status = mockSyncStatus();
  syncListeners.forEach((listener) => listener(status));
  return status;
}
let unlocked = false;

// TMDB in the demo: a key is "checked" by its shape alone. The real app never
// sends the demo library to TMDB, so pages look the same with or without one.
let tmdb: TmdbStatus = { configured: false, customKey: false, builtIn: false, rejected: false };

/** The twin of xorvio-core's parental::looks_adult. */
function looksAdult(name: string) {
  let folded = name.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase();
  for (const phrase of ["adult swim", "young adult"]) folded = folded.replaceAll(phrase, " ");
  return /(?:^|[^\p{L}\p{N}])(?:adult(?:s|e|es|o|os|a|as|i)?|x{3,}|porn\p{L}*|eroti[ckq]\p{L}*|hentai|nsfw|yetiskin\p{L}*|erwachsene)(?:[^\p{L}\p{N}]|$)|(?:^|[^\p{N}])(?:18\s*\+|\+\s*18)(?:[^\p{N}]|$)/u.test(folded);
}

function editKind(id: string): EditKind | null {
  if (id.includes("|ep|")) return null;
  const kinds: Record<string, EditKind> = { ch: "channel", livecat: "liveCategory", moviecat: "movieCategory", seriescat: "seriesCategory", mv: "movie", se: "series" };
  return kinds[id.split("|")[1]] ?? null;
}

function editItem(id: string, change: (e: { displayName: string | null; hidden: boolean; locked: boolean | null }) => void, titlesToo = false) {
  const kind = editKind(id);
  if (!kind || (!titlesToo && (kind === "movie" || kind === "series"))) throw new Error("Only channels and categories can be changed this way.");
  const e = edits.get(id) ?? { displayName: null, hidden: false, locked: null };
  change(e);
  if (!e.displayName && !e.hidden && e.locked === null) edits.delete(id);
  else edits.set(id, e);
}

function lockedIds() {
  const ids = new Set<string>();
  for (const [id, e] of edits) if (e.locked === true) ids.add(id);
  for (const c of categoryList) if ((edits.get(c.id)?.locked ?? null) === null && looksAdult(c.name)) ids.add(c.id);
  return ids;
}

/** Hidden channels and categories (unless asked for), locked ones until the PIN is entered, and those the profile leaves out. */
function excluded(includeHidden = false) {
  const ids = new Set<string>();
  if (!includeHidden) for (const [id, e] of edits) if (e.hidden) ids.add(id);
  if (pin && !unlocked) lockedIds().forEach((id) => ids.add(id));
  profileExcluded({ categories: categoryList, looksAdult }).forEach((id) => ids.add(id));
  return ids;
}

/** Home's genre rows: a title whose genre field names the genre, or in a category named for it. */
function ofGenre<T extends Movie | Series>(items: T[], genre: ContentGenre | null | undefined) {
  if (!genre) return items;
  const forGenre = genreCategories({ categories: categoryList, looksAdult }, genre);
  return items.filter((i) => forGenre.has(i.categoryId ?? "") || genreMatches(genre, i.genre));
}

const channelOut = (c: Channel, out: Set<string>) => out.has(c.id) || (!!c.categoryId && out.has(c.categoryId));

/** A channel or category under the name someone gave it. */
function named<T extends { id: string; name: string }>(item: T): T {
  const name = edits.get(item.id)?.displayName;
  return name ? { ...item, name } : item;
}

// Similar channels as one row: a twin of xorvio-core's grouping.rs and
// names::version_name, as far as the demo's names need.
const preferredVersions = new Map<string, string>();
/** Audio and subtitles chosen per series, as the library's preferences keep them. */
const trackChoices = new Map<string, TrackChoice>();
const VERSION_TAGS = ["HD", "FHD", "UHD", "SD", "4K", "8K", "HEVC", "H265", "1080P", "720P", "2160P", "HDR", "50FPS", "60FPS", "BACKUP", "BKP", "ALT"];
const QUALITY: Record<string, [rank: number, label: string]> = { "4K": [4, "4K"], UHD: [4, "4K"], FHD: [3, "FHD"], "1080P": [3, "FHD"], HD: [2, "HD"], "720P": [2, "HD"], SD: [0, "SD"] };

function versionName(raw: string) {
  const text = raw.normalize("NFKC").trim();
  const words = text.toUpperCase().split(/[^\p{L}\p{N}.]+/u).filter(Boolean);
  const [rank, label] = words.map((w) => QUALITY[w]).filter(Boolean).sort((a, b) => b[0] - a[0])[0] ?? [1, null];
  const backup = words.some((w) => ["BACKUP", "BKP", "ALT"].includes(w));
  const unbracketed = text
    .replace(/^(?:\|[A-Za-z]{2,4}\|\s*|\[[A-Za-z]{2,4}\]\s*|[A-Za-z]{2,4}\s*[:|]\s+)/, "")
    .replace(/\s*[([{]([^)\]}]*)[)\]}]/g, (whole, inner: string) => (inner.split(/[\s/,|+-]+/).filter(Boolean).every((w) => VERSION_TAGS.includes(w.toUpperCase())) ? "" : whole));
  const kept = unbracketed.split(/\s+/).filter(Boolean);
  while (kept.length > 1 && (VERSION_TAGS.includes(kept[kept.length - 1].toUpperCase()) || /^[|:-]+$/.test(kept[kept.length - 1]))) kept.pop();
  const name = kept.join(" ");
  const key = name.normalize("NFKD").replace(/\p{M}/gu, "").toLowerCase().replace(/[^\p{L}\p{N}+]/gu, "");
  return { name, key, rank, label, backup };
}

const groupKey = (channel: Channel) => `${channel.categoryId ?? channel.sourceId}|${versionName(channel.name).key}`;

/** Channels as listed, similar ones folded into one row that plays the best (or chosen) version. */
function groupSimilar(list: Channel[]): Channel[] {
  const rows = new Map<string, { channel: Channel; version: ReturnType<typeof versionName> }[]>();
  for (const channel of list) {
    const key = groupKey(channel);
    rows.set(key, [...(rows.get(key) ?? []), { channel, version: versionName(channel.name) }]);
  }
  return [...rows].map(([key, versions]) => {
    if (versions.length === 1) return named(versions[0].channel);
    const chosen = versions.findIndex((v) => v.channel.id === preferredVersions.get(key));
    let playing = chosen;
    if (playing < 0) {
      playing = 0;
      versions.forEach(({ version }, i) => {
        const best = versions[playing].version;
        if (version.rank > best.rank || (version.rank === best.rank && best.backup && !version.backup)) playing = i;
      });
    }
    const row = named(versions[playing].channel);
    return {
      ...row,
      name: edits.get(row.id)?.displayName ?? versions[playing].version.name,
      versions: versions.map(({ channel, version }, i) => ({ id: channel.id, name: edits.get(channel.id)?.displayName ?? channel.name, quality: version.label, preferred: i === chosen })),
    };
  });
}

function itemEdits(): ItemEdit[] {
  const result: ItemEdit[] = [];
  // Locked ones wait for the PIN; locks kept while there's no PIN show nothing.
  const add = (edit: ItemEdit) => (edit.displayName || edit.hidden || edit.locked) && (!edit.locked || unlocked) && result.push(edit);
  for (const c of categoryList) {
    const e = edits.get(c.id);
    const automatic = !!pin && (e?.locked ?? null) === null && looksAdult(c.name);
    if (!e && !automatic) continue;
    add({ itemId: c.id, kind: editKind(c.id)!, sourceId: S, originalName: c.name, displayName: e?.displayName ?? null, hidden: !!e?.hidden, locked: automatic || (!!pin && e?.locked === true), lockedAutomatically: automatic });
  }
  for (const c of channelList) {
    const e = edits.get(c.id);
    if (e) add({ itemId: c.id, kind: "channel", sourceId: S, originalName: c.name, displayName: e.displayName, hidden: e.hidden, locked: !!pin && e.locked === true, lockedAutomatically: false });
  }
  for (const t of [...films, ...shows]) {
    if (edits.get(t.id)?.hidden) add({ itemId: t.id, kind: editKind(t.id)!, sourceId: S, originalName: `${t.title} (${t.year})`, displayName: null, hidden: true, locked: false, lockedAutomatically: false });
  }
  return result;
}

/** A twin of xorvio-core's version key: copies of a film share a TMDB or IMDb id, else a title and year. */
const versionKey = (m: Movie) => (m.tmdbId ? `t${m.tmdbId}` : m.imdbId ? `i${m.imdbId}` : `k${m.title.toLowerCase()}|${m.year ?? ""}`);

/** Films or series as Movies and Series filter them: one source, and what's been watched. */
function browseFilter<T extends Movie | Series>(items: T[], q: BrowseQuery, kind: "movie" | "series") {
  let list = q.sourceId ? items.filter((i) => i.sourceId === q.sourceId) : items;
  const watched = q.watched ?? "all";
  if (watched === "all") return list;
  if (kind === "movie") {
    const finished = (m: T) => films.some((f) => versionKey(f) === versionKey(m as Movie) && progress.get(f.id)?.finished);
    const resumable = (m: T) => films.some((f) => versionKey(f) === versionKey(m as Movie) && isResumable(progress.get(f.id)));
    list = list.filter((m) => (watched === "unwatched" ? !finished(m) : resumable(m)));
  } else {
    const done = (show: T) => {
      const eps = (episodesBySeries.get(show.id) ?? []).filter((e) => e.season > 0);
      return eps.length > 0 && eps.every((e) => progress.get(e.id)?.finished);
    };
    const started = (show: T) => [...progress.values()].some((p) => p.seriesId === show.id);
    list = list.filter((show) => (watched === "unwatched" ? !started(show) : started(show) && !done(show)));
  }
  return list;
}

const isResumable = (p: PlaybackProgress | undefined) => !!p && !p.finished && p.positionSeconds > 30 && p.positionSeconds / p.durationSeconds < 0.95;

/** One poster per film: the first copy in the list's order. */
function oneCopyEach(list: Movie[]) {
  const seen = new Set<string>();
  return list.filter((m) => !seen.has(versionKey(m)) && !!seen.add(versionKey(m)));
}

function titleMarks(kind: "movie" | "series", ids: string[]): TitleMark[] {
  const out = excluded();
  return ids.map((id) => {
    if (kind === "series") {
      const eps = (episodesBySeries.get(id) ?? []).filter((e) => e.season > 0);
      const watched = eps.filter((e) => progress.get(e.id)?.finished).length;
      const done = eps.length > 0 && watched === eps.length;
      return { id, versions: 1, watched: done, progress: !done && watched > 0 ? watched / eps.length : null };
    }
    const film = films.find((f) => f.id === id);
    const copies = film ? films.filter((f) => versionKey(f) === versionKey(film) && (f.id === id || !out.has(f.id))) : [];
    const played = copies.map((c) => progress.get(c.id)).filter((p): p is PlaybackProgress => !!p);
    const latest = played.filter(isResumable).sort((a, b) => b.updatedAt - a.updatedAt)[0];
    return { id, versions: Math.max(1, copies.length), watched: played.some((p) => p.finished), progress: latest ? latest.positionSeconds / latest.durationSeconds : null };
  });
}

function checkPin(given: string | null) {
  if (pin && given !== pin) throw new Error("That PIN isn't correct.");
}

function listOf(id: string): string[] {
  if (id === FAVORITES_LIST) return [...favorites];
  const group = groups.find((g) => g.id === id);
  if (!group) throw new Error("This group no longer exists.");
  return group.channelIds;
}

function groupName(name: string, renaming?: string) {
  const trimmed = name.trim();
  if (!trimmed) throw new Error("A group needs a name.");
  if (groups.some((g) => g.id !== renaming && g.name.toLowerCase() === trimmed.toLowerCase())) throw new Error(`There's already a group called “${trimmed}”.`);
  return trimmed;
}

function emitRecording(r: Recording) {
  recordings.set(r.id, r);
  // Whether it clashes depends on the others, as the recorder works it out.
  const shown = withConflicts([...recordings.values()]).find((c) => c.id === r.id) ?? r;
  recordingListeners.forEach((l) => l(shown));
}

function makeRecording(req: ScheduleRequest, status: Recording["status"], extra: Partial<Recording> = {}): Recording {
  return {
    id: crypto.randomUUID(),
    ...req,
    status,
    filePath: status === "recording" ? `~/Movies/Xorvio/${req.channelName} - ${req.title}.ts` : null,
    fileSizeBytes: status === "recording" ? 0 : null,
    recordedSeconds: status === "recording" ? 0 : null,
    error: null,
    createdAt: now(),
    ruleId: null,
    episode: null,
    fromCatchup: false,
    note: null,
    conflict: null,
    ...extra,
  };
}

// Recordings in the mock "record" by counting seconds; one from catch-up
// goes faster than the clock, as the provider's archive sends it.
setInterval(() => {
  const t = now();
  for (const r of recordings.values()) {
    if (r.status === "scheduled" && t >= r.start - r.paddingBeforeSeconds) emitRecording({ ...r, status: "recording", filePath: `~/Movies/Xorvio/${r.channelName} - ${r.title}.ts`, recordedSeconds: 0, fileSizeBytes: 0 });
    else if (r.status === "recording" && r.fromCatchup) {
      const length = r.end + r.paddingAfterSeconds - (r.start - r.paddingBeforeSeconds);
      const saved = Math.min(length, (r.recordedSeconds ?? 0) + 40);
      emitRecording({ ...r, status: saved >= length ? "completed" : "recording", recordedSeconds: saved, fileSizeBytes: Math.round(saved * 640_000) });
    } else if (r.status === "recording") {
      if (t >= r.end + r.paddingAfterSeconds) emitRecording({ ...r, status: "completed" });
      else emitRecording({ ...r, recordedSeconds: (r.recordedSeconds ?? 0) + 5, fileSizeBytes: (r.fileSizeBytes ?? 0) + 5 * 1_000_000 });
    }
  }
}, 5000);

// MARK: Series recordings and conflicts in the demo

/** The demo provider allows this many streams at once, so overlapping recordings show as conflicts. */
const DEMO_STREAMS = 2;
const seriesRules = new Map<string, SeriesRule>();
/** Airings ("<channel>|<start>") of each series that were cancelled: they aren't scheduled again. */
const skippedAirings = new Map<string, Set<string>>();
const recordingsListeners = new Set<() => void>();
const recordingsChanged = () => recordingsListeners.forEach((l) => l());

/** The twin of xorvio-core's watchlist::normalized. */
function mockTitleKey(title: string) {
  const words = title
    .toLowerCase()
    .replace(/@/g, " at ")
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean)
    .map((w) => (["v", "vs", "versus"].includes(w) ? "at" : w));
  if (words.length > 1 && words[0] === "live") words.shift();
  return words.join(" ");
}

/** Recordings beyond the provider's streams, as the recorder's rules.rs marks them. */
function withConflicts(list: Recording[]): Recording[] {
  const t = now();
  const active = list.filter((r) => r.status === "scheduled" || r.status === "recording");
  const window = (r: Recording): [number, number] => {
    const from = r.start - r.paddingBeforeSeconds;
    const until = r.end + r.paddingAfterSeconds;
    return r.fromCatchup ? [r.createdAt, Math.max(r.createdAt + (until - from), t + 60)] : [from, until];
  };
  const worst = new Map<string, number>();
  for (const r of active) {
    const [at] = window(r);
    const together = active.filter((o) => o.sourceId === r.sourceId && window(o)[0] <= at && at < window(o)[1]);
    if (together.length > DEMO_STREAMS) together.forEach((o) => worst.set(o.id, Math.max(worst.get(o.id) ?? 0, together.length)));
  }
  return list.map((r) => {
    const n = worst.get(r.id);
    const conflict = n ? `Clashes with ${n - 1} other recording${n - 1 === 1 ? "" : "s"}: your provider allows ${DEMO_STREAMS} streams at a time.` : null;
    return { ...r, conflict };
  });
}

/** The series with its next airing and counts, as the recorder lists them. */
function countedRules(): SeriesRule[] {
  const all = withConflicts([...recordings.values()]);
  return [...seriesRules.values()]
    .map((rule) => {
      const mine = all.filter((r) => r.ruleId === rule.id);
      const scheduled = mine.filter((r) => r.status === "scheduled").sort((a, b) => a.start - b.start);
      return {
        ...rule,
        nextAiring: scheduled[0]?.start ?? null,
        nextChannelName: scheduled[0]?.channelName ?? null,
        scheduledCount: scheduled.length,
        recordedCount: mine.filter((r) => r.status === "completed" || r.status === "partial").length,
        conflictCount: mine.filter((r) => r.conflict).length,
      };
    })
    .sort((a, b) => a.title.localeCompare(b.title));
}

/** Schedules a series' airings in the demo guide, each once, skipping cancelled ones and, for new episodes only, repeats and episodes it has. */
function planSeries(rule: SeriesRule) {
  const t = now();
  const key = mockTitleKey(rule.title);
  const channels = rule.anyChannel ? groupSimilar(channelList) : channelList.filter((c) => c.id === rule.channelId);
  const mine = [...recordings.values()].filter((r) => r.ruleId === rule.id);
  const taken = new Set([...mine.map((r) => `${versionName(r.channelName).key}|${r.start}`), ...(skippedAirings.get(rule.id) ?? [])]);
  const episodes = new Set(mine.filter((r) => r.status !== "failed" && r.status !== "cancelled").map((r) => r.episode ?? r.summary));
  const airings = channels
    .flatMap((channel) => programsFor(channel.id).map((program) => ({ program, channel })))
    .filter(({ program }) => program.start > t && mockTitleKey(program.title) === key)
    .sort((a, b) => a.program.start - b.program.start);
  for (const { program, channel } of airings) {
    const airing = `${versionName(channel.name).key}|${program.start}`;
    if (taken.has(airing)) continue;
    taken.add(airing);
    const episode = program.episode ?? program.summary;
    if (rule.newOnly && program.flags.isRepeat && !program.flags.isNew) continue;
    if (rule.newOnly && episode && episodes.has(episode) && !program.flags.isNew) continue;
    if (episode) episodes.add(episode);
    const padding = { before: rule.paddingBeforeSeconds ?? settings.recordPaddingBefore * 60, after: rule.paddingAfterSeconds ?? settings.recordPaddingAfter * 60 };
    const r = makeRecording(
      { sourceId: S, channelId: channel.id, channelName: channel.name, channelLogoUrl: channel.logoUrl, title: program.title, subtitle: program.subtitle, summary: program.summary, artworkUrl: program.imageUrl ?? rule.artworkUrl, programId: program.id, start: program.start, end: program.end, paddingBeforeSeconds: padding.before, paddingAfterSeconds: padding.after },
      "scheduled",
      { ruleId: rule.id, episode: program.episode },
    );
    recordings.set(r.id, r);
  }
}

function addSeries(program: Program, channel: Channel, options: SeriesRuleOptions): SeriesRule {
  const key = mockTitleKey(program.title);
  const existing = [...seriesRules.values()].find((r) => mockTitleKey(r.title) === key && (r.channelId === channel.id || (r.anyChannel && options.anyChannel)));
  const rule: SeriesRule = existing
    ? { ...existing, ...options }
    : {
        id: crypto.randomUUID(),
        title: program.title,
        sourceId: S,
        channelId: channel.id,
        channelName: channel.name,
        channelLogoUrl: channel.logoUrl,
        artworkUrl: program.imageUrl,
        createdAt: now(),
        nextAiring: null,
        nextChannelName: null,
        scheduledCount: 0,
        recordedCount: 0,
        conflictCount: 0,
        ...options,
      };
  seriesRules.set(rule.id, rule);
  for (const r of [...recordings.values()]) if (r.ruleId === rule.id && r.status === "scheduled") recordings.delete(r.id);
  planSeries(rule);
  return countedRules().find((r) => r.id === rule.id)!;
}

/** Recorded episodes, a catch-up save or two, a clash and a recording saved elsewhere, so Recordings looks lived-in. */
function seedRecordings() {
  const t = now();
  const lumen = channelList.find((c) => c.id === `${S}|ch|401`)!;
  const tidewater = programsFor(lumen.id).filter((p) => mockTitleKey(p.title) === "tidewater");
  const rule = addSeries(tidewater.find((p) => p.start > t) ?? tidewater[0], lumen, { anyChannel: false, newOnly: true, keep: 5, paddingBeforeSeconds: null, paddingAfterSeconds: null });
  tidewater
    .filter((p) => p.end < t)
    .slice(-2)
    .forEach((p, i) => {
      const r = makeRecording(
        { sourceId: S, channelId: lumen.id, channelName: lumen.name, channelLogoUrl: lumen.logoUrl, title: p.title, subtitle: null, summary: p.summary, artworkUrl: p.imageUrl, programId: p.id, start: p.start, end: p.end, paddingBeforeSeconds: 60, paddingAfterSeconds: 180 },
        "completed",
        { ruleId: rule.id, episode: `S2 E${i + 4}`, filePath: `~/Movies/Xorvio/${lumen.name} - ${p.title}.ts`, recordedSeconds: p.end - p.start + 240, fileSizeBytes: 2_300_000_000 },
      );
      recordings.set(r.id, r);
    });

  const fieldhouse = channelList.find((c) => c.id === `${S}|ch|201`)!;
  const matchday = programsFor(fieldhouse.id).find((p) => p.start > t && mockTitleKey(p.title) === "matchday live");
  if (matchday) addSeries(matchday, fieldhouse, { anyChannel: true, newOnly: true, keep: 3, paddingBeforeSeconds: 120, paddingAfterSeconds: 900 });

  // Three at once tomorrow evening, with two streams allowed.
  const evening = new Date();
  evening.setDate(evening.getDate() + 1);
  evening.setHours(20, 0, 0, 0);
  const at = Math.floor(evening.getTime() / 1000);
  for (const id of ["301", "601", "402"]) {
    const channel = channelList.find((c) => c.id === `${S}|ch|${id}`)!;
    const p = programsFor(channel.id).find((p) => p.start <= at && at < p.end);
    if (!p) continue;
    const r = makeRecording(
      { sourceId: S, channelId: channel.id, channelName: channel.name, channelLogoUrl: channel.logoUrl, title: p.title, subtitle: null, summary: p.summary, artworkUrl: p.imageUrl, programId: p.id, start: p.start, end: p.end, paddingBeforeSeconds: 60, paddingAfterSeconds: 180 },
      "scheduled",
    );
    recordings.set(r.id, r);
  }

  // From catch-up: one saved yesterday, one saving now.
  const meridian = channelList.find((c) => c.id === `${S}|ch|101`)!;
  const northwind = channelList.find((c) => c.id === `${S}|ch|102`)!;
  const yesterday = programsFor(meridian.id).find((p) => mockTitleKey(p.title) === "evening edition" && p.end < t - 6 * 3600);
  if (yesterday) {
    const r = makeRecording(
      { sourceId: S, channelId: meridian.id, channelName: meridian.name, channelLogoUrl: meridian.logoUrl, title: yesterday.title, subtitle: null, summary: yesterday.summary, artworkUrl: yesterday.imageUrl, programId: yesterday.id, start: yesterday.start, end: yesterday.end, paddingBeforeSeconds: 60, paddingAfterSeconds: 180 },
      "completed",
      { fromCatchup: true, filePath: `/Volumes/TV Drive/Recordings/${meridian.name} - ${yesterday.title}.ts`, recordedSeconds: yesterday.end - yesterday.start + 240, fileSizeBytes: 1_640_000_000, createdAt: t - 5 * 3600 },
    );
    recordings.set(r.id, r);
  }
  const earlier = programsFor(northwind.id).filter((p) => p.end < t - 1800 && p.end - p.start >= 3600).pop();
  if (earlier) {
    const r = makeRecording(
      { sourceId: S, channelId: northwind.id, channelName: northwind.name, channelLogoUrl: northwind.logoUrl, title: earlier.title, subtitle: null, summary: earlier.summary, artworkUrl: earlier.imageUrl, programId: earlier.id, start: earlier.start, end: earlier.end, paddingBeforeSeconds: 60, paddingAfterSeconds: 180 },
      "recording",
      {
        fromCatchup: true,
        createdAt: t - 90,
        recordedSeconds: 1500,
        fileSizeBytes: 960_000_000,
        filePath: `~/Movies/Xorvio/${northwind.name} - ${earlier.title}.ts`,
        note: "“/Volumes/TV Drive/Recordings” wasn't available, so this was saved in “~/Movies/Xorvio”.",
      },
    );
    recordings.set(r.id, r);
  }
  settings = { ...settings, sourceRecordingFolders: { [S]: "/Volumes/TV Drive/Recordings" } };
}
seedRecordings();

// MARK: Ad breaks and seek-bar thumbnails in the demo

/** Ad breaks found in the recorded episodes (as the app finds them once a recording finishes); the catch-up save waits for Find Ad Breaks. */
const adBreaks = new Map<string, AdBreaks>();
for (const r of recordings.values()) {
  if (r.status !== "completed" || r.fromCatchup) continue;
  const length = r.recordedSeconds ?? r.end - r.start;
  const at = (share: number) => Math.round(length * share);
  adBreaks.set(r.id, {
    state: "found",
    breaks: [
      { startSeconds: at(0.28), endSeconds: at(0.28) + 152 },
      { startSeconds: at(0.63), endSeconds: at(0.63) + 187 },
    ],
    foundBy: "Xorvio",
    error: null,
  });
}

/**
 * The sample film's thumbnails: a frame a minute, drawn here from the film
 * itself into sheets as the app's ffmpeg does (or plain numbered tiles when
 * the film can't be read). Made once, the first time they're asked for.
 */
let demoSheet: ThumbnailSheet | null = null;
let makingDemoSheet = false;

async function makeDemoSheet(): Promise<ThumbnailSheet> {
  const [columns, rows, width, interval] = [10, 10, 160, 60];
  const video = document.createElement("video");
  video.muted = true;
  video.preload = "auto";
  video.src = sampleFilm;
  const loaded = await new Promise<boolean>((resolve) => {
    video.onloadedmetadata = () => resolve(true);
    video.onerror = () => resolve(false);
  });
  const length = loaded && Number.isFinite(video.duration) ? video.duration : 7260;
  const height = loaded && video.videoWidth ? Math.round((width * video.videoHeight) / video.videoWidth / 2) * 2 : 90;
  const count = Math.floor(length / interval);
  const seekTo = (time: number) =>
    new Promise<boolean>((resolve) => {
      video.onseeked = () => resolve(true);
      video.onerror = () => resolve(false);
      video.currentTime = time;
    });
  const images: string[] = [];
  for (let first = 0; first < count; first += columns * rows) {
    const tiles = Math.min(columns * rows, count - first);
    const canvas = document.createElement("canvas");
    canvas.width = columns * width;
    canvas.height = Math.ceil(tiles / columns) * height;
    const context = canvas.getContext("2d")!;
    for (let i = 0; i < tiles; i++) {
      const [x, y] = [(i % columns) * width, Math.floor(i / columns) * height];
      const time = (first + i + 0.5) * interval;
      if (loaded && (await seekTo(time))) context.drawImage(video, x, y, width, height);
      else {
        context.fillStyle = `hsl(${((first + i) * 37) % 360} 35% 28%)`;
        context.fillRect(x, y, width, height);
        context.fillStyle = "#fff";
        context.font = "600 16px system-ui";
        context.fillText(`${Math.floor(time / 60)} min`, x + 10, y + 26);
      }
    }
    images.push(canvas.toDataURL("image/jpeg", 0.75));
  }
  video.removeAttribute("src");
  return { images, intervalSeconds: interval, count, columns, rows, width, height };
}

const sleep = (ms = 60) => new Promise((r) => setTimeout(r, ms));

// Settings › Storage: made-up sizes that follow what the mock holds.
const listingCount = () => [...guideByChannel.values()].reduce((n, p) => n + p.length, 0);
let temporaryBytes = 1_380_000;

function sortItems<T extends Movie | Series>(items: T[], sort: string) {
  const copy = [...items];
  if (sort === "title") copy.sort((a, b) => a.title.localeCompare(b.title));
  else if (sort === "year") copy.sort((a, b) => (b.year ?? 0) - (a.year ?? 0));
  else if (sort === "rating") copy.sort((a, b) => (b.rating ?? 0) - (a.rating ?? 0));
  else copy.sort((a, b) => (b.addedAt ?? 0) - (a.addedAt ?? 0));
  return copy;
}

/** What a next episode's card says, as xorvio-core's continue_watching::up_next_reason. */
function upNextReason(reason: "next" | "latest" | "start", airDate: number | null): ContinueItem["reason"] {
  const recent = airDate != null && airDate <= now() && now() - airDate < 14 * day;
  return reason !== "start" && recent ? "newEpisode" : reason;
}

function continueItems(): ContinueItem[] {
  const items: ContinueItem[] = [];
  const out = excluded();
  // Watchlist series not started: the newest episode while it's airing, else the first.
  for (const entry of watchlist.values()) {
    if (entry.kind !== "series" || [...progress.values()].some((p) => p.seriesId === entry.id)) continue;
    const show = shows.find((s) => s.id === entry.id);
    const eps = episodesBySeries.get(entry.id) ?? [];
    if (!show || !eps.length || out.has(show.categoryId ?? "")) continue;
    const latest = [...eps].reverse().find((e) => (e.airDate ?? 0) <= now());
    const airing = !!latest?.airDate && now() - latest.airDate <= 60 * day;
    const ep = airing ? latest! : eps[0];
    const reason = upNextReason(airing ? "latest" : "start", ep.airDate);
    const code = `S${String(ep.season).padStart(2, "0")} E${String(ep.number).padStart(2, "0")}`;
    const date = Math.max(entry.addedAt, reason === "start" ? 0 : (ep.airDate ?? 0));
    items.push({ itemId: ep.id, kind: "episode", sourceId: S, title: show.title, subtitle: `${code} · ${ep.title}`, artworkUrl: ep.stillUrl ?? show.backdropUrl, progress: 0, remainingSeconds: ep.durationSeconds, upNext: true, reason, updatedAt: date });
  }
  for (const p of [...progress.values()].sort((a, b) => b.updatedAt - a.updatedAt)) {
    if (p.kind === "movie" && !p.finished) {
      const m = films.find((f) => f.id === p.itemId)!;
      if (out.has(m.categoryId ?? "")) continue;
      items.push({ itemId: m.id, kind: "movie", sourceId: m.sourceId, title: m.title, subtitle: String(m.year), artworkUrl: m.backdropUrl, progress: p.positionSeconds / p.durationSeconds, remainingSeconds: p.durationSeconds - p.positionSeconds, upNext: false, updatedAt: p.updatedAt });
    } else if (p.kind === "episode") {
      const eps = episodesBySeries.get(p.seriesId!)!;
      const index = eps.findIndex((e) => e.id === p.itemId);
      const ep = p.finished ? eps[index + 1] : eps[index];
      if (!ep) continue;
      const show = shows.find((s) => s.id === p.seriesId)!;
      if (out.has(show.categoryId ?? "")) continue;
      const code = `S${String(ep.season).padStart(2, "0")} E${String(ep.number).padStart(2, "0")}`;
      items.push({
        itemId: ep.id,
        kind: "episode",
        sourceId: show.sourceId,
        title: show.title,
        subtitle: `${code} · ${ep.title}`,
        artworkUrl: ep.stillUrl ?? show.backdropUrl,
        progress: p.finished ? 0 : p.positionSeconds / p.durationSeconds,
        remainingSeconds: p.finished ? null : p.durationSeconds - p.positionSeconds,
        upNext: p.finished,
        reason: p.finished ? upNextReason("next", ep.airDate) : null,
        updatedAt: p.finished ? Math.max(p.updatedAt, upNextReason("next", ep.airDate) === "newEpisode" ? (ep.airDate ?? 0) : 0) : p.updatedAt,
      });
    }
  }
  return items.sort((a, b) => b.updatedAt - a.updatedAt);
}

// A pretend TV on the network, so the Cast button, its list and the casting
// state can be seen in a browser. Nothing is sent anywhere.
const livingRoomTv: CastDevice = { id: "living-room-tv", name: "Living Room TV", model: "Chromecast" };
const castListeners = new Set<(status: CastStatus) => void>();
let castStatus: CastStatus | null = null;
let castClock: ReturnType<typeof setInterval> | undefined;

function castUpdate(patch: Partial<CastStatus>) {
  if (!castStatus) return;
  castStatus = { ...castStatus, ...patch };
  const sent = castStatus;
  castListeners.forEach((listener) => listener(sent));
}

/** The pretend TV moves on a second at a time while it plays, and says so every five. */
function castTick() {
  clearInterval(castClock);
  let ticks = 0;
  castClock = setInterval(() => {
    if (!castStatus || castStatus.state !== "playing" || castStatus.live) return;
    const position = castStatus.positionSeconds + 1;
    const finished = castStatus.durationSeconds != null && position >= castStatus.durationSeconds;
    castStatus = { ...castStatus, positionSeconds: Math.min(position, castStatus.durationSeconds ?? position), state: finished ? "finished" : "playing" };
    if (finished || ++ticks % 5 === 0) castUpdate({});
  }, 1000);
}

const plainStream: ResolvedStream = {
  url: "",
  mode: "native",
  isLive: false,
  durationSeconds: null,
  startOffsetSeconds: 0,
  audioTracks: [],
  subtitleTracks: [],
  audioIndex: null,
  audioDelaySeconds: 0,
  info: { container: "MP4", video: "H.264 High", audio: "AAC · stereo", notes: [] },
};

export function createMockBackend(): Backend {
  const demoSource: SourceSummary = {
    config: { id: S, name: "Demo Library", kind: "demo", displayHost: null, filePath: null, isPaused: false, liveFormat: "hls", refreshInterval: "manual", userAgent: null, createdAt: now() - 30 * day, sortIndex: 0, guideOffsetMinutes: 0 },
    status: {
      lastSuccess: now() - 3600,
      lastAttempt: now() - 3600,
      lastError: null,
      guideLastSuccess: now() - 3600,
      guideLastError: null,
      channelCount: channelList.length,
      movieCount: films.length,
      seriesCount: shows.length,
      programCount: listingCount(),
      capabilities: { live: true, movies: true, series: true, guide: true, replay: true },
      maxConnections: 2,
      accountExpires: null,
      authenticationExpired: false,
      serverTimeZone: null,
      catalogWarning: null,
    },
    refreshing: false,
  };

  const allSources = () => [...(demoAdded ? [demoSource] : []), ...folderSources];
  const ordered = () => {
    const place = (s: SourceSummary) => (sourceOrder.includes(s.config.id) ? sourceOrder.indexOf(s.config.id) : sourceOrder.length);
    return allSources()
      .map((s, i) => ({ s, i }))
      .sort((a, b) => place(a.s) - place(b.s) || a.i - b.i)
      .map(({ s }, sortIndex) => ({ ...s, config: { ...s.config, sortIndex }, refreshing: refreshing.has(s.config.id) }));
  };

  return {
    listSources: async () => ordered(),
    addSource: async () => {
      throw new Error("Adding sources needs the desktop app.");
    },
    cancelRefresh: async (id) => {
      if (!refreshing.has(id)) return false;
      stopping.add(id);
      return true;
    },
    reorderSources: async (ids) => {
      sourceOrder = [...ids];
      changed();
    },
    testXtreamLogin: async (secrets) => {
      await sleep(600);
      if (!secrets.serverUrl || !secrets.username || !secrets.password) throw new Error("Enter a server, username and password first.");
      if (secrets.password === "wrong") throw new Error("The provider rejected the username or password.");
      return { maxConnections: 2, expires: now() + 200 * day };
    },
    xtreamLoginInLink: async (link) => xtreamLoginIn(link),
    keepDroppedPlaylist: async (name, contents) => {
      if (!/#EXTINF/.test(contents)) throw new Error(/#EXTM3U/.test(contents) ? "The playlist could not be read: it contains no playable entries" : "The playlist could not be read: it is not an M3U playlist");
      return `/Users/you/Library/Application Support/com.xorvio.desktop/Playlists/${name.split(/[\\/]/).pop() || "Playlist.m3u"}`;
    },
    setMenuState: async () => {},
    // The browser preview has no menu bar; its shortcuts are the interface's own (commands.ts).
    onMenu: () => () => {},
    onRefreshFinished: (handler) => {
      finishedListeners.add(handler);
      return () => finishedListeners.delete(handler);
    },
    addFolderSource: async (name, folders) => {
      if (!folders.length) throw new Error("This source is missing a folder. Edit the source to add it.");
      const taken = folderSources.find((s) => s.config.folders?.some((f) => folders.includes(f)));
      if (taken) throw new Error(`“${folderName(folders[0])}” is already part of “${taken.config.name}”. Add it there, or remove it from there first.`);
      const config: SourceConfig = {
        id: `folder-${crypto.randomUUID()}`,
        name: name.trim() || folderName(folders[0]),
        kind: "folder",
        displayHost: describeFolders(folders),
        filePath: null,
        folders,
        isPaused: false,
        liveFormat: "hls",
        refreshInterval: "daily",
        userAgent: null,
        createdAt: now(),
        sortIndex: folderSources.length + 1,
        guideOffsetMinutes: 0,
      };
      folderSources.push({ config, status: emptyStatus(), refreshing: false });
      // The demo's last few films move into the first folder added, so its row on Home has something to show.
      if (folderSources.length === 1) films.slice(-6).forEach((film, i) => Object.assign(film, { sourceId: config.id, addedAt: now() - i * 60 }));
      return config;
    },
    // The demo approves a Plex sign-in on the third check, with two servers.
    plexStartSignIn: async () => {
      plexChecks = 0;
      // A blank page stands in for plex.tv.
      return { pinId: 1, code: "XRV4", authUrl: "about:blank" };
    },
    plexCheckSignIn: async () =>
      ++plexChecks < 3
        ? null
        : [
            { id: "home", name: "Living Room", owned: true },
            { id: "friend", name: "Sam's Server", owned: false },
          ],
    addPlexServer: async () => {
      throw new Error("Adding sources needs the desktop app.");
    },
    signInToSourceAgain: async () => {},

    traktStatus: async () => trakt,
    traktStartSignIn: async () => {
      traktChecks = 0;
      return { userCode: "XRV4T2K9", verificationUrl: "about:blank", expiresIn: 600, interval: 2 };
    },
    traktCheckSignIn: async () => {
      if (++traktChecks < 3) return false;
      traktChanged({ connected: true, username: "demo-viewer", lastSync: Math.floor(Date.now() / 1000) });
      return true;
    },
    traktDisconnect: async () => traktChanged({ connected: false, username: null, lastSync: null, lastError: null }),
    traktSyncNow: async () => {
      traktChanged({ lastSync: Math.floor(Date.now() / 1000), lastError: null });
      return trakt;
    },
    traktSetOptions: async (options) => traktChanged({ options }),
    traktSetApp: async (clientId) => traktChanged({ customApp: !!clientId.trim() }),
    traktScrobble: async () => {},
    traktRow: async (source, limit) => {
      await sleep(120);
      if (source.type === "recommended" && !trakt.connected) throw new Error("Xorvio isn't connected to Trakt.");
      return traktRowOf(source, limit);
    },
    // Vite serves the gateway's own page, so the preview plays trailers as the app does.
    trailerPage: async (youtubeId) => `${location.origin}/crates/xorvio-media/src/gateway/trailer.html?v=${encodeURIComponent(youtubeId)}`,
    traktLists: async (query) => {
      await sleep(150);
      if ((query.type === "mine" || query.type === "liked") && !trakt.connected) throw new Error("Xorvio isn't connected to Trakt.");
      const plain = ({ mine: _mine, liked: _liked, ...info }: (typeof traktLists)[number]): TraktListInfo => info;
      switch (query.type) {
        case "mine":
          return traktLists.filter((l) => l.mine).map(plain);
        case "liked":
          return traktLists.filter((l) => l.liked).map(plain);
        case "popular":
          return traktLists.filter((l) => !l.mine).map(plain);
        case "search": {
          const text = query.text.trim().toLowerCase();
          if (!text) return [];
          const address = /trakt\.tv\/(?:users\/([\w.-]+)\/)?lists\/([\w.-]+)/.exec(text);
          return traktLists
            .filter((l) => (address ? l.user === (address[1] ?? l.user) && (l.list === address[2] || l.name.toLowerCase().replace(/\W+/g, "-") === address[2]) : l.name.toLowerCase().includes(text)))
            .map(plain);
        }
      }
    },
    tmdbStatus: async () => tmdb,
    // The demo library is never sent to TMDB, so nothing waits to be looked up.
    searchLookupsLeft: async () => (tmdb.configured ? 0 : null),
    tmdbSetKey: async (key) => {
      const trimmed = key.trim();
      if (trimmed && !/^[0-9a-f]{32}$|^eyJ[\w-]+\.[\w-]+\.[\w-]+$/.test(trimmed)) throw new Error("TMDB didn't accept that key. Copy the API key or the read access token from themoviedb.org › Settings › API.");
      tmdb = { ...tmdb, configured: !!trimmed, customKey: !!trimmed, rejected: false };
      return tmdb;
    },
    onTraktChanged: (handler) => {
      traktListeners.add(handler);
      return () => traktListeners.delete(handler);
    },
    replacePlexSignIn: async () => {},
    updateSource: async (config) => {
      const folder = folderSources.find((s) => s.config.id === config.id);
      if (!folder) return;
      if (!config.folders?.length) throw new Error("This source is missing a folder. Edit the source to add it.");
      folder.config = { ...config, displayHost: describeFolders(config.folders) };
    },
    removeSource: async (id) => {
      const at = folderSources.findIndex((s) => s.config.id === id);
      if (at >= 0) folderSources.splice(at, 1);
      films.filter((film) => film.sourceId === id).forEach((film) => Object.assign(film, { sourceId: S }));
    },
    sourceSecrets: async () => ({}),
    refreshSource: async (id) => {
      const folder = folderSources.find((s) => s.config.id === id);
      const summary = folder ?? demoSource;
      if (refreshing.has(id)) return summary.status;
      refreshing.add(id);
      const steps = folder ? ["Connecting", "Reading catalog", "Organizing content"] : ["Connecting", "Reading catalog", "Organizing content", "Loading guide"];
      for (const phase of steps) {
        if (stopping.has(id)) break;
        progressListeners.forEach((listener) => listener({ sourceId: id, phase, fraction: null }));
        await sleep(650);
      }
      const stopped = stopping.delete(id);
      refreshing.delete(id);
      finishedListeners.forEach((listener) => listener(id));
      // A stopped refresh leaves the source as it was.
      if (stopped) return summary.status;
      if (folder) return scanFolders(folder);
      demoSource.status = { ...demoSource.status, lastAttempt: now(), lastSuccess: now(), guideLastSuccess: now(), lastError: null };
      return demoSource.status;
    },
    addDemoSource: async () => {
      demoAdded = true;
    },
    swiftImportAvailable: async () => false,
    importSwiftSources: async () => [],
    swiftLibraryAvailable: async () => false,
    importSwiftLibrary: async () => ({ addons: [], addonProblems: [], watchlistAdded: 0, watchlistSkipped: [], progressImported: 0 }),
    swiftDataAvailable: async () => false,
    importSwiftProgress: async () => ({ imported: 0, skipped: 0 }),

    categories: async (kind: CategoryKind, includeHidden) => {
      const out = excluded(!!includeHidden);
      return categoryList.filter((c) => c.kind === kind && !out.has(c.id)).map(named);
    },
    channels: async (filter) => {
      const out = excluded(!!filter.includeHidden);
      const listId = filter.listId ?? (filter.favoritesOnly ? FAVORITES_LIST : null);
      let list = listId ? listOf(listId).map((id) => channelList.find((c) => c.id === id)!).filter(Boolean) : channelList;
      if (filter.categoryId) list = list.filter((c) => c.categoryId === filter.categoryId);
      const shown = list.filter((c) => !channelOut(c, out)).slice(0, filter.limit ?? undefined);
      return (filter.groupSimilar ?? settings.groupSimilarChannels) ? groupSimilar(shown) : shown.map(named);
    },
    channel: async (id) => {
      const channel = channelList.find((c) => c.id === id);
      return channel ? named(channel) : null;
    },
    movies: async (q) => {
      await sleep();
      const out = excluded();
      let list = ofGenre(films.filter((m) => !out.has(m.categoryId ?? "") && !out.has(m.id)), q.contentGenre);
      if (q.categoryId) list = list.filter((m) => m.categoryId === q.categoryId);
      if (q.genre) list = list.filter((m) => m.genre?.includes(q.genre!));
      list = sortItems(browseFilter(list, q, "movie"), q.sort);
      return (q.groupVersions ? oneCopyEach(list) : list).slice(q.offset, q.offset + q.limit);
    },
    movie: async (id) => films.find((m) => m.id === id) ?? null,
    seriesList: async (q) => {
      await sleep();
      const out = excluded();
      let list = ofGenre(shows.filter((s) => !out.has(s.categoryId ?? "") && !out.has(s.id)), q.contentGenre);
      if (q.categoryId) list = list.filter((s) => s.categoryId === q.categoryId);
      if (q.genre) list = list.filter((s) => s.genre?.includes(q.genre!));
      return sortItems(browseFilter(list, q, "series"), q.sort).slice(q.offset, q.offset + q.limit);
    },
    series: async (id) => shows.find((s) => s.id === id) ?? null,
    episodes: async (seriesId) => episodesBySeries.get(seriesId) ?? [],
    details: async (itemId) => detailsFor(itemId),
    relatedMovies: async () => [],
    relatedSeries: async () => [],
    titleMarks: async (kind, ids) => titleMarks(kind === "series" ? "series" : "movie", ids),
    movieVersions: async (movieId) => {
      const film = films.find((f) => f.id === movieId);
      if (!film) return [];
      const out = excluded();
      const category = (id: string | null) => categoryList.find((c) => c.id === id);
      return films
        .filter((f) => versionKey(f) === versionKey(film) && (f.id === movieId || !out.has(f.id)))
        .sort((a, b) => Number(b.id === movieId) - Number(a.id === movieId))
        .map((f) => ({ movie: f, sourceName: "Demo Library", categoryName: category(f.categoryId) ? named(category(f.categoryId)!).name : null, quality: null, extension: "mp4" }));
    },
    // The demo library is never sent to TMDB, so it never gains posters.
    fillPosters: async () => [],
    // Nor to Wikidata: without TMDB a person is a search.
    person: async () => null,
    genres: async (kind) => {
      const items = kind === "movie" ? films : shows;
      return [...new Set(items.flatMap((i) => i.genre?.split(", ") ?? []))].sort();
    },
    topTitles: async (kind, limit) => {
      const out = excluded();
      return kind === "movie"
        ? { movies: films.filter((f) => (f.year ?? 0) >= 2025 && !out.has(f.categoryId ?? "") && !out.has(f.id)).sort((a, b) => (b.rating ?? 0) - (a.rating ?? 0)).slice(0, limit), series: [] }
        : { movies: [], series: shows.filter((s) => !out.has(s.categoryId ?? "") && !out.has(s.id)).sort((a, b) => (b.rating ?? 0) - (a.rating ?? 0)).slice(0, limit) };
    },
    search: async (text) => {
      await sleep(30);
      const q = text.trim().toLowerCase();
      if (!q) return { channels: [], movies: [], series: [], programs: [], people: [], reasons: {} };
      const out = excluded();
      const programs = [];
      for (const [channelId, list] of guideByChannel) {
        const channel = channelList.find((c) => c.id === channelId)!;
        if (channelOut(channel, out)) continue;
        for (const p of list) if (p.title.toLowerCase().includes(q) && p.end > now() && programs.length < 300) programs.push({ program: p, channel: named(channel) });
      }
      const shown = <T extends Movie | Series>(item: T) => !out.has(item.categoryId ?? "") && !out.has(item.id);
      const named_ = <T extends Movie | Series>(item: T) => item.title.toLowerCase().includes(q);
      const movies = films.filter((m) => shown(m) && named_(m));
      const series = shows.filter((s) => shown(s) && named_(s));
      const movieTerms = termMatches(text, films.filter((m) => shown(m) && !named_(m)));
      const seriesTerms = termMatches(text, shows.filter((s) => shown(s) && !named_(s)));
      const people = new Map<string, SearchPerson>();
      for (const [name, fromId] of [...movieTerms.people, ...seriesTerms.people]) {
        const known = people.get(name);
        if (known) known.titles += 1;
        else people.set(name, { name, photoUrl: null, tmdbId: null, fromId, titles: 1 });
      }
      return {
        channels: channelList.map(named).filter((c) => c.name.toLowerCase().includes(q) && !channelOut(c, out)),
        movies: [...movies, ...movieTerms.found.map(([m]) => m)].slice(0, 40),
        series: [...series, ...seriesTerms.found.map(([s]) => s)].slice(0, 40),
        programs,
        people: [...people.values()].sort((a, b) => b.titles - a.titles).slice(0, 12),
        reasons: Object.fromEntries([...movieTerms.found, ...seriesTerms.found].map(([item, why]) => [item.id, why])),
      };
    },

    guide: async (channelIds, from, to) =>
      channelIds.map((id): GuideRow => ({
        channel: named(channelList.find((c) => c.id === id)!),
        programs: programsFor(id).filter((p) => p.end > from && p.start < to),
      })),
    nowNext: async (channelIds) =>
      channelIds.map((id) => {
        const list = programsFor(id);
        const i = list.findIndex((p) => p.start <= now() && now() < p.end);
        return { channel: named(channelList.find((c) => c.id === id)!), programs: i >= 0 ? list.slice(i, i + 2) : [] };
      }),
    program: async (programId) => {
      for (const [channelId, list] of guideByChannel) {
        const program = list.find((p) => p.id === programId);
        if (program) return { program, channel: named(channelList.find((c) => c.id === channelId)!) };
      }
      return null;
    },

    upcomingPrograms: async (from, to, limit) => {
      const hits = [];
      const out = excluded();
      for (const [channelId, list] of guideByChannel) {
        const channel = named(channelList.find((c) => c.id === channelId)!);
        if (channelOut(channel, out)) continue;
        const picks = list.filter((p) => p.start >= from && p.start < to && p.imageUrl && (p.flags.isLive || p.category === "Movie" || p.category === "Sports")).slice(0, 2);
        hits.push(...picks.map((program) => ({ program, channel })));
      }
      return hits.sort((a, b) => a.program.start - b.program.start).slice(0, limit);
    },
    searchGuide: async (text, from) => {
      await sleep(30);
      const q = text.trim().toLowerCase();
      if (q.length < 2) return [];
      const t = now();
      const hits: ProgramHit[] = [];
      for (const [channelId, list] of guideByChannel)
        for (const p of list) if (p.end > from && p.title.toLowerCase().includes(q)) hits.push({ program: p, channel: channelList.find((c) => c.id === channelId)! });
      // Still to come first, soonest first; then what aired, latest first.
      const rank = (p: Program) => (p.end <= t ? [1, -p.start] : [0, p.start]);
      return hits.sort((a, b) => rank(a.program)[0] - rank(b.program)[0] || rank(a.program)[1] - rank(b.program)[1]).slice(0, 30);
    },
    guideSpan: async () => {
      const all = [...guideByChannel.values()].flat();
      return all.length ? { from: Math.min(...all.map((p) => p.end)), to: Math.max(...all.map((p) => p.end)) } : null;
    },

    guideSources: async () => [...guideSources.values()],
    addGuideSource: async (guide) => {
      let host: string | null = null;
      try {
        host = new URL(guide.url).host;
      } catch {
        throw new Error("Xorvio needs a valid guide address to continue.");
      }
      const added = { id: `guide-${crypto.randomUUID()}`, name: guide.name.trim() || host, displayHost: host, offsetMinutes: guide.offsetMinutes, createdAt: now(), sortIndex: guideSources.size };
      guideSources.set(added.id, { guide: added, status: emptyStatus(), refreshing: false });
      return added;
    },
    updateGuideSource: async (guide) => {
      const existing = guideSources.get(guide.id);
      if (existing) guideSources.set(guide.id, { ...existing, guide: { ...existing.guide, name: guide.name.trim() || existing.guide.name, offsetMinutes: guide.offsetMinutes } });
    },
    removeGuideSource: async (id) => {
      guideSources.delete(id);
      for (const [channelId, choice] of guideOverrides) if (choice.guideId === id) guideOverrides.delete(channelId);
    },
    guideSourceLink: async () => null,
    refreshGuideSource: async (id) => {
      await sleep(400);
      const summary = guideSources.get(id);
      if (!summary) throw new Error("This guide no longer exists.");
      const status = { ...summary.status, lastAttempt: now(), guideLastError: "Added guides load in the desktop app." };
      guideSources.set(id, { ...summary, status });
      return status;
    },
    guideChannels: async (text) => {
      await sleep(30);
      const q = text.trim().toLowerCase();
      return channelList.filter((c) => !q || c.name.toLowerCase().includes(q) || c.epgId!.toLowerCase().includes(q)).slice(0, 60).map(guideChannel);
    },
    guideMatch: async (channelId) => {
      const choice = guideOverrides.get(channelId);
      const shown = choice ? channelList.find((c) => c.epgId === choice.epgId) : channelList.find((c) => c.id === channelId);
      return { channelId, current: shown ? guideChannel(shown) : null, chosen: !!choice };
    },
    matchGuide: async (channelId, guideId, epgId) => {
      guideOverrides.set(channelId, { guideId, epgId });
    },
    resetGuideMatch: async (channelId) => {
      guideOverrides.delete(channelId);
    },
    // As Home and the Watchlist show it: less what's hidden, locked or left out by the profile.
    watchlist: async () => {
      const out = excluded();
      const kept = (e: WatchlistEntry) => {
        if (e.kind === "program") return (e.end ?? 0) > now() && !channelOut(channelList.find((c) => c.id === e.channelId) ?? ({ id: "" } as Channel), out);
        const title = e.kind === "movie" ? films.find((f) => f.id === e.id) : shows.find((s) => s.id === e.id);
        return !title || !out.has(title.categoryId ?? "");
      };
      return [...watchlist.values()].filter(kept);
    },
    setWatchlisted: async (entry, saved) => {
      if (saved) watchlist.set(entry.id, entry);
      else watchlist.delete(entry.id);
      changed();
    },
    favoriteChannelIds: async () => [...favorites],
    setFavorite: async (id, favorite) => {
      if (favorite) favorites.add(id);
      else favorites.delete(id);
      changed();
    },

    favoriteLists: async () => groups.map((g) => ({ ...g, channelIds: [...g.channelIds] })),
    createFavoriteList: async (name, channelId) => {
      const group = { id: crypto.randomUUID(), name: groupName(name), channelIds: channelId ? [channelId] : [] };
      groups.push(group);
      return { ...group, channelIds: [...group.channelIds] };
    },
    renameFavoriteList: async (id, name) => {
      const group = groups.find((g) => g.id === id);
      if (!group) throw new Error("This group no longer exists.");
      group.name = groupName(name, id);
    },
    deleteFavoriteList: async (id) => {
      const i = groups.findIndex((g) => g.id === id);
      if (i >= 0) groups.splice(i, 1);
    },
    setInFavoriteList: async (listId, channelId, member) => {
      if (listId === FAVORITES_LIST) {
        if (member) favorites.add(channelId);
        else favorites.delete(channelId);
        return;
      }
      const list = listOf(listId);
      const i = list.indexOf(channelId);
      if (member && i < 0) list.push(channelId);
      if (!member && i >= 0) list.splice(i, 1);
    },
    reorderFavoriteList: async (listId, channelIds) => {
      // Named channels take the places they held between them; the rest stay put.
      const current = listOf(listId);
      const named = new Set(channelIds.filter((id) => current.includes(id)));
      const moved = [...named];
      const next = current.map((id) => (named.has(id) ? moved.shift()! : id));
      if (listId === FAVORITES_LIST) {
        favorites.clear();
        next.forEach((id) => favorites.add(id));
      } else listOf(listId).splice(0, current.length, ...next);
    },
    itemEdits: async () => itemEdits(),
    renameItem: async (itemId, name) => editItem(itemId, (e) => (e.displayName = name?.trim() || null)),
    setHidden: async (itemId, hidden) => editItem(itemId, (e) => (e.hidden = hidden), true),
    setCatchupOffset: async (channelId, minutes) => {
      const channel = channelList.find((c) => c.id === channelId);
      if (!channel) throw new Error("Only channels have a catch-up offset.");
      channel.catchupOffsetMinutes = Math.max(-30, Math.min(30, Math.round(minutes)));
    },
    setPreferredVersion: async (channelId, preferred) => {
      const channel = channelList.find((c) => c.id === channelId);
      if (!channel) throw new Error("This channel is no longer available.");
      if (preferred) preferredVersions.set(groupKey(channel), channelId);
      else preferredVersions.delete(groupKey(channel));
    },
    showAllHidden: async () => {
      for (const id of [...edits.keys()]) editItem(id, (e) => (e.hidden = false));
    },
    setLocked: async (itemId, locked) => {
      if (!pin) throw new Error("Set a parental PIN in Settings first.");
      if (!locked && !unlocked) throw new Error("Enter the parental PIN first.");
      const automatic = editKind(itemId) !== "channel" && looksAdult(categoryList.find((c) => c.id === itemId)?.name ?? "");
      editItem(itemId, (e) => (e.locked = locked === automatic ? null : locked));
    },
    parentalStatus: async () => {
      // Locks again by itself after the time chosen in Settings, as the app does.
      const minutes = settings.parentalLockMinutes;
      const locksAt = unlocked && unlockedAt !== null && minutes ? unlockedAt + minutes * 60 : null;
      if (locksAt !== null && locksAt <= now()) unlocked = false;
      return { hasPin: !!pin, unlocked: !!pin && unlocked, lockedCount: pin ? lockedIds().size : 0, locksAt: pin && unlocked ? locksAt : null };
    },
    setPin: async (currentPin, newPin) => {
      if (newPin !== null && !/^\d{4}$/.test(newPin)) throw new Error("A PIN is four digits.");
      checkPin(currentPin);
      pin = newPin;
      unlocked = false;
    },
    unlockParental: async (given) => {
      if (pin && given !== pin) return false;
      unlocked = true;
      unlockedAt = now();
      return true;
    },
    lockParental: async () => {
      unlocked = false;
    },
    saveProgress: async (p) => {
      progress.set(p.itemId, p);
    },
    progress: async (itemId) => progress.get(itemId) ?? null,
    setWatched: async (items, watched) => {
      for (const item of items) {
        if (watched) progress.set(item.itemId, { ...item, finished: true, updatedAt: now() });
        else progress.delete(item.itemId);
      }
    },
    removeFromContinueWatching: async (itemId) => {
      if (!progress.get(itemId)?.finished) progress.delete(itemId);
    },
    seriesProgress: async (ids) => ids.map((id) => progress.get(id)).filter((p): p is PlaybackProgress => !!p),
    continueWatching: async (limit) => continueItems().slice(0, limit),
    recentChannels: async (limit) => {
      const out = excluded();
      return recents.slice(0, limit).map((id) => channelList.find((c) => c.id === id)!).filter((c) => !channelOut(c, out)).map(named);
    },
    recordChannelWatched: async (id) => {
      const i = recents.indexOf(id);
      if (i >= 0) recents.splice(i, 1);
      recents.unshift(id);
      const channel = channelList.find((c) => c.id === id);
      if (channel) viewingHistory.unshift({ itemId: id, kind: "channel", sourceId: S, title: channel.name, subtitle: null, watchedAt: now() });
    },
    recordWatched: async (itemId, kind, sourceId, title, subtitle) => {
      viewingHistory.unshift({ itemId, kind, sourceId, title, subtitle, watchedAt: now() });
    },
    history: async () => viewingHistory.slice(0, 500),
    clearHistory: async () => {
      viewingHistory.length = 0;
      recents.length = 0;
    },

    play: async (request) => {
      await sleep(150);
      if (request.kind === "channel" || request.kind === "replay")
        return { ...plainStream, url: sampleLive, isLive: request.kind === "channel" };
      return { ...plainStream, url: sampleFilm };
    },
    seekStream: async (url, offset) => ({ ...plainStream, url, startOffsetSeconds: offset }),
    releaseStream: async () => {},
    // The demo's channels are files, so its "buffer" is the same file, seekable.
    startTimeshift: async (url) => ({ ...plainStream, url, isLive: true, timeshift: { windowSeconds: 1800 } }),
    thumbnails: async (url): Promise<Thumbnails> => {
      if (url !== sampleFilm) return { state: "unavailable", reason: "Live channels have no thumbnails." };
      if (demoSheet) return { state: "ready", sheet: demoSheet };
      if (!makingDemoSheet) {
        makingDemoSheet = true;
        makeDemoSheet()
          .then((sheet) => (demoSheet = sheet))
          .catch(() => (makingDemoSheet = false));
      }
      return { state: "making" };
    },
    restream: async (url, offset, options) => ({
      ...plainStream,
      url,
      startOffsetSeconds: offset,
      audioDelaySeconds: options.audioDelaySeconds ?? 0,
      burnedSubtitle: options.burnSubtitle != null && options.burnSubtitle >= 0 ? options.burnSubtitle : null,
    }),
    // The demo's streams are files that never refuse anything.
    streamFailure: async () => null,
    trackChoice: async (seriesId) => trackChoices.get(seriesId) ?? null,
    setTrackChoice: async (seriesId, choice) => {
      if (choice) trackChoices.set(seriesId, choice);
      else trackChoices.delete(seriesId);
    },
    addonSubtitleText: async () => "WEBVTT\n\n00:00:01.000 --> 00:00:05.000\nA subtitle from the demo add-on.\n",
    episodeSegments: async () => ({ intro: { startSeconds: 2, endSeconds: 20 }, recap: null, credits: null }),
    setPlaybackSupport: async () => {},
    // Listed so the menus can be seen here; handing over needs the desktop app.
    externalPlayers: async () => [
      { id: "iina", name: "IINA" },
      { id: "vlc", name: "VLC" },
    ],
    openInPlayer: async () => {
      throw new Error("Opening in another player needs the desktop app.");
    },

    castDevices: async () => [livingRoomTv],
    castCheck: async (request) =>
      request.kind === "recording" && recordings.get(request.recordingId)?.status === "recording" ? "A recording still being made can go to a TV once it's finished." : null,
    castConnect: async (deviceId) => {
      await sleep(700);
      if (deviceId !== livingRoomTv.id) throw new Error("That TV isn't on the network any more.");
      castStatus = { deviceId, deviceName: livingRoomTv.name, state: "ready", positionSeconds: 0, durationSeconds: null, live: false, canSeek: false, volume: 0.4, muted: false, message: null };
      return castStatus;
    },
    castLoad: async (request, _details, positionSeconds) => {
      if (!castStatus) throw new Error("The TV isn't connected any more.");
      await sleep(900);
      const live = request.kind === "channel";
      const durationSeconds = live ? null : request.kind === "movie" ? 6120 : 2640;
      castUpdate({ state: "playing", live, canSeek: !live, durationSeconds, positionSeconds: live ? 0 : (positionSeconds ?? 0), message: null });
      castTick();
      return castStatus!;
    },
    castPlay: async () => castUpdate({ state: "playing" }),
    castPause: async () => castUpdate({ state: "paused" }),
    castSeek: async (positionSeconds) => {
      castUpdate({ state: "buffering", positionSeconds });
      await sleep(500);
      castUpdate({ state: castStatus?.state === "buffering" ? "playing" : castStatus?.state });
    },
    castVolume: async (volume) => castUpdate({ volume, muted: false }),
    castMute: async (muted) => castUpdate({ muted }),
    castStop: async () => {
      clearInterval(castClock);
      const at = castStatus && !castStatus.live && castStatus.state !== "finished" ? castStatus.positionSeconds : null;
      castStatus = null;
      return at;
    },

    recordings: async () => withConflicts([...recordings.values()]),
    adBreaks: async (id) => adBreaks.get(id) ?? { state: "never", breaks: [], foundBy: null, error: null },
    findAdBreaks: async (id) => {
      const r = recordings.get(id);
      if (!r || (r.status !== "completed" && r.status !== "partial")) throw new Error("This is still recording. Its ad breaks can be found once it's finished.");
      const finding: AdBreaks = { state: "finding", breaks: adBreaks.get(id)?.breaks ?? [], foundBy: null, error: null };
      adBreaks.set(id, finding);
      // As if found in the background a few seconds later.
      setTimeout(() => {
        const length = r.recordedSeconds ?? r.end - r.start;
        adBreaks.set(id, { state: "found", breaks: [{ startSeconds: Math.round(length * 0.4), endSeconds: Math.round(length * 0.4) + 125 }], foundBy: "Xorvio", error: null });
      }, 4000);
      return finding;
    },
    // As the app words it; the demo allows two streams, so a third Multiview tile shows the warning.
    connectionWarning: async (sourceId, watching) => {
      const limit = sourceId === S ? demoSource.status.maxConnections : null;
      const recording = [...recordings.values()].filter((r) => r.sourceId === sourceId && r.status === "recording").length;
      const inUse = watching + recording;
      if (!limit || inUse < limit) return null;
      const what = [watching && `${watching} playing`, recording && `${recording} recording`].filter(Boolean).join(" and ");
      return `Your provider allows ${limit} stream${limit === 1 ? "" : "s"} at a time, and ${inUse} ${inUse === 1 ? "is" : "are"} already in use: ${what}.`;
    },
    scheduleRecording: async (req) => {
      const r = makeRecording(req, now() >= req.start - req.paddingBeforeSeconds ? "recording" : "scheduled");
      emitRecording(r);
      recordingsChanged();
      return withConflicts([...recordings.values()]).find((c) => c.id === r.id)!;
    },
    recordNow: async (req) => {
      const r = makeRecording({ ...req, start: Math.min(req.start, now()) }, "recording");
      emitRecording(r);
      recordingsChanged();
      return r;
    },
    stopRecording: async (id) => {
      const r = recordings.get(id);
      if (r) emitRecording({ ...r, status: "completed", end: r.fromCatchup ? r.end : now() });
    },
    cancelRecording: async (id) => {
      const r = recordings.get(id);
      if (!r) return;
      if (r.ruleId) skippedAirings.set(r.ruleId, new Set([...(skippedAirings.get(r.ruleId) ?? []), `${versionName(r.channelName).key}|${r.start}`]));
      emitRecording({ ...r, status: "cancelled" });
      recordingsChanged();
    },
    deleteRecording: async (id) => {
      const r = recordings.get(id);
      recordings.delete(id);
      if (r) recordingListeners.forEach((l) => l({ ...r, status: "cancelled" }));
      recordingsChanged();
    },
    revealRecording: async () => {},
    recordFromCatchup: async (req) => {
      const channel = channelList.find((c) => c.id === req.channelId);
      if (!channel?.catchupDays) throw new Error("This channel doesn't keep programmes to watch again.");
      if (req.end > now()) throw new Error("This programme hasn't finished yet. Record it as it airs instead.");
      const oldest = now() - channel.catchupDays * day + 300;
      if (req.start < oldest) throw new Error("That's further back than the channel keeps programmes.");
      const r = makeRecording(
        { ...req, paddingBeforeSeconds: Math.max(0, Math.min(req.paddingBeforeSeconds, req.start - oldest)), paddingAfterSeconds: Math.max(0, Math.min(req.paddingAfterSeconds, now() - req.end)) },
        "recording",
        { fromCatchup: true },
      );
      emitRecording(r);
      recordingsChanged();
      return r;
    },
    seriesRules: async () => countedRules(),
    createSeriesRule: async (programId, channelId, options) => {
      await sleep(300);
      const channel = channelList.find((c) => c.id === channelId) ?? groupSimilar(channelList).find((c) => c.id === channelId);
      const program = channel && programsFor(channel.id).find((p) => p.id === programId);
      if (!channel || !program) throw new Error("That programme is no longer in the guide.");
      const rule = addSeries(program, channel, options);
      // The programme chosen records at once when it's on now.
      if (program.start <= now() && now() < program.end && ![...recordings.values()].some((r) => r.ruleId === rule.id && r.start === program.start)) {
        const r = makeRecording(
          { sourceId: S, channelId: channel.id, channelName: channel.name, channelLogoUrl: channel.logoUrl, title: program.title, subtitle: program.subtitle, summary: program.summary, artworkUrl: program.imageUrl, programId: program.id, start: program.start, end: program.end, paddingBeforeSeconds: settings.recordPaddingBefore * 60, paddingAfterSeconds: settings.recordPaddingAfter * 60 },
          "recording",
          { ruleId: rule.id },
        );
        emitRecording(r);
      }
      recordingsChanged();
      return countedRules().find((r) => r.id === rule.id)!;
    },
    updateSeriesRule: async (id, options) => {
      const rule = seriesRules.get(id);
      if (!rule) throw new Error("That series recording no longer exists.");
      seriesRules.set(id, { ...rule, ...options });
      for (const r of [...recordings.values()]) if (r.ruleId === id && r.status === "scheduled") recordings.delete(r.id);
      planSeries(seriesRules.get(id)!);
      recordingsChanged();
      return countedRules().find((r) => r.id === id)!;
    },
    deleteSeriesRule: async (id, deleteRecordings) => {
      for (const r of [...recordings.values()]) {
        if (r.ruleId !== id) continue;
        if (r.status === "scheduled" || r.status === "cancelled" || deleteRecordings) recordings.delete(r.id);
        else recordings.set(r.id, { ...r, ruleId: null });
      }
      seriesRules.delete(id);
      skippedAirings.delete(id);
      recordingsChanged();
    },

    addons: async () => [],
    installAddon: async () => {
      throw new Error("Add-ons need the desktop app.");
    },
    removeAddon: async () => {},
    setAddonEnabled: async () => {},
    discoverRows: async () => [],
    addonCatalog: async () => {
      throw new Error("Add-ons need the desktop app.");
    },
    addonMeta: async () => {
      throw new Error("Add-ons need the desktop app.");
    },
    addonStreams: async () => [],
    addonSubtitles: async () => [],
    folderSubtitles: async () => [],
    folderSubtitleText: async () => "WEBVTT\n\n00:00:01.000 --> 00:00:05.000\nA subtitle file beside the video.\n",
    addonSearch: async () => ({ movies: [], series: [] }),

    settings: async () => settings,
    saveSettings: async (s) => {
      const startsSyncing = s.syncEnabled && !settings.syncEnabled;
      settings = s;
      if (startsSyncing) mockSync();
    },
    storageUsage: async () => {
      const recorded = [...recordings.values()].filter((r) => r.filePath);
      return {
        catalogBytes: 21_400_000 + listingCount() * 410,
        guideListings: listingCount(),
        libraryBytes: 184_320,
        recordingsBytes: recorded.reduce((n, r) => n + (r.fileSizeBytes ?? 0), 0),
        recordingCount: recorded.length,
        temporaryBytes,
      };
    },
    clearGuideListings: async () => {
      await sleep(1200);
      guideByChannel.clear();
      demoSource.status = { ...demoSource.status, programCount: 0 };
      changed();
    },
    clearTemporaryFiles: async () => {
      temporaryBytes = 0;
    },
    showRecordingsFolder: async () => {},
    chooseFolder: async () => null,
    chooseFile: async () => null,
    chooseMediaFolders: async () => sampleFolders[folderPicks++ % sampleFolders.length],
    log: async (level, message) => console[level](message),
    diagnostics: async () => [...diagnosticEvents].reverse(),
    clearDiagnostics: async () => {
      diagnosticEvents.length = 0;
    },
    exportDiagnostics: async () => null,
    // The browser preview has one window.
    popOut: async () => {},
    takePopOut: async () => null,
    dock: async () => {},
    setPopOutOnTop: async () => {},
    onPopOutPlay: () => () => {},
    onDock: () => () => {},
    onPopOutClosed: () => () => {},
    openExternal: async (url) => {
      window.open(url, "_blank");
    },

    ...libraryBackend((id) => folderSources.find((s) => s.config.id === id)?.config),

    ...profileBackend({
      categories: categoryList,
      looksAdult,
      counts: () => {
        const counts = new Map<string, number>();
        for (const item of [...films, ...shows, ...channelList]) if (item.categoryId) counts.set(item.categoryId, (counts.get(item.categoryId) ?? 0) + 1);
        return counts;
      },
      locked: () => (pin && !unlocked ? lockedIds() : new Set()),
      named,
    }),
    newReleases: async (limit) => {
      const out = excluded();
      const year = new Date().getFullYear();
      return films
        .filter((m) => !out.has(m.categoryId ?? "") && (m.year ?? 0) >= year - 1 && (m.year ?? 0) <= year + 1)
        .sort((a, b) => (b.year ?? 0) - (a.year ?? 0) || (b.addedAt ?? 0) - (a.addedAt ?? 0))
        .slice(0, limit);
    },
    newEpisodeSeries: async (limit) => {
      const out = excluded();
      const unseen = [...unseenEpisodes].sort((a, b) => b[1].foundAt - a[1].foundAt).map(([id]) => id);
      const aired = [...episodesBySeries]
        .map(([id, eps]) => [id, Math.max(...eps.filter((e) => e.season > 0).map((e) => e.airDate ?? 0))] as const)
        .filter(([, latest]) => latest > now() - 30 * day && latest <= now() + day)
        .sort((a, b) => b[1] - a[1])
        .map(([id]) => id);
      return [...new Set([...unseen, ...aired])]
        .map((id) => shows.find((s) => s.id === id))
        .filter((s): s is Series => !!s && !out.has(s.categoryId ?? ""))
        .slice(0, limit);
    },
    newEpisodeCounts: async () => Object.fromEntries([...unseenEpisodes].map(([id, { count }]) => [id, count])),
    markEpisodesSeen: async (seriesId) => unseenEpisodes.delete(seriesId),
    // The browser's own notifications stand in for the system's.
    notificationPermission: async () => {
      if (typeof Notification === "undefined") return "unknown";
      return Notification.permission === "granted" ? "allowed" : Notification.permission === "denied" ? "denied" : "notDetermined";
    },
    sendTestReminder: async () => {
      if (typeof Notification === "undefined") throw new Error("Reminders need the desktop app.");
      if (Notification.permission === "default") await Notification.requestPermission();
      if (Notification.permission !== "granted") throw new Error("Notifications are off for Xorvio. Turn them on in System Settings › Notifications.");
      setTimeout(() => new Notification("Your game is about to start", { body: "This is how Xorvio reminds you about programmes on your watchlist." }), 3000);
    },
    openNotificationSettings: async () => {
      throw new Error("Notification settings open from the desktop app.");
    },
    onNow: async (limit) => {
      const out = excluded();
      const t = now();
      const order = [...favorites, ...recents, ...channelList.map((c) => c.id)];
      const hits: ProgramHit[] = [];
      for (const id of new Set(order)) {
        const channel = channelList.find((c) => c.id === id);
        if (!channel || channelOut(channel, out) || hits.length >= limit) continue;
        const program = programsFor(id).find((p) => p.start <= t && t < p.end);
        if (program) hits.push({ program, channel: named(channel) });
      }
      return hits;
    },
    backupStatus: async () => ({
      computerName: "This Mac",
      automaticFolder: "/Users/you/Library/Application Support/com.xorvio.desktop/Backups",
      lastAutomatic: settings.autoBackup === "off" ? null : now() - 5 * 3600,
      automaticError: null,
      restoreNotice: null,
      minPassphraseLength: 8,
    }),
    backUpNow: async (passphrase) => {
      await sleep(passphrase ? 1400 : 700);
      return `/Users/you/Documents/Xorvio Backup ${new Date().toISOString().slice(0, 10)}.xorvio-backup`;
    },
    chooseBackup: async () => ({
      path: "/Users/you/Documents/Xorvio Backup 2026-10-28.xorvio-backup",
      fileName: "Xorvio Backup 2026-10-28.xorvio-backup",
      appVersion: "0.1.0",
      createdAt: now() - 3 * day,
      computer: "Studio Mac",
      counts: { sources: 2, addons: 1, watchlist: 14, progress: 86, history: 412, favorites: 9, lists: 2, itemEdits: 31, recordings: 6, scheduledRecordings: 2 },
      includesSecrets: true,
      automatic: false,
      recordingNow: [...recordings.values()].filter((r) => r.status === "recording").length,
      scheduledHere: [...recordings.values()].filter((r) => r.status === "scheduled").length,
    }),
    restoreBackup: async (_path, passphrase) => {
      await sleep(passphrase ? 1200 : 500);
      if (passphrase && passphrase !== "demo passphrase") {
        throw new Error("That passphrase doesn't open this backup's logins. Check it and try again, or leave it empty to restore without them.");
      }
      // The desktop app restarts here; the preview carries on as it was.
    },
    showBackupsFolder: async () => {},
    syncStatus: async () => mockSyncStatus(),
    syncNow: () => mockSync(),

    onRecordingChanged: (handler) => {
      recordingListeners.add(handler);
      return () => recordingListeners.delete(handler);
    },
    onRecordingsChanged: (handler) => {
      recordingsListeners.add(handler);
      return () => recordingsListeners.delete(handler);
    },
    onRefreshProgress: (handler) => {
      progressListeners.add(handler);
      return () => progressListeners.delete(handler);
    },
    onLibraryChanged: (handler) => {
      libraryListeners.add(handler);
      return () => libraryListeners.delete(handler);
    },
    onNavigate: () => () => {},
    onPlayChannel: () => () => {},
    onCastDevices: () => () => {},
    onCastChanged: (handler) => {
      castListeners.add(handler);
      return () => castListeners.delete(handler);
    },
    onSyncChanged: (handler) => {
      syncListeners.add(handler);
      return () => syncListeners.delete(handler);
    },
    onBackupChanged: () => () => {},
    onOpenSeries: () => () => {},
    onPlayEpisode: () => () => {},
  };
}
