// What each source is called, for the tags on search results and the
// Source picker and column in Movies and Series.

import { useEffect } from "react";
import { create } from "zustand";
import { api } from "./api";
import type { SourceConfig } from "./types";

const useSourceList = create<{ sources: SourceConfig[] | null; load(): void }>((set) => ({
  sources: null,
  load: () => {
    api
      .listSources()
      .then((list) => set({ sources: list.map((s) => s.config) }))
      .catch(() => set({ sources: [] }));
  },
}));

/** Every source in the person's order, read again whenever a screen using it opens. */
export function useSources(): SourceConfig[] {
  const sources = useSourceList((s) => s.sources);
  useEffect(() => useSourceList.getState().load(), []);
  return sources ?? [];
}

/** A source's name by id ("Add-ons" for titles from add-ons). */
export function useSourceName(): (sourceId: string) => string {
  const sources = useSources();
  return (sourceId) => (sourceId === "addon" ? "Add-ons" : (sources.find((s) => s.id === sourceId)?.name ?? ""));
}
