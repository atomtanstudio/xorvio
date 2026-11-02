// What the film, series and category menus of Movies and Series offer:
// hiding a title from the library, marking a film watched or unwatched, and
// pinning a category to Home as a row of its own.

import { useEffect, useMemo, useState } from "react";
import { api, errorText } from "./api";
import { nowSeconds } from "./format";
import { categoryRow, useHomeLayout, useHomeLayouts, type HomeLayout, type HomeMode } from "./homeLayout";
import { useProfiles } from "./profiles";
import { useApp } from "./store";
import type { Category, Movie, PlaybackProgress, Series } from "./types";

const store = () => useApp.getState();

/** The copies of a film the sources carry; just the film for one that isn't in the catalog. */
async function copiesOf(movie: Movie): Promise<Movie[]> {
  const versions = await api.movieVersions(movie.id).catch(() => []);
  return versions.length ? versions.map((v) => v.movie) : [movie];
}

/**
 * Hides a film, with every copy of it, so it doesn't come back under another
 * source's; or a series. Settings › Hidden & Locked shows them again.
 */
export async function hideTitle(item: Movie | Series, kind: "movie" | "series") {
  try {
    const ids = kind === "movie" ? (await copiesOf(item as Movie)).map((m) => m.id) : [item.id];
    for (const id of ids) await api.setHidden(id, true);
    await store().loadEdits();
    store().showToast(`“${item.title}” is hidden. Settings › Hidden & Locked shows it again.`);
  } catch (error) {
    store().showToast(errorText(error), "error");
  }
}

/** Marks a film watched, or unwatched (forgetting where every copy of it got to). */
export async function markMovieWatched(movie: Movie, watched: boolean) {
  try {
    const copies = watched ? [movie] : await copiesOf(movie);
    const items: PlaybackProgress[] = copies.map((m) => ({
      itemId: m.id,
      kind: "movie",
      sourceId: m.sourceId,
      seriesId: null,
      positionSeconds: 0,
      durationSeconds: 0,
      finished: true,
      updatedAt: nowSeconds(),
    }));
    await api.setWatched(items, watched);
    useApp.setState((s) => ({ libraryRevision: s.libraryRevision + 1 }));
  } catch (error) {
    store().showToast(errorText(error), "error");
  }
}

// MARK: Categories on Home

/** Which of Home's layouts is in use, as Home works it out: the add-ons' when they lead and have catalogues. */
async function homeMode(): Promise<HomeMode> {
  const addons = await api.addons().catch(() => []);
  return store().settings.addonsLead && addons.some((a) => a.enabled && a.catalogs.length > 0) ? "addons" : "sources";
}

let modeInUse: HomeMode = "sources";

const layoutKey = (mode: HomeMode) => `${useProfiles.getState().activeId}/${mode}`;

/** The Home layout in use, loaded and kept up to date, so menus know which categories are pinned. Returns their ids. */
export function usePinnedCategories(): Set<string> {
  const addonsLead = useApp((s) => s.settings.addonsLead);
  const [mode, setMode] = useState<HomeMode | null>(null);
  useEffect(() => {
    let cancelled = false;
    homeMode().then((m) => {
      modeInUse = m;
      if (!cancelled) setMode(m);
    });
    return () => {
      cancelled = true;
    };
  }, [addonsLead]);
  const layout = useHomeLayout(mode);
  return useMemo(() => pinnedIn(layout), [layout]);
}

function pinnedIn(layout: HomeLayout | null | undefined) {
  return new Set((layout?.rows ?? []).filter((r) => r.kind === "category" && r.visible && r.categoryId).map((r) => r.categoryId!));
}

/** Whether a category is a row on Home. */
export function isPinned(categoryId: string) {
  return pinnedIn(useHomeLayouts.getState().layouts[layoutKey(modeInUse)]).has(categoryId);
}

/** Adds a category to Home as a row, after Continue Watching, or takes it off. */
export async function pinCategory(category: Category, pinned: boolean) {
  try {
    const mode = await homeMode();
    modeInUse = mode;
    const layouts = useHomeLayouts.getState();
    if (!layouts.layouts[layoutKey(mode)]) await layouts.load(mode);
    const layout = useHomeLayouts.getState().layouts[layoutKey(mode)];
    if (!layout) return;
    const row = categoryRow(category);
    let rows = layout.rows.filter((r) => r.id !== row.id);
    if (pinned) {
      const at = rows[0]?.kind === "continueWatching" ? 1 : 0;
      rows = [...rows.slice(0, at), row, ...rows.slice(at)];
    }
    await useHomeLayouts.getState().save(mode, { ...layout, rows });
    store().showToast(pinned ? `“${category.name}” is a row on Home now.` : `“${category.name}” is no longer on Home.`);
  } catch (error) {
    store().showToast(errorText(error), "error");
  }
}
