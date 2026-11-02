// Home's rows: which ones show, in what order, as Xorvio for Mac's
// HomeLayout. Each profile has its own, and Home keeps one arrangement for
// when the sources lead it and one for when add-ons do, so switching between
// them keeps each. The backend only stores layouts (library.sqlite); what the
// rows are and the standard arrangement live here, so the browser preview
// shares them. Every media folder and media server has a row of its own,
// there from when it's added and gone when it's removed.

import { useEffect } from "react";
import { create } from "zustand";
import { api } from "./api";
import { active, GENRE_NAMES, useProfiles } from "./profiles";
import { useSources } from "./sources";
import type { AddonCatalogRow, CategoryKind, ContentGenre, LibraryProfile, SourceKind, SourceSummary, TraktChart, TraktListInfo, TraktMedia, TraktRowRef, TraktRowSource } from "./types";

export type HomeMode = "sources" | "addons";

export type HomeRowKind =
  | "continueWatching"
  | "watchlist"
  | "recordings"
  | "tonight"
  | "onNow"
  | "favoriteChannels"
  | "recentChannels"
  | "top10Movies"
  | "top10Series"
  /** Films from this year and last, this year's first. */
  | "newReleases"
  | "recentMovies"
  | "topRatedMovies"
  /** Series with episodes new to the person, then ones just aired. */
  | "newEpisodes"
  | "newSeries"
  | "topRatedSeries"
  /** Every add-on catalogue not given a row of its own, in Personal's order. */
  | "addonRows"
  /** One add-on catalogue. */
  | "addonCatalog"
  /** A category from the sources, pinned as a row. */
  | "category"
  | "genreMovies"
  | "genreSeries"
  /** A Trakt chart (shown as a Top 10), list or recommendations. */
  | "trakt"
  /** A media folder's or media server's films and series, newest first. */
  | "library";

export interface HomeRow {
  id: string;
  kind: HomeRowKind;
  visible: boolean;
  categoryId?: string;
  /** Kept so the row still has a name if the provider renames or drops the category. */
  categoryName?: string;
  categoryKind?: CategoryKind;
  genre?: ContentGenre;
  catalog?: { addonId: string; mediaType: string; catalogId: string; title: string };
  trakt?: TraktRowRef;
  /** A library row's source, with its name and kind kept for when it's gone. */
  sourceId?: string;
  sourceName?: string;
  sourceKind?: SourceKind;
}

/** A media folder or media server: the sources whose rows Home has by itself. */
export interface Library {
  id: string;
  name: string;
  kind: SourceKind;
}

const LIBRARY_KINDS = new Set<SourceKind>(["folder", "plex", "jellyfin", "emby"]);

/** The libraries among the sources, in the person's order. */
export function librariesOf(sources: SourceSummary[]): Library[] {
  return sources.filter((s) => LIBRARY_KINDS.has(s.config.kind)).map((s) => ({ id: s.config.id, name: s.config.name, kind: s.config.kind }));
}

export function libraryRow(library: Library, visible = true): HomeRow {
  return { id: `library-${library.id}`, kind: "library", visible, sourceId: library.id, sourceName: library.name, sourceKind: library.kind };
}

/**
 * Libraries' rows go after Continue Watching and the watchlist when a
 * layout starts with them, else first: a library just added shows near the
 * top. Rows for libraries already there take their current names, and a
 * removed library's row goes.
 */
function withLibraries(rows: HomeRow[], libraries: Library[]): HomeRow[] {
  const byId = new Map(libraries.map((library) => [library.id, library]));
  const kept = rows
    .filter((row) => row.kind !== "library" || (!!row.sourceId && byId.has(row.sourceId)))
    .map((row) => {
      const library = row.kind === "library" && row.sourceId ? byId.get(row.sourceId) : undefined;
      return library ? { ...row, sourceName: library.name, sourceKind: library.kind } : row;
    });
  const present = new Set(kept.map((row) => row.id));
  const added = libraries.map((library) => libraryRow(library)).filter((row) => !present.has(row.id));
  if (!added.length) return kept;
  let at = 0;
  while (at < kept.length && (kept[at].kind === "continueWatching" || kept[at].kind === "watchlist")) at++;
  return [...kept.slice(0, at), ...added, ...kept.slice(at)];
}

export interface HomeLayout {
  version: number;
  /** The featured titles at the top. */
  showHero: boolean;
  rows: HomeRow[];
}

/** 2: Trakt's Top 10 films and series join Home. */
const VERSION = 2;

/** The libraries Home knows of; null until the sources are first read. */
let knownLibraries: Library[] | null = null;

/** Rows every layout has, shown or switched off. */
const BUILT_INS: HomeRowKind[] = [
  "continueWatching",
  "watchlist",
  "recordings",
  "tonight",
  "onNow",
  "favoriteChannels",
  "recentChannels",
  "top10Movies",
  "top10Series",
  "newReleases",
  "recentMovies",
  "topRatedMovies",
  "newEpisodes",
  "newSeries",
  "topRatedSeries",
  "addonRows",
];

const KINDS = new Set<string>([...BUILT_INS, "addonCatalog", "category", "genreMovies", "genreSeries", "trakt", "library"]);

const builtIn = (kind: HomeRowKind, visible = true): HomeRow => ({ id: kind, kind, visible });

export function genreRow(genre: ContentGenre, series = false): HomeRow {
  return { id: `${series ? "genre-series" : "genre-movies"}-${genre}`, kind: series ? "genreSeries" : "genreMovies", visible: true, genre };
}

export function categoryRow(category: { id: string; name: string; kind: CategoryKind }): HomeRow {
  return { id: `category-${category.id}`, kind: "category", visible: true, categoryId: category.id, categoryName: category.name, categoryKind: category.kind };
}

export const catalogKey = (row: { addonId: string; mediaType: string; catalogId: string }) => `${row.addonId}/${row.mediaType}/${row.catalogId}`;

export function catalogRow(row: AddonCatalogRow): HomeRow {
  return {
    id: `addon-${catalogKey(row)}`,
    kind: "addonCatalog",
    visible: true,
    catalog: { addonId: row.addonId, mediaType: row.mediaType, catalogId: row.catalogId, title: row.title },
  };
}

const MEDIA_NAMES: Record<TraktMedia, string> = { movies: "Movies", shows: "Series" };

/** Trakt's charts as Home names them. Box office is films only. */
export const TRAKT_CHARTS: { chart: TraktChart; label: string; note: string }[] = [
  { chart: "trending", label: "Top 10", note: "What Trakt's members are watching now" },
  { chart: "popular", label: "Popular", note: "Trakt's most popular of all time" },
  { chart: "watched", label: "Most Watched This Week", note: "Played most on Trakt in the last week" },
  { chart: "boxOffice", label: "Box Office Top 10", note: "This weekend's US box office, from Trakt" },
];

function chartTitle(chart: TraktChart, media: TraktMedia) {
  const kind = MEDIA_NAMES[media];
  switch (chart) {
    case "trending":
      return `Top 10 ${kind} on Trakt`;
    case "popular":
      return `Popular ${kind} on Trakt`;
    case "watched":
      return `Most Watched ${kind} This Week`;
    case "boxOffice":
      return "Box Office Top 10";
  }
}

export function traktChartRow(chart: TraktChart, media: TraktMedia, visible = true): HomeRow {
  const source: TraktRowSource = { type: "chart", chart, media };
  return { id: `trakt-${chart}-${media}`, kind: "trakt", visible, trakt: { source, title: chartTitle(chart, media) } };
}

export function traktRecommendedRow(media: TraktMedia): HomeRow {
  const source: TraktRowSource = { type: "recommended", media };
  return { id: `trakt-recommended-${media}`, kind: "trakt", visible: true, trakt: { source, title: `Recommended ${MEDIA_NAMES[media]} for You` } };
}

export function traktListRow(list: TraktListInfo): HomeRow {
  const source: TraktRowSource = { type: "list", user: list.user, list: list.list };
  return { id: `trakt-list-${list.user}/${list.list}`, kind: "trakt", visible: true, trakt: { source, title: list.name, owner: list.owner } };
}

/** Trakt's Top 10 films and series, which every layout starts with. */
const traktTop10 = (visible = true) => [traktChartRow("trending", "movies", visible), traktChartRow("trending", "shows", visible)];

/**
 * Home as it was before it could be changed, plus a row for each of the
 * profile's favourite genres, and New Releases and New Episodes as Xorvio for
 * Mac has them, and Trakt's Top 10s. Rows Home didn't have are there, switched off. When the
 * sources carry channels but no films or series (`liveOnly`), the live rows
 * lead instead, as they do on the Mac.
 */
export function standardLayout(mode: HomeMode, profile?: LibraryProfile, liveOnly = false, libraries: Library[] = knownLibraries ?? []): HomeLayout {
  const genres = (profile?.genres ?? []).slice(0, 4).map((genre) => genreRow(genre));
  const rows =
    mode === "sources" && liveOnly
      ? [
          builtIn("continueWatching"),
          builtIn("watchlist"),
          builtIn("recordings"),
          builtIn("favoriteChannels"),
          builtIn("onNow"),
          builtIn("tonight"),
          builtIn("recentChannels"),
          ...genres.map((row) => ({ ...row, visible: false })),
          ...traktTop10(false),
        ]
      : mode === "addons"
      ? [
          builtIn("continueWatching"),
          ...traktTop10(),
          builtIn("addonRows"),
          builtIn("watchlist"),
          builtIn("recordings"),
          builtIn("tonight"),
          ...genres.map((row) => ({ ...row, visible: false })),
        ]
      : [
          builtIn("continueWatching"),
          builtIn("watchlist"),
          builtIn("recordings"),
          builtIn("tonight"),
          ...traktTop10(),
          builtIn("top10Movies"),
          builtIn("newReleases"),
          builtIn("addonRows"),
          ...genres,
          builtIn("newEpisodes"),
          builtIn("recentMovies"),
          builtIn("topRatedSeries"),
          builtIn("newSeries"),
        ];
  const present = new Set(rows.map((row) => row.id));
  const all = [...rows, ...BUILT_INS.filter((kind) => !present.has(kind)).map((kind) => builtIn(kind, false))];
  return { version: VERSION, showHero: true, rows: withLibraries(all, libraries) };
}

/**
 * A saved layout made whole: rows this version doesn't know are dropped,
 * each row is there once, and built-in rows it lacks come last, switched
 * off. A layout saved before Trakt's Top 10s gains them where the standard
 * layout has them.
 */
export function normalized(saved: unknown, mode: HomeMode, profile?: LibraryProfile, liveOnly = false, libraries: Library[] | null = knownLibraries): HomeLayout {
  const layout = saved as Partial<HomeLayout> | null;
  if (!layout || typeof layout !== "object" || !Array.isArray(layout.rows)) return standardLayout(mode, profile, liveOnly, libraries ?? []);
  const seen = new Set<string>();
  let rows = layout.rows.filter(
    (row): row is HomeRow =>
      !!row &&
      typeof row.id === "string" &&
      KINDS.has(row.kind) &&
      (row.kind !== "trakt" || !!row.trakt?.source) &&
      (row.kind !== "library" || !!row.sourceId) &&
      !seen.has(row.id) &&
      !!seen.add(row.id),
  );
  if ((layout.version ?? 1) < 2) {
    const top10 = traktTop10(!(mode === "sources" && liveOnly)).filter((row) => !seen.has(row.id));
    const after = rows.findIndex((row) => row.id === (mode === "addons" ? "continueWatching" : "tonight"));
    rows.splice(after + 1, 0, ...top10);
    top10.forEach((row) => seen.add(row.id));
  }
  for (const kind of BUILT_INS) if (!seen.has(kind)) rows.push(builtIn(kind, false));
  // Until the sources are read, library rows stay as they were.
  if (libraries) rows = withLibraries(rows, libraries);
  return { version: VERSION, showHero: layout.showHero !== false, rows: rows.map((row) => ({ ...row, visible: row.visible !== false })) };
}

const TITLES: Partial<Record<HomeRowKind, string>> = {
  continueWatching: "Continue Watching",
  watchlist: "Watchlist",
  recordings: "Recordings",
  tonight: "Tonight on TV",
  onNow: "On Now",
  favoriteChannels: "Favorite Channels",
  recentChannels: "Recently Watched Channels",
  top10Movies: "Top 10 New Releases",
  top10Series: "Top 10 Series",
  newReleases: "New Releases",
  recentMovies: "Recently Added Movies",
  topRatedMovies: "Top Rated Movies",
  newEpisodes: "New Episodes",
  newSeries: "New Series",
  topRatedSeries: "Top Rated Series",
  addonRows: "Add-on rows",
};

/** A row's title, as Home shows it. */
export function rowTitle(row: HomeRow) {
  switch (row.kind) {
    case "genreMovies":
      return `${row.genre ? GENRE_NAMES[row.genre] : "Genre"} Movies`;
    case "genreSeries":
      return `${row.genre ? GENRE_NAMES[row.genre] : "Genre"} Series`;
    case "category":
      return row.categoryName ?? "Category";
    case "addonCatalog":
      return row.catalog?.title ?? "Add-on row";
    case "trakt":
      return row.trakt?.title ?? "Trakt";
    case "library":
      return row.sourceName ?? "Library";
    default:
      return TITLES[row.kind] ?? row.kind;
  }
}

const NOTES: Partial<Record<HomeRowKind, string>> = {
  tonight: "Films, sport and events coming up today",
  onNow: "What's airing now, your channels first",
  favoriteChannels: "With what's on each now",
  top10Movies: "This year's films your sources carry the most copies of",
  top10Series: "This year's series your sources carry the most copies of",
  newReleases: "Newest films by release year",
  newEpisodes: "Series with the newest episodes",
  addonRows: "Every catalogue from your add-ons that hasn't a row of its own",
  genreMovies: "Films of this genre, newest first",
  genreSeries: "Series of this genre, newest first",
  addonCatalog: "From an add-on",
};

/** One line under a row's title in Customize Home, saying where it comes from. */
export function rowNote(row: HomeRow) {
  if (row.kind === "trakt") return row.trakt ? traktNote(row.trakt) : "From Trakt";
  if (row.kind === "library") return row.sourceKind === "folder" ? "Your media folder, newest first" : "Your media server, newest first";
  if (row.kind === "category") return row.categoryKind === "live" ? "Channels from your sources" : row.categoryKind === "series" ? "Series from your sources" : "Films from your sources";
  return NOTES[row.kind] ?? null;
}

/** A Trakt row's title on its own page, which shows more than a Top 10. */
export function traktPageTitle(trakt: TraktRowRef) {
  const source = trakt.source;
  return source.type === "chart" && source.chart === "trending" ? `Trending ${MEDIA_NAMES[source.media]} on Trakt` : trakt.title;
}

/** Where a Trakt row's titles come from, in a line. */
export function traktNote(trakt: TraktRowRef) {
  const source = trakt.source;
  if (source.type === "chart") return TRAKT_CHARTS.find((c) => c.chart === source.chart)?.note ?? "From Trakt";
  if (source.type === "recommended") return "From Trakt, by what you've watched and rated";
  return trakt.owner ? `A Trakt list by ${trakt.owner}` : "A Trakt list";
}

/** Rows added with a choice (a category, a genre, an add-on row, a Trakt chart or list) are removed rather than switched off. */
export function isRemovable(row: HomeRow) {
  return row.kind === "category" || row.kind === "genreMovies" || row.kind === "genreSeries" || row.kind === "addonCatalog" || row.kind === "trakt";
}

/** Moves a row past hidden ones by `by` (−1 up, 1 down), so the change shows on Home. */
export function moved(layout: HomeLayout, id: string, by: -1 | 1, skipHidden: boolean): HomeLayout {
  const rows = [...layout.rows];
  const from = rows.findIndex((row) => row.id === id);
  if (from < 0) return layout;
  let to = from + by;
  while (skipHidden && to >= 0 && to < rows.length && !rows[to].visible) to += by;
  if (to < 0 || to >= rows.length) return layout;
  const [row] = rows.splice(from, 1);
  rows.splice(to, 0, row);
  return { ...layout, rows };
}

interface HomeLayoutState {
  /** By "<profile id>/<mode>". */
  layouts: Record<string, HomeLayout>;
  /** Layouts nothing was saved for, by the same key: they follow the standard one as the library changes. */
  standard: Record<string, boolean>;
  /** The sources have channels but no films or series, so the standard layout leads with live rows. Home finds out. */
  liveOnly: boolean;
  setLiveOnly(liveOnly: boolean): void;
  load(mode: HomeMode): Promise<void>;
  /** Saves a layout for the profile in use; one the same as the standard layout is saved as none, so it follows later standards. */
  save(mode: HomeMode, layout: HomeLayout): Promise<void>;
}

function key(mode: HomeMode) {
  return `${useProfiles.getState().activeId}/${mode}`;
}

export const useHomeLayouts = create<HomeLayoutState>((set, get) => ({
  layouts: {},
  standard: {},
  liveOnly: false,
  setLiveOnly: (liveOnly) => {
    if (get().liveOnly === liveOnly) return;
    // Standard layouts change with it: the active profile's at once, the others when next loaded.
    const { activeId } = useProfiles.getState();
    const profile = active(useProfiles.getState());
    set((s) => {
      const layouts = { ...s.layouts };
      for (const [at, standard] of Object.entries(s.standard)) {
        if (!standard) continue;
        const [profileId, mode] = at.split("/") as [string, HomeMode];
        if (profileId === activeId) layouts[at] = standardLayout(mode, profile, liveOnly);
        else delete layouts[at];
      }
      return { liveOnly, layouts };
    });
  },
  load: async (mode) => {
    const at = key(mode);
    const saved = await api.homeLayout(mode).catch(() => null);
    set((s) => ({
      layouts: { ...s.layouts, [at]: normalized(saved, mode, active(useProfiles.getState()), s.liveOnly) },
      standard: { ...s.standard, [at]: saved == null },
    }));
  },
  save: async (mode, layout) => {
    const at = key(mode);
    const standard = standardLayout(mode, active(useProfiles.getState()), get().liveOnly);
    const isStandard = JSON.stringify(layout) === JSON.stringify(standard);
    set((s) => ({ layouts: { ...s.layouts, [at]: layout }, standard: { ...s.standard, [at]: isStandard } }));
    await api.saveHomeLayout(mode, isStandard ? null : layout);
  },
}));

/** The profile's layout for this mode, once it's known. */
export function useHomeLayout(mode: HomeMode | null): HomeLayout | null {
  const activeId = useProfiles((s) => s.activeId);
  const layout = useHomeLayouts((s) => (mode && activeId ? s.layouts[`${activeId}/${mode}`] : undefined));
  useEffect(() => {
    if (mode && activeId && !layout) useHomeLayouts.getState().load(mode);
  }, [mode, activeId, layout]);
  return layout ?? null;
}

/**
 * Follows the sources: a library added, renamed or removed changes the
 * layouts Home has read at once. Standard layouts are made again (another
 * profile's when it's next read); saved ones gain or lose the row.
 */
function followLibraries(sources: SourceSummary[] | null) {
  if (!sources) return;
  const libraries = librariesOf(sources);
  if (knownLibraries && JSON.stringify(knownLibraries) === JSON.stringify(libraries)) return;
  knownLibraries = libraries;
  const { activeId } = useProfiles.getState();
  const profile = active(useProfiles.getState());
  useHomeLayouts.setState((s) => {
    const layouts: Record<string, HomeLayout> = {};
    for (const [at, layout] of Object.entries(s.layouts)) {
      const [profileId, mode] = at.split("/") as [string, HomeMode];
      if (!s.standard[at]) layouts[at] = normalized(layout, mode, undefined, s.liveOnly, libraries);
      else if (profileId === activeId) layouts[at] = standardLayout(mode, profile, s.liveOnly, libraries);
    }
    return { layouts };
  });
}

followLibraries(useSources.getState().sources);
useSources.subscribe((s) => followLibraries(s.sources));
