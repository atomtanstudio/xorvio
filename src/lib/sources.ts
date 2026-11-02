// The sources and their refreshes, shared by the Sources screen, the
// sidebar (a badge for failing sources, progress at its foot), the toolbar's
// spinner, the menu bar and first-run onboarding. Refreshes started in the
// background by the app's scheduler show here too: their progress arrives
// as events, and each one's end as `refresh-finished`.

import { create } from "zustand";
import { api, errorText } from "./api";
import { useApp } from "./store";
import type { RefreshProgress, SourceKind, SourceStatus, SourceSummary } from "./types";

/** The steps a refresh reports start with this one (xorvio-core's sources::phase). */
const FIRST_STEP = "Connecting";

interface SourcesState {
  /** In the user's order; null until first loaded. */
  sources: SourceSummary[] | null;
  /** Refreshes running, by source id, with the step each is on. */
  progress: Map<string, RefreshProgress>;
  /** Refreshes someone asked to stop, which haven't returned yet. */
  stopping: Set<string>;
  /** The add sheet was asked for from elsewhere (File › Add Source…, the toolbar), with the kind to start on; the Sources screen opens it. */
  pendingAdd: { kind: SourceKind | null } | null;
  load(): Promise<void>;
  /** Refreshes a source, catalog then guide; resolves to its status, or null when it couldn't run. */
  refresh(id: string): Promise<SourceStatus | null>;
  refreshAll(): void;
  /** Stops a refresh; the source keeps what it had. */
  cancel(id: string): Promise<void>;
  /** Moves a source to `to` among the sources (Live TV lists channels in this order). */
  move(id: string, to: number): Promise<void>;
  /** Goes to Sources and opens the add sheet there. */
  requestAdd(kind?: SourceKind | null): void;
}

export const useSources = create<SourcesState>((set, get) => ({
  sources: null,
  progress: new Map(),
  stopping: new Set(),
  pendingAdd: null,

  load: async () => {
    const sources = await api.listSources().catch(() => get().sources ?? []);
    set((s) => {
      // A refresh already running when this window opened shows without its step until the next one.
      const progress = new Map(s.progress);
      for (const summary of sources) {
        if (summary.refreshing && !progress.has(summary.config.id)) progress.set(summary.config.id, { sourceId: summary.config.id, phase: "Refreshing", fraction: null });
      }
      return { sources, progress };
    });
  },

  refresh: async (id) => {
    set((s) => ({ progress: new Map(s.progress).set(id, s.progress.get(id) ?? { sourceId: id, phase: FIRST_STEP, fraction: null }) }));
    try {
      const status = await api.refreshSource(id);
      const { loadLibrary, guideChanged } = useApp.getState();
      loadLibrary().catch(() => {});
      guideChanged();
      return status;
    } catch (e) {
      useApp.getState().showToast(errorText(e), "error");
      return null;
    } finally {
      finished(id);
      await get().load();
    }
  },

  refreshAll: () => {
    for (const s of get().sources ?? []) if (!s.config.isPaused && !get().progress.has(s.config.id)) get().refresh(s.config.id);
  },

  cancel: async (id) => {
    set((s) => ({ stopping: new Set(s.stopping).add(id) }));
    const name = get().sources?.find((s) => s.config.id === id)?.config.name ?? "the source";
    // Stopped before its new catalogue was written, a source keeps the one it had; after, only the guide stays as it was.
    if (await api.cancelRefresh(id).catch(() => false)) useApp.getState().showToast(`Stopped refreshing ${name}.`);
    else set((s) => withoutStopping(s.stopping, id));
  },

  move: async (id, to) => {
    const list = get().sources ?? [];
    const from = list.findIndex((s) => s.config.id === id);
    if (from < 0 || to < 0 || to >= list.length || from === to) return;
    const next = [...list];
    const [moved] = next.splice(from, 1);
    next.splice(to, 0, moved);
    // Shown in the new order at once; the backend's order is what lasts.
    set({ sources: next });
    try {
      await api.reorderSources(next.map((s) => s.config.id));
    } catch (e) {
      useApp.getState().showToast(errorText(e), "error");
    }
    await get().load();
  },

  requestAdd: (kind = null) => {
    set({ pendingAdd: { kind } });
    useApp.getState().go("sources");
  },
}));

function withoutStopping(stopping: Set<string>, id: string) {
  const next = new Set(stopping);
  next.delete(id);
  return { stopping: next };
}

/** A refresh ended: its progress goes. */
function finished(id: string) {
  useSources.setState((s) => {
    if (!s.progress.has(id) && !s.stopping.has(id)) return s;
    const progress = new Map(s.progress);
    progress.delete(id);
    return { progress, ...withoutStopping(s.stopping, id) };
  });
}

/**
 * Follows refreshes from anywhere (the scheduler's too) for as long as the
 * window is open. Late steps from a refresh that has ended are ignored: a
 * new refresh always begins with its first step.
 */
export function followSources() {
  useSources.getState().load();
  const offProgress = api.onRefreshProgress((p) => {
    const { progress } = useSources.getState();
    if (!progress.has(p.sourceId) && p.phase !== FIRST_STEP) return;
    useSources.setState({ progress: new Map(progress).set(p.sourceId, p) });
  });
  const offFinished = api.onRefreshFinished((id) => {
    finished(id);
    useSources.getState().load();
  });
  const offLibrary = api.onLibraryChanged(() => useSources.getState().load());
  return () => {
    offProgress();
    offFinished();
    offLibrary();
  };
}

/** Sources whose last refresh failed (paused ones aside), for the sidebar's badge. */
export function failingSources(sources: SourceSummary[] | null) {
  return (sources ?? []).filter((s) => !s.config.isPaused && !!s.status.lastError);
}
