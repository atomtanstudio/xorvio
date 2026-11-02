// A media folder's own library page, opened from its Home row's Show All
// or from Sources: its folders and videos, filtered by what each video's
// file holds (GIFs, animations and videos; under a minute or longer). Edit
// puts a checkbox on each: a click ticks one, Shift-click a run of them and
// ⌘-click (Ctrl on Windows and Linux) one more, then Move to… or Delete.
// Deleted files wait in the folder's Recently Deleted for 30 days. Folders
// here are the real ones on the disk: made, renamed, moved and deleted here.

import { useVirtualizer } from "@tanstack/react-virtual";
import { Check, ChevronRight, Folder, FolderInput, FolderPlus, Pencil, Play, RotateCcw, Trash2, TriangleAlert, X } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { create } from "zustand";
import { Art } from "../components/Artwork";
import { confirmAction } from "../components/Confirm";
import { askName } from "../components/EditSheets";
import { openMenu, type MenuEntry } from "../components/Menu";
import { Sheet } from "../components/Sheet";
import { openMovie, playMovie } from "../lib/actions";
import { api, errorText } from "../lib/api";
import { fileSize, playerClock, shortDate } from "../lib/format";
import { useFilledPosters } from "../lib/posterFill";
import { useApp } from "../lib/store";
import type { ClipKind, ClipLength, DeletedBatch, FolderEntry, FolderListing, FolderQuery, FolderSort, FolderVideo } from "../lib/types";
import "../styles/library.css";

/** Videos asked for at a time (the app gives at most 500). */
const PAGE = 200;

const KIND_NAMES: Record<ClipKind, string> = { gif: "GIF", animation: "Animation", video: "Video" };

interface Filters {
  kind: ClipKind | null;
  length: ClipLength | null;
  sort: FolderSort;
  /** Everything below the folder too. */
  below: boolean;
}

/** The filters, kept while going from folder to folder. */
const useFilters = create<Filters & { set(change: Partial<Filters>): void }>((set) => ({
  kind: null,
  length: null,
  sort: "added",
  below: false,
  set: (change) => set(change),
}));

/** Opens a folder: back to it when it's further up the way here, a new page otherwise. */
export function openMediaFolder(sourceId: string, folder: string | null) {
  const { stack, push } = useApp.getState();
  const at = stack.findLastIndex((page) => page.type === "library" && page.sourceId === sourceId && (page.folder ?? null) === folder);
  if (at >= 0) useApp.setState({ stack: stack.slice(0, at + 1) });
  else push({ type: "library", sourceId, folder });
}

const fileStem = (path: string) => {
  const name = path.split("/").pop() ?? path;
  return name.includes(".") ? name.slice(0, name.lastIndexOf(".")) : name;
};

function videoMeta(video: FolderVideo) {
  const facts = video.facts;
  if (!facts) return "Not read yet";
  return [facts.durationSeconds ? playerClock(facts.durationSeconds) : null, video.kind ? KIND_NAMES[video.kind] : null, facts.sizeBytes ? fileSize(facts.sizeBytes) : null].filter(Boolean).join(" · ");
}

const plural = (count: number, one: string, many = `${one}s`) => `${count.toLocaleString()} ${count === 1 ? one : many}`;

export function MediaLibraryScreen({ sourceId, folder }: { sourceId: string; folder: string | null }) {
  const libraryRevision = useApp((s) => s.libraryRevision);
  const density = useApp((s) => s.settings.density);
  const { kind, length, sort, below, set: setFilters } = useFilters();
  const [listing, setListing] = useState<FolderListing | null>(null);
  const [videos, setVideos] = useState<FolderVideo[]>([]);
  const [error, setError] = useState<string | null>(null);
  /** Why a change couldn't be made: it stays until it's closed or another change is asked for (the reload after one clears `error`). */
  const [failure, setFailure] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [selectedFolders, setSelectedFolders] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState<string | null>(null);
  const [moving, setMoving] = useState<{ videos: string[]; folders: string[] } | null>(null);
  const [showDeleted, setShowDeleted] = useState(false);
  const anchor = useRef<number | null>(null);
  const loading = useRef(false);

  const query = useCallback(
    (offset: number, limit = PAGE): FolderQuery => ({ sourceId, folder, below, kind, length, sort, limit, offset }),
    [sourceId, folder, below, kind, length, sort],
  );
  const queryKey = JSON.stringify(query(0));

  // A new folder or filter starts afresh; a change to the library reloads what's loaded, in place.
  const loadedCount = useRef(0);
  loadedCount.current = videos.length;
  const load = useCallback(
    async (fresh: boolean) => {
      loading.current = true;
      try {
        const found = await api.folderListing(query(0, fresh ? PAGE : Math.min(500, Math.max(PAGE, loadedCount.current))));
        setListing(found);
        setVideos(found.videos);
        setError(null);
      } catch (e) {
        setError(errorText(e));
      } finally {
        loading.current = false;
      }
    },
    [query],
  );
  useEffect(() => {
    setVideos([]);
    setListing(null);
    anchor.current = null;
    load(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queryKey]);
  const firstRevision = useRef(libraryRevision);
  useEffect(() => {
    if (libraryRevision !== firstRevision.current) load(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [libraryRevision]);
  // Files read in the background: their type and length now place them.
  const editingRef = useRef(editing);
  editingRef.current = editing;
  useEffect(() => {
    let timer: number | undefined;
    const off = api.onFolderFacts(() => {
      window.clearTimeout(timer);
      timer = window.setTimeout(async () => {
        const { kind: k, length: l } = useFilters.getState();
        // A filter shows more as files are read; while editing, only the counts change, so the ticks stay put.
        if ((k || l) && !editingRef.current) load(false);
        else {
          const counts = await api.folderListing(query(0, 1)).catch(() => null);
          if (counts) setListing((prev) => (prev ? { ...prev, total: counts.total, unread: counts.unread } : prev));
        }
      }, 2500);
    });
    return () => {
      off();
      window.clearTimeout(timer);
    };
  }, [load, query]);

  const loadMore = useCallback(async () => {
    if (loading.current || !listing || videos.length >= listing.total) return;
    loading.current = true;
    try {
      const more = await api.folderListing(query(videos.length));
      setVideos((prev) => {
        const seen = new Set(prev.map((v) => v.movie.id));
        return [...prev, ...more.videos.filter((v) => !seen.has(v.movie.id))];
      });
    } catch (e) {
      setError(errorText(e));
    } finally {
      loading.current = false;
    }
  }, [listing, videos.length, query]);

  // MARK: Choosing

  const stopEditing = useCallback(() => {
    setEditing(false);
    setSelected(new Set());
    setSelectedFolders(new Set());
    anchor.current = null;
  }, []);
  // A new folder or filter starts with nothing ticked.
  useEffect(() => {
    setSelected(new Set());
    setSelectedFolders(new Set());
  }, [queryKey]);

  const tick = (index: number, e: React.MouseEvent) => {
    const id = videos[index].movie.id;
    const next = new Set(selected);
    if (e.shiftKey && anchor.current != null) {
      // A run from the last one ticked, all ticked as that one is.
      const on = selected.has(videos[anchor.current]?.movie.id ?? "") || !selected.size;
      const [from, to] = anchor.current < index ? [anchor.current, index] : [index, anchor.current];
      for (let i = from; i <= to; i++) {
        if (on) next.add(videos[i].movie.id);
        else next.delete(videos[i].movie.id);
      }
    } else if (next.has(id)) next.delete(id);
    else next.add(id);
    anchor.current = index;
    setSelected(next);
  };
  const tickFolder = (path: string) => {
    const next = new Set(selectedFolders);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    setSelectedFolders(next);
  };
  const selectAll = async () => {
    try {
      const ids = await api.folderVideoIds(query(0));
      setSelected(new Set(ids));
      // Folders aren't filtered, so Select All takes them only when nothing is.
      setSelectedFolders(new Set(kind || length ? [] : (listing?.folders ?? []).map((f) => f.path)));
    } catch (e) {
      setFailure(errorText(e));
    }
  };
  const chosenCount = selected.size + selectedFolders.size;

  // Esc leaves Edit (before the window's own Esc, which goes back); ⌘A ticks everything.
  useEffect(() => {
    if (!editing) return;
    const onKey = (e: KeyboardEvent) => {
      const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLSelectElement;
      if (typing || document.querySelector(".sheet-backdrop")) return;
      if (e.key === "Escape") {
        e.stopPropagation();
        stopEditing();
      } else if ((e.metaKey || e.ctrlKey) && (e.key === "a" || e.key === "A")) {
        e.preventDefault();
        selectAll();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  // MARK: Changes

  const sourceName = listing?.trail[0]?.name ?? "this folder";
  const run = async (label: string, change: () => Promise<void>) => {
    setBusy(label);
    setFailure(null);
    try {
      await change();
    } catch (e) {
      setFailure(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  const deleteChosen = async (videoIds: string[], folderPaths: string[]) => {
    if (!videoIds.length && !folderPaths.length) return;
    const what = [folderPaths.length ? plural(folderPaths.length, "folder") : null, videoIds.length ? plural(videoIds.length, "video") : null].filter(Boolean).join(" and ");
    const inFolders = folderPaths.reduce((sum, path) => sum + (listing?.folders.find((f) => f.path === path)?.videos ?? 0), 0);
    const ok = await confirmAction({
      title: `Delete ${what}?`,
      message: `${inFolders ? `The folders hold ${plural(inFolders, "video")}. ` : ""}They go to Recently Deleted in “${sourceName}”, with their pictures and subtitles. You can put them back from there for 30 days, then they're deleted for good.`,
      confirmLabel: "Delete",
      destructive: true,
    });
    if (!ok) return;
    await run(`Deleting ${what}…`, async () => {
      await api.deleteFolderItems(sourceId, videoIds, folderPaths);
      const gone = new Set(videoIds);
      setVideos((prev) => prev.filter((v) => !gone.has(v.movie.id)));
      setListing((prev) => (prev ? { ...prev, total: Math.max(0, prev.total - videoIds.length), folders: prev.folders.filter((f) => !folderPaths.includes(f.path)) } : prev));
      setSelected(new Set());
      setSelectedFolders(new Set());
    });
  };

  const moveChosen = async (to: string) => {
    if (!moving) return;
    const { videos: videoIds, folders: folderPaths } = moving;
    setMoving(null);
    const what = [folderPaths.length ? plural(folderPaths.length, "folder") : null, videoIds.length ? plural(videoIds.length, "video") : null].filter(Boolean).join(" and ");
    await run(`Moving ${what}…`, async () => {
      await api.moveFolderItems(sourceId, videoIds, folderPaths, to);
      setSelected(new Set());
      setSelectedFolders(new Set());
      await load(false);
    });
  };

  const newFolder = (parent: string, then?: (path: string) => void) =>
    askName({
      title: "New Folder",
      placeholder: "Folder name",
      note: `It's made on the disk, in “${parent.split(/[\\/]/).filter(Boolean).pop() ?? parent}”.`,
      confirmLabel: "Make Folder",
      submit: async (name) => {
        const path = await api.createMediaFolder(sourceId, parent, name);
        then?.(path);
        await load(false);
      },
    });

  const renameFolder = (entry: FolderEntry) =>
    askName({
      title: "Rename Folder",
      initial: entry.name,
      placeholder: entry.name,
      note: "The folder is renamed on the disk. What's in it keeps its progress and the rest.",
      confirmLabel: "Rename",
      submit: async (name) => {
        await api.renameMediaFolder(sourceId, entry.path, name);
        await load(false);
      },
    });

  const renameVideo = (video: FolderVideo) =>
    askName({
      title: "Rename Video",
      initial: fileStem(video.path),
      placeholder: fileStem(video.path),
      note: "The file is renamed on the disk (its extension stays), and its pictures and subtitles with it.",
      confirmLabel: "Rename",
      submit: async (name) => {
        await api.renameFolderVideo(sourceId, video.movie.id, name);
        await load(false);
      },
    });

  // MARK: Menus

  /** What a right-click on a video or folder acts on: everything ticked when it's one of them, otherwise just it. */
  const chosenWith = (videoId: string | null, folderPath: string | null) => {
    const isChosen = (videoId && selected.has(videoId)) || (folderPath && selectedFolders.has(folderPath));
    if (editing && isChosen) return { videos: [...selected], folders: [...selectedFolders] };
    return { videos: videoId ? [videoId] : [], folders: folderPath ? [folderPath] : [] };
  };
  const videoMenu = (e: React.MouseEvent, video: FolderVideo) => {
    const chosen = chosenWith(video.movie.id, null);
    const several = chosen.videos.length + chosen.folders.length > 1;
    const items: MenuEntry[] = several
      ? []
      : [
          { label: "Play", icon: <Play size={14} />, onSelect: () => playMovie(video.movie) },
          { label: "Details", onSelect: () => openMovie(video.movie) },
          { separator: true },
          { label: "Rename…", icon: <Pencil size={14} />, onSelect: () => renameVideo(video) },
        ];
    items.push(
      { label: several ? `Move ${chosen.videos.length + chosen.folders.length} to…` : "Move to…", icon: <FolderInput size={14} />, onSelect: () => setMoving(chosen) },
      { label: several ? `Delete ${chosen.videos.length + chosen.folders.length}…` : "Delete…", icon: <Trash2 size={14} />, destructive: true, onSelect: () => deleteChosen(chosen.videos, chosen.folders) },
    );
    openMenu(e, items);
  };
  const folderMenu = (e: React.MouseEvent, entry: FolderEntry) => {
    const chosen = chosenWith(null, entry.path);
    const several = chosen.videos.length + chosen.folders.length > 1;
    const items: MenuEntry[] = several
      ? []
      : [
          { label: "Open", onSelect: () => openMediaFolder(sourceId, entry.path) },
          { label: "Rename…", icon: <Pencil size={14} />, onSelect: () => renameFolder(entry) },
          { label: "New Folder Inside…", icon: <FolderPlus size={14} />, onSelect: () => newFolder(entry.path) },
          { separator: true },
        ];
    items.push(
      { label: several ? `Move ${chosen.videos.length + chosen.folders.length} to…` : "Move to…", icon: <FolderInput size={14} />, onSelect: () => setMoving(chosen) },
      { label: several ? `Delete ${chosen.videos.length + chosen.folders.length}…` : "Delete…", icon: <Trash2 size={14} />, destructive: true, onSelect: () => deleteChosen(chosen.videos, chosen.folders) },
    );
    openMenu(e, items);
  };

  // MARK: Layout

  const here = listing?.trail[listing.trail.length - 1];
  const title = here?.name ?? "";
  const filtered = !!kind || !!length;
  const count = listing ? `${plural(listing.total, "video")}${below ? " here and in folders inside" : ""}` : "";
  const unreadNote =
    listing && listing.unread > 0
      ? filtered
        ? `${plural(listing.unread, "video")} here ${listing.unread === 1 ? "hasn't" : "haven't"} been read yet, so ${listing.unread === 1 ? "it isn't" : "they aren't"} sorted by type or length. Xorvio reads them in the background.`
        : `Reading files: ${listing.unread.toLocaleString()} to go`
      : null;

  return (
    <div className="content fixed media-library">
      <div className="library-header">
        <div className="library-heading">
          {listing && listing.trail.length > 1 && (
            <nav className="library-trail" aria-label="Folders">
              {listing.trail.slice(0, -1).map((step) => (
                <span key={step.path}>
                  <button className="link-button" onClick={() => openMediaFolder(sourceId, step.path === listing.trail[0].path ? null : step.path)}>
                    {step.name}
                  </button>
                  <ChevronRight size={13} />
                </span>
              ))}
            </nav>
          )}
          <h1 className="page-title">{title}</h1>
          <p className="page-subtitle">{[count, unreadNote].filter(Boolean).join(" · ")}</p>
        </div>
        <div className="library-actions">
          <button className="btn-plain" onClick={() => setShowDeleted(true)}>
            <Trash2 size={14} /> Recently Deleted
          </button>
          {listing?.folder && (
            <button className="btn-plain" onClick={() => newFolder(listing.folder!)}>
              <FolderPlus size={14} /> New Folder
            </button>
          )}
          {editing ? (
            <button className="btn-accent" onClick={stopEditing}>
              Done
            </button>
          ) : (
            <button className="btn-plain" onClick={() => setEditing(true)}>
              Edit
            </button>
          )}
        </div>
      </div>
      <div className="filter-bar library-filters">
        <div className="segmented" role="group" aria-label="Type">
          {([null, "video", "animation", "gif"] as (ClipKind | null)[]).map((value) => (
            <button key={value ?? "all"} className={kind === value ? "selected" : ""} onClick={() => setFilters({ kind: value })}>
              {value ? `${KIND_NAMES[value]}s` : "All"}
            </button>
          ))}
        </div>
        <select className="popup-select" value={length ?? ""} onChange={(e) => setFilters({ length: (e.target.value || null) as ClipLength | null })} aria-label="Length">
          <option value="">Any Length</option>
          <option value="short">Under a Minute</option>
          <option value="long">A Minute or Longer</option>
        </select>
        <select className="popup-select" value={sort} onChange={(e) => setFilters({ sort: e.target.value as FolderSort })} aria-label="Sort">
          <option value="added">Newest</option>
          <option value="name">Name</option>
          <option value="length">Longest</option>
          <option value="size">Largest</option>
        </select>
        <button className={`btn-plain${below ? " active" : ""}`} onClick={() => setFilters({ below: !below })} aria-pressed={below}>
          {below && <Check size={13} strokeWidth={2.6} />} Include Folders Inside
        </button>
      </div>
      {error && <div className="library-error">{error}</div>}
      {failure && (
        <div className="library-failure" role="alert">
          <TriangleAlert size={16} />
          <span>{failure}</span>
          <button className="icon-button" aria-label="Close" onClick={() => setFailure(null)}>
            <X size={14} />
          </button>
        </div>
      )}
      <ClipGrid
        listing={listing}
        videos={videos}
        density={density}
        editing={editing}
        selected={selected}
        selectedFolders={selectedFolders}
        onEnd={loadMore}
        onOpenFolder={(entry) => (editing ? tickFolder(entry.path) : openMediaFolder(sourceId, entry.path))}
        onFolderMenu={folderMenu}
        onVideo={(index, e) => {
          if (editing || e.shiftKey || e.metaKey || e.ctrlKey) {
            if (!editing) setEditing(true);
            tick(index, e);
          } else playMovie(videos[index].movie);
        }}
        onVideoMenu={videoMenu}
        empty={
          listing && !listing.folders.length && !videos.length ? (
            <div className="empty-state">
              <div>
                <h3>{filtered ? "Nothing here matches" : "No videos here"}</h3>
                <p>{filtered ? (listing.unread ? "Some videos haven't been read yet; they're placed once they are." : "Try another type or length, or include the folders inside.") : "This folder has no videos of its own."}</p>
              </div>
            </div>
          ) : null
        }
      />
      {(editing || busy) && (
        <div className="library-edit-bar" role="toolbar" aria-label="Chosen videos">
          {busy ? (
            <span className="library-busy">{busy}</span>
          ) : (
            <>
              <span className="library-chosen">{chosenCount ? `${chosenCount.toLocaleString()} chosen` : "Click to choose · Shift-click for a run · ⌘-click for one more"}</span>
              <button className="btn-plain" onClick={selectAll}>
                Select All{listing ? ` ${(listing.total + (filtered ? 0 : listing.folders.length)).toLocaleString()}` : ""}
              </button>
              {chosenCount > 0 && (
                <button className="btn-plain" onClick={() => (setSelected(new Set()), setSelectedFolders(new Set()))}>
                  Select None
                </button>
              )}
              <div style={{ flex: 1 }} />
              <button className="btn-plain" disabled={!chosenCount} onClick={() => setMoving({ videos: [...selected], folders: [...selectedFolders] })}>
                <FolderInput size={14} /> Move to…
              </button>
              <button className="btn-accent danger" disabled={!chosenCount} onClick={() => deleteChosen([...selected], [...selectedFolders])}>
                <Trash2 size={14} /> Delete
              </button>
            </>
          )}
        </div>
      )}
      {moving && listing && (
        <MoveSheet
          sourceId={sourceId}
          start={listing.folder ?? listing.trail[0]?.path ?? null}
          from={below ? null : listing.folder}
          moving={moving}
          onNewFolder={newFolder}
          onMove={moveChosen}
          onClose={() => setMoving(null)}
        />
      )}
      {showDeleted && <RecentlyDeletedSheet sourceId={sourceId} sourceName={sourceName} onRestored={() => load(false)} onClose={() => setShowDeleted(false)} />}
    </div>
  );
}

// MARK: The grid

interface GridProps {
  listing: FolderListing | null;
  videos: FolderVideo[];
  density: string;
  editing: boolean;
  selected: Set<string>;
  selectedFolders: Set<string>;
  onEnd: () => void;
  onOpenFolder: (entry: FolderEntry) => void;
  onFolderMenu: (e: React.MouseEvent, entry: FolderEntry) => void;
  onVideo: (index: number, e: React.MouseEvent) => void;
  onVideoMenu: (e: React.MouseEvent, video: FolderVideo) => void;
  empty: React.ReactNode;
}

function ClipGrid({ listing, videos, density, editing, selected, selectedFolders, onEnd, onOpenFolder, onFolderMenu, onVideo, onVideoMenu, empty }: GridProps) {
  const scroller = useRef<HTMLDivElement>(null);
  const top = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(1000);
  const [topHeight, setTopHeight] = useState(0);
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const observer = new ResizeObserver(() => {
      setWidth(el.clientWidth);
      setTopHeight(top.current?.offsetHeight ?? 0);
    });
    observer.observe(el);
    if (top.current) observer.observe(top.current);
    return () => observer.disconnect();
  }, []);
  const cardWidth = density === "compact" ? 176 : 224;
  const gap = density === "compact" ? 16 : 22;
  const inner = width - 2 * 39;
  const columns = Math.max(2, Math.floor((inner + gap) / (cardWidth + gap)));
  const shownWidth = (inner - (columns - 1) * gap) / columns;
  const rowHeight = Math.round((shownWidth * 9) / 16) + 58;
  const rows = Math.ceil(videos.length / columns);
  const virtualizer = useVirtualizer({ count: rows, getScrollElement: () => scroller.current, estimateSize: () => rowHeight, overscan: 3, scrollMargin: topHeight });
  useEffect(() => virtualizer.measure(), [rowHeight, virtualizer]);
  const virtualRows = virtualizer.getVirtualItems();
  const lastRow = virtualRows[virtualRows.length - 1]?.index ?? 0;
  const firstRow = virtualRows[0]?.index ?? 0;
  const onScreen = useMemo(() => videos.slice(firstRow * columns, (lastRow + 1) * columns).map((v) => v.movie), [videos, firstRow, lastRow, columns]);
  const frames = useFilledPosters(onScreen);
  useEffect(() => {
    if (rows && lastRow >= rows - 3) onEnd();
  }, [lastRow, rows, onEnd]);

  return (
    <div className="browse-scroll library-scroll" ref={scroller}>
      <div ref={top}>
        {listing && listing.folders.length > 0 && (
          <div className="folder-tiles">
            {listing.folders.map((entry) => {
              const on = selectedFolders.has(entry.path);
              return (
                <button key={entry.path} className={`folder-tile${on ? " chosen" : ""}`} onClick={() => onOpenFolder(entry)} onContextMenu={(e) => onFolderMenu(e, entry)}>
                  {editing && <span className={`tick${on ? " on" : ""}`}>{on && <Check size={12} strokeWidth={3} />}</span>}
                  <Folder size={20} className="folder-icon" />
                  <span className="folder-text">
                    <span className="folder-name ellipsis">{entry.name}</span>
                    <span className="folder-count">{plural(entry.videos, "video")}</span>
                  </span>
                </button>
              );
            })}
          </div>
        )}
        {empty}
      </div>
      <div style={{ height: virtualizer.getTotalSize() + 90, position: "relative" }}>
        {virtualRows.map((row) => (
          <div
            key={row.key}
            className="clip-row"
            style={{ top: row.start - topHeight + 4, gridTemplateColumns: `repeat(${columns}, ${shownWidth}px)`, columnGap: gap }}
          >
            {videos.slice(row.index * columns, row.index * columns + columns).map((video, i) => {
              const index = row.index * columns + i;
              const on = selected.has(video.movie.id);
              return (
                <button
                  key={video.movie.id}
                  className={`clip-card${on ? " chosen" : ""}`}
                  onClick={(e) => onVideo(index, e)}
                  onMouseDown={(e) => e.shiftKey && e.preventDefault()}
                  onContextMenu={(e) => onVideoMenu(e, video)}
                  title={video.path}
                >
                  <Art className="clip-frame" src={video.movie.posterUrl ?? frames.get(video.movie.id) ?? null} fallback={video.movie.title}>
                    {editing && <span className={`tick${on ? " on" : ""}`}>{on && <Check size={12} strokeWidth={3} />}</span>}
                    {video.kind && video.kind !== "video" && <span className={`clip-kind ${video.kind}`}>{KIND_NAMES[video.kind]}</span>}
                    {video.facts?.durationSeconds ? <span className="clip-length">{playerClock(video.facts.durationSeconds)}</span> : null}
                  </Art>
                  <div className="clip-title ellipsis">{video.movie.title}</div>
                  <div className="clip-meta ellipsis">{videoMeta(video)}</div>
                </button>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
}

// MARK: Move to…

function MoveSheet({
  sourceId,
  start,
  from,
  moving,
  onNewFolder,
  onMove,
  onClose,
}: {
  sourceId: string;
  start: string | null;
  /** Where everything chosen is now, when it's all in one folder: moving there does nothing. */
  from: string | null;
  moving: { videos: string[]; folders: string[] };
  onNewFolder: (parent: string, then?: (path: string) => void) => Promise<boolean>;
  onMove: (to: string) => void;
  onClose: () => void;
}) {
  const [at, setAt] = useState<string | null>(start);
  const [listing, setListing] = useState<FolderListing | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [round, setRound] = useState(0);
  useEffect(() => {
    let cancelled = false;
    api
      .folderListing({ sourceId, folder: at, below: false, kind: null, length: null, sort: "name", limit: 1, offset: 0 })
      .then((found) => !cancelled && (setListing(found), setError(null)))
      .catch((e) => !cancelled && setError(errorText(e)));
    return () => {
      cancelled = true;
    };
  }, [sourceId, at, round]);
  // A folder can't go inside itself.
  const blocked = (path: string) => moving.folders.some((folder) => path === folder || path.startsWith(`${folder}/`) || path.startsWith(`${folder}\\`));
  const here = listing?.folder ?? null;
  const count = moving.videos.length + moving.folders.length;
  return (
    <Sheet
      title={`Move ${count === 1 ? "1 Item" : `${count.toLocaleString()} Items`} to…`}
      subtitle="They move on the disk, with their pictures and subtitles, and keep their progress."
      width={520}
      onClose={onClose}
      footer={
        <>
          {here && (
            <button className="btn-plain" style={{ marginRight: "auto" }} onClick={() => onNewFolder(here, (path) => (setAt(path), setRound((r) => r + 1)))}>
              <FolderPlus size={14} /> New Folder
            </button>
          )}
          <button className="btn-plain" onClick={onClose}>
            Cancel
          </button>
          <button className="btn-accent" disabled={!here || here === from || blocked(here)} onClick={() => here && onMove(here)}>
            Move Here
          </button>
        </>
      }
    >
      {listing && (
        <nav className="library-trail move-trail" aria-label="Folders">
          {listing.trail.map((step, i) => (
            <span key={step.path}>
              {i > 0 && <ChevronRight size={13} />}
              <button className={i === listing.trail.length - 1 ? "current" : "link-button"} onClick={() => setAt(step.path)}>
                {step.name}
              </button>
            </span>
          ))}
        </nav>
      )}
      <div className="move-folders">
        {listing?.folders.map((entry) => (
          <button key={entry.path} className="move-folder" disabled={blocked(entry.path)} onClick={() => setAt(entry.path)}>
            <Folder size={16} className="folder-icon" />
            <span className="ellipsis">{entry.name}</span>
            <span className="folder-count">{plural(entry.videos, "video")}</span>
            <ChevronRight size={14} />
          </button>
        ))}
        {listing && !listing.folders.length && <p className="muted small">No folders inside this one.</p>}
        {error && <p className="library-error">{error}</p>}
      </div>
    </Sheet>
  );
}

// MARK: Recently Deleted

/** A video file's name, as the app's media folders read them (naming.rs). */
const VIDEO_FILE = /\.(mkv|mp4|m4v|mov|avi|ts|m2ts|mts|webm|wmv|mpg|mpeg)$/i;

function RecentlyDeletedSheet({ sourceId, sourceName, onRestored, onClose }: { sourceId: string; sourceName: string; onRestored: () => void; onClose: () => void }) {
  const [batches, setBatches] = useState<DeletedBatch[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState<string | null>(null);
  const load = useCallback(() => {
    api
      .recentlyDeleted(sourceId)
      .then((found) => (setBatches(found), setError(null)))
      .catch((e) => setError(errorText(e)));
  }, [sourceId]);
  useEffect(load, [load]);
  const act = async (change: () => Promise<void>) => {
    setBusy(true);
    try {
      await change();
      setError(null);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
      load();
    }
  };
  const describe = (batch: DeletedBatch) => {
    const folders = batch.entries.filter((e) => e.folder).length;
    const videos = batch.entries.filter((e) => !e.folder && VIDEO_FILE.test(e.path)).length;
    const others = batch.entries.length - folders - videos;
    return [folders ? plural(folders, "folder") : null, videos ? plural(videos, "video") : null, others ? `${plural(others, "picture or subtitle", "pictures and subtitles")}` : null].filter(Boolean).join(", ");
  };
  const deleteForGood = async (ids: string[] | null) => {
    const ok = await confirmAction({
      title: ids ? "Delete for good?" : "Empty Recently Deleted?",
      message: ids ? "These files are deleted from the disk. This can't be undone." : `Everything in Recently Deleted in “${sourceName}” is deleted from the disk. This can't be undone.`,
      confirmLabel: ids ? "Delete" : "Empty",
      destructive: true,
    });
    if (ok) act(() => api.emptyRecentlyDeleted(sourceId, ids));
  };
  const time = (t: number) => `${shortDate(t)}, ${new Date(t * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}`;
  return (
    <Sheet
      title="Recently Deleted"
      subtitle={`What was deleted in “${sourceName}” waits here 30 days, then it's deleted for good.`}
      width={600}
      onClose={onClose}
      footer={
        <>
          <button className="btn-plain btn-danger" style={{ marginRight: "auto" }} disabled={busy || !batches?.length} onClick={() => deleteForGood(null)}>
            Empty Recently Deleted
          </button>
          <button className="btn-accent" onClick={onClose}>
            Done
          </button>
        </>
      }
    >
      {error && <p className="library-error">{error}</p>}
      {batches && !batches.length && <p className="muted">Nothing's been deleted.</p>}
      <div className="deleted-list">
        {batches?.map((batch) => (
          <div key={batch.id} className="deleted-batch">
            <div className="deleted-line">
              <button className="deleted-summary" onClick={() => setOpen(open === batch.id ? null : batch.id)} aria-expanded={open === batch.id}>
                <ChevronRight size={14} className={open === batch.id ? "turned" : ""} />
                <span>
                  <span className="deleted-what">{describe(batch)}</span>
                  <span className="deleted-when">{time(batch.deletedAt)}</span>
                </span>
              </button>
              <button className="btn-plain" disabled={busy} onClick={() => act(() => api.restoreDeleted(sourceId, batch.id).then(onRestored))}>
                <RotateCcw size={13} /> Put Back
              </button>
              <button className="btn-plain btn-danger" disabled={busy} onClick={() => deleteForGood([batch.id])}>
                Delete Now
              </button>
            </div>
            {open === batch.id && (
              <ul className="deleted-entries">
                {batch.entries.slice(0, 200).map((entry) => (
                  <li key={entry.path} className="ellipsis">
                    {entry.folder && <Folder size={12} />} {entry.path}
                  </li>
                ))}
                {batch.entries.length > 200 && <li className="muted">and {(batch.entries.length - 200).toLocaleString()} more</li>}
              </ul>
            )}
          </div>
        ))}
      </div>
    </Sheet>
  );
}
