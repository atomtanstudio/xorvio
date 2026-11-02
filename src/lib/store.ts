// App-wide state: where the person is, what's playing and where the video
// shows, and small caches several screens share (favorites, channel groups,
// renamed/hidden/locked items, watchlist ids, recordings). Screens load their
// own content through `api`.

import { create } from "zustand";
import { api } from "./api";
import type { Channel, FavoriteList, ItemEdit, MediaKind, ParentalStatus, PlayRequest, ProgramHit, Recording, ResolvedStream, Settings, TraktRowRef, WatchlistEntry } from "./types";
import { defaultSettings } from "./types";
import { stopPreviews } from "./previewClips";
import { sameChannel } from "./versions";
import { isPopOutWindow } from "./windows";

export type Tab = "home" | "watchlist" | "live" | "movies" | "series" | "recordings" | "discover" | "sources" | "settings";

export type Page =
  | { type: "movie"; id: string }
  | { type: "series"; id: string }
  | { type: "search"; query: string }
  | { type: "continue" }
  /** Someone from a cast: TMDB's id when the cast came from TMDB, and the title they were chosen from. */
  | { type: "person"; name: string; tmdbId?: string | null; fromId?: string | null }
  /** A Trakt chart, list or recommendations in full. */
  | { type: "trakt"; trakt: TraktRowRef }
  /** A media folder's own library page: one of its folders (null for its own). */
  | { type: "library"; sourceId: string; folder: string | null };

/** An add-on title (or episode) to choose a copy of. */
export interface CopiesRequest {
  mediaType: string;
  mediaId: string;
  /** Progress is saved against this id. */
  itemId: string;
  title: string;
  subtitle?: string | null;
  artwork?: string | null;
  year?: number | null;
  /** For an episode, so the provider's copy of the same episode can be found. */
  episode?: { season: number; number: number } | null;
  /** Play the chosen copy from the beginning rather than where it was left. */
  fromStart?: boolean;
  /**
   * Switching copies while one plays: the moment to carry on from, the
   * copy playing (marked in the list), and whether closing the list
   * without choosing should play on.
   */
  switching?: { at: number; playingUrl: string | null; resume: boolean } | null;
}

/** Where the video appears. One element, moved between places, so switching never restarts the stream. */
export type Presentation = "hidden" | "full" | "mini" | "docked";

export interface NowPlaying {
  request: PlayRequest;
  kind: MediaKind;
  /** The item progress is saved against. */
  itemId: string;
  sourceId: string;
  seriesId?: string | null;
  title: string;
  subtitle?: string | null;
  artworkUrl?: string | null;
  channel?: Channel;
  resumeFrom?: number;
  durationHint?: number | null;
  /** The add-on copy playing, so the next episode can play a similar one. */
  copy?: { addonName: string; quality: string | null } | null;
}

/** What moves between the main window and the pop-out player: the item, and where it was. */
export interface PopOutHandoff {
  item: NowPlaying;
  /** Seconds in, for films and episodes; 0 for live channels. */
  position: number;
}

interface AppState {
  tab: Tab;
  /** Pages pushed on top of the tab's own screen, like a navigation stack. */
  stack: Page[];
  searchText: string;
  /** Titles pages show in the toolbar once they've loaded, by page id. */
  pageTitles: Record<string, string>;
  setPageTitle(id: string, title: string): void;
  go(tab: Tab): void;
  /** Movies or Series as one source's, as a library row's Show All opens them; Browse takes it when it opens. */
  browseStart: { kind: "movie" | "series"; sourceId: string } | null;
  browseSource(kind: "movie" | "series", sourceId: string): void;
  push(page: Page): void;
  back(): void;
  setSearchText(text: string): void;

  settings: Settings;
  loadSettings(): Promise<void>;
  updateSettings(patch: Partial<Settings>): void;

  nowPlaying: NowPlaying | null;
  stream: ResolvedStream | null;
  presentation: Presentation;
  playError: string | null;
  play(item: NowPlaying, presentation?: Presentation): Promise<void>;
  setPresentation(p: Presentation): void;
  stop(): void;
  /** Channels tuned to, newest first, for Last Channel. Seeded from Recently Watched. */
  channelTrail: Channel[];
  /** In the main window: the video is playing in the pop-out player, so what's chosen plays there. */
  poppedOut: boolean;

  favorites: Set<string>;
  watchlistIds: Set<string>;
  /** Saved programmes whose reminder is on (a reminder can be off with the programme still saved). */
  reminderIds: Set<string>;
  recordings: Map<string, Recording>;
  /** Bumped when library state changes so rows reload. */
  libraryRevision: number;
  loadLibrary(): Promise<void>;
  toggleFavorite(channelId: string): Promise<void>;
  setWatchlisted(entry: WatchlistEntry, saved: boolean): Promise<void>;
  upsertRecording(recording: Recording): void;
  /** Re-reads recordings from the recorder, which decides how each one ended. */
  refreshRecordings(): Promise<void>;

  /** Channel lists people named (My Groups). */
  groups: FavoriteList[];
  /** Renamed, hidden and locked channels and categories, by id. */
  edits: Map<string, ItemEdit>;
  parental: ParentalStatus;
  /** Bumped when names, hiding, locks or groups change, so channel lists reload. */
  editsRevision: number;
  loadEdits(): Promise<void>;

  /** The copies sheet for an add-on title, opened from anywhere. */
  copies: CopiesRequest | null;
  showCopies(request: CopiesRequest | null): void;

  /** A guide programme shown in a sheet, with Watch, Record and Remind. */
  programSheet: ProgramHit | null;
  showProgram(hit: ProgramHit | null): void;

  /** A programme for Live TV's guide to show, on its day and at its time (Show in Guide). */
  guideTarget: ProgramHit | null;
  showInGuide(hit: ProgramHit | null): void;

  /** The channel whose guide channel is being chosen, in the Match Guide sheet. */
  guideMatch: Channel | null;
  showGuideMatch(channel: Channel | null): void;
  /** Bumped when channels show different listings (a guide matched, added or loaded) so the guide reloads. */
  guideRevision: number;
  guideChanged(): void;

  toast: { text: string; tone?: "info" | "error" } | null;
  showToast(text: string, tone?: "info" | "error"): void;
}

let toastTimer: ReturnType<typeof setTimeout> | undefined;

export const useApp = create<AppState>((set, get) => ({
  tab: "home",
  stack: [],
  searchText: "",
  pageTitles: {},
  setPageTitle: (id, title) => set((s) => (s.pageTitles[id] === title ? s : { pageTitles: { ...s.pageTitles, [id]: title } })),
  browseStart: null,
  browseSource: (kind, sourceId) => {
    set({ browseStart: { kind, sourceId } });
    get().go(kind === "movie" ? "movies" : "series");
  },
  go: (tab) => {
    const { presentation } = get();
    // Leaving Live TV sends its docked video to the corner, as in the Swift app.
    set({ tab, stack: [], presentation: presentation === "docked" && tab !== "live" ? "mini" : presentation });
  },
  push: (page) => set((s) => ({ stack: [...s.stack, page] })),
  back: () => set((s) => ({ stack: s.stack.slice(0, -1) })),
  setSearchText: (text) => {
    const { stack } = get();
    const top = stack[stack.length - 1];
    if (!text.trim()) set({ searchText: text, stack: top?.type === "search" ? stack.slice(0, -1) : stack });
    else if (top?.type === "search") set({ searchText: text, stack: [...stack.slice(0, -1), { type: "search", query: text }] });
    else set({ searchText: text, stack: [...stack, { type: "search", query: text }] });
  },

  settings: defaultSettings,
  loadSettings: async () => set({ settings: { ...defaultSettings, ...(await api.settings()) } }),
  updateSettings: (patch) => {
    const settings = { ...get().settings, ...patch };
    set({ settings });
    api.saveSettings(settings).catch(() => {});
  },

  nowPlaying: null,
  stream: null,
  presentation: "hidden",
  playError: null,
  play: async (item, presentation = "full") => {
    // A preview clip on Home holds a stream of its own; it gives way first.
    stopPreviews();
    if (get().poppedOut && !isPopOutWindow) {
      api.popOut({ item, position: item.resumeFrom ?? 0 }, get().settings.keepPopOutOnTop).catch((error) => get().showToast(String(error), "error"));
      return;
    }
    const previous = get().stream;
    const channel = item.channel;
    // Versions of one channel are one place on the trail, so Last Channel never flips between them.
    if (channel) set((s) => ({ channelTrail: [channel, ...s.channelTrail.filter((c) => !sameChannel(c, channel))].slice(0, 20) }));
    set({ nowPlaying: item, stream: null, playError: null, presentation });
    if (previous) api.releaseStream(previous.url).catch(() => {});
    try {
      const stream = await api.play(item.request);
      if (get().nowPlaying !== item) {
        api.releaseStream(stream.url).catch(() => {});
        return;
      }
      set({ stream });
      if (item.channel) api.recordChannelWatched(item.channel.id).catch(() => {});
      else if (item.kind === "movie" || item.kind === "episode") api.recordWatched(item.itemId, item.kind, item.sourceId, item.title, item.subtitle ?? null).catch(() => {});
    } catch (error) {
      if (get().nowPlaying === item) set({ playError: typeof error === "string" ? error : (error as Error).message });
    }
  },
  setPresentation: (presentation) => set({ presentation }),
  stop: () => {
    const { stream } = get();
    if (stream) api.releaseStream(stream.url).catch(() => {});
    set({ nowPlaying: null, stream: null, presentation: "hidden", playError: null });
  },
  channelTrail: [],
  poppedOut: false,

  favorites: new Set(),
  watchlistIds: new Set(),
  reminderIds: new Set(),
  recordings: new Map(),
  libraryRevision: 0,
  loadLibrary: async () => {
    const [favorites, watchlist, recordings] = await Promise.all([api.favoriteChannelIds(), api.watchlist(), api.recordings(), get().loadEdits()]);
    if (!get().channelTrail.length) api.recentChannels(20).then((recent) => !get().channelTrail.length && set({ channelTrail: recent })).catch(() => {});
    set((s) => ({
      favorites: new Set(favorites),
      watchlistIds: new Set(watchlist.map((w) => w.id)),
      reminderIds: new Set(watchlist.filter((w) => w.remind).map((w) => w.id)),
      recordings: new Map(recordings.map((r) => [r.id, r])),
      libraryRevision: s.libraryRevision + 1,
    }));
  },
  toggleFavorite: async (channelId) => {
    const favorite = !get().favorites.has(channelId);
    await api.setFavorite(channelId, favorite);
    set((s) => {
      const favorites = new Set(s.favorites);
      if (favorite) favorites.add(channelId);
      else favorites.delete(channelId);
      return { favorites, libraryRevision: s.libraryRevision + 1 };
    });
  },
  setWatchlisted: async (entry, saved) => {
    await api.setWatchlisted(entry, saved);
    set((s) => {
      const watchlistIds = new Set(s.watchlistIds);
      const reminderIds = new Set(s.reminderIds);
      if (saved) watchlistIds.add(entry.id);
      else watchlistIds.delete(entry.id);
      if (saved && entry.remind) reminderIds.add(entry.id);
      else reminderIds.delete(entry.id);
      return { watchlistIds, reminderIds, libraryRevision: s.libraryRevision + 1 };
    });
  },
  refreshRecordings: async () => {
    const list = await api.recordings();
    set({ recordings: new Map(list.map((r) => [r.id, r])) });
  },
  upsertRecording: (recording) =>
    set((s) => {
      const recordings = new Map(s.recordings);
      recordings.set(recording.id, recording);
      return { recordings };
    }),

  groups: [],
  edits: new Map(),
  parental: { hasPin: false, unlocked: false, lockedCount: 0 },
  editsRevision: 0,
  loadEdits: async () => {
    const [groups, edits, parental] = await Promise.all([api.favoriteLists(), api.itemEdits(), api.parentalStatus()]);
    // Home's rows leave out hidden and locked channels too, so they reload as well.
    set((s) => ({
      groups,
      edits: new Map(edits.map((e) => [e.itemId, e])),
      parental,
      editsRevision: s.editsRevision + 1,
      libraryRevision: s.libraryRevision + 1,
    }));
  },

  copies: null,
  showCopies: (copies) => set({ copies }),

  programSheet: null,
  showProgram: (programSheet) => set({ programSheet }),

  guideTarget: null,
  showInGuide: (guideTarget) => {
    if (guideTarget) get().go("live");
    set({ guideTarget });
  },

  guideMatch: null,
  showGuideMatch: (guideMatch) => set({ guideMatch }),
  guideRevision: 0,
  guideChanged: () => set((s) => ({ guideRevision: s.guideRevision + 1 })),

  toast: null,
  showToast: (text, tone = "info") => {
    clearTimeout(toastTimer);
    set({ toast: { text, tone } });
    toastTimer = setTimeout(() => set({ toast: null }), 3500);
  },
}));

/** Active or scheduled recordings for a guide programme, matched by id or by channel and time. */
export function recordingFor(recordings: Map<string, Recording>, channelId: string, start: number, programId?: string) {
  for (const r of recordings.values()) {
    if (r.status === "cancelled" || r.status === "failed") continue;
    if ((programId && r.programId === programId) || (r.channelId === channelId && r.start === start)) return r;
  }
  return undefined;
}
