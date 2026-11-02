// "New Episode" badges, as Xorvio for Mac's: series with episodes the
// six-hourly check found that haven't been looked at. They follow the
// library, and a series' badge goes as soon as it's opened.

import { useEffect } from "react";
import { create } from "zustand";
import { api } from "./api";
import { useApp } from "./store";

interface NewEpisodesState {
  /** New episodes by series id. */
  counts: Record<string, number>;
  /** The library revision they were last loaded for, so several cards ask once. */
  loadedFor: number;
  load(revision: number): Promise<void>;
  /** A series was opened. */
  seen(seriesId: string): void;
}

export const useNewEpisodes = create<NewEpisodesState>((set, get) => ({
  counts: {},
  loadedFor: -1,
  load: async (revision) => {
    if (get().loadedFor === revision) return;
    set({ loadedFor: revision });
    const counts = await api.newEpisodeCounts().catch(() => null);
    if (counts) set({ counts });
  },
  seen: (seriesId) => {
    if (get().counts[seriesId]) {
      const counts = { ...get().counts };
      delete counts[seriesId];
      set({ counts });
    }
    // The backend may know of some the badges don't yet; it's told either way.
    api.markEpisodesSeen(seriesId).catch(() => {});
  },
}));

/** The badges, kept current as the library changes. */
export function useNewEpisodeCounts() {
  const revision = useApp((s) => s.libraryRevision);
  const counts = useNewEpisodes((s) => s.counts);
  useEffect(() => {
    useNewEpisodes.getState().load(revision);
  }, [revision]);
  return counts;
}

/** "New Episode", or "3 New Episodes". */
export function newEpisodeLabel(count: number) {
  return count === 1 ? "New Episode" : `${count} New Episodes`;
}
