// Context menus for titles, channels, categories, groups and programmes,
// shared by every screen.

import {
  ArrowDown,
  ArrowUp,
  Bell,
  BellOff,
  Bookmark,
  BookmarkMinus,
  CalendarSearch,
  CircleCheck,
  CircleDashed,
  CircleDot,
  CircleStop,
  Download,
  Eye,
  EyeOff,
  FolderPlus,
  Grid2x2Plus,
  Info,
  Layers,
  ListMinus,
  ListPlus,
  Lock,
  LockOpen,
  Pencil,
  Pin,
  PinOff,
  Play,
  Repeat,
  RotateCcw,
  Star,
  StarOff,
  Timer,
  Trash2,
  Tv,
  Undo2,
  X,
} from "lucide-react";
import { chooseVersion, deleteGroup, editCatchupOffset, inGroup, moveInList, newGroup, renameGroup, renameItem, resetName, setHidden, setLocked, toggleInGroup } from "../lib/channelEdits";
import {
  cancelOrStopRecording,
  lastChannel,
  markContinueWatched,
  movieEntry,
  openMovie,
  openSeries,
  playChannel,
  playLastChannel,
  playMovie,
  playReplay,
  programEntry,
  recordProgram,
  removeFromContinueWatching,
  resumeContinue,
  seriesEntry,
  setReminder,
  showEntryInGuide,
  toggleFavoriteChannel,
  toggleWatchlist,
} from "../lib/actions";
import { nowSeconds } from "../lib/format";
import { catchupSaveFor, recordSeries, saveFromCatchup, seriesFor, useSeries } from "../lib/series";
import { hideTitle, isPinned, markMovieWatched, pinCategory } from "../lib/titleEdits";
import { recordingFor, useApp } from "../lib/store";
import { externalTitle, openInEntries } from "../player/external";
import { addToMultiview } from "../player/multiviewStore";
import type { Category, Channel, ContinueItem, FavoriteList, Movie, Program, Series, WatchlistEntry } from "../lib/types";
import { isFavoriteChannel } from "../lib/versions";
import { offsetLabel } from "./EditSheets";
import type { MenuEntry } from "./Menu";

const icon = { size: 15, strokeWidth: 1.8 };

/** What a film's menu knows beyond the film: whether it's been watched (both choices show when that isn't known). */
export interface MovieMenuOptions {
  watched?: boolean;
}

export function movieMenu(movie: Movie, options: MovieMenuOptions = {}): MenuEntry[] {
  const saved = useApp.getState().watchlistIds.has(movie.id);
  const inCatalog = movie.stream.type !== "addon";
  const watchedEntries: MenuEntry[] = [];
  if (options.watched !== true) watchedEntries.push({ label: "Mark as Watched", icon: <CircleCheck {...icon} />, onSelect: () => markMovieWatched(movie, true) });
  if (options.watched !== false) watchedEntries.push({ label: "Mark as Unwatched", icon: <CircleDashed {...icon} />, onSelect: () => markMovieWatched(movie, false) });
  return [
    { label: "Play", icon: <Play {...icon} />, onSelect: () => playMovie(movie) },
    { label: "Play from Beginning", icon: <RotateCcw {...icon} />, onSelect: () => playMovie(movie, true) },
    // An add-on film has no address until a copy is chosen.
    ...(movie.stream.type === "addon" ? [] : openInEntries({ kind: "movie", movieId: movie.id }, externalTitle({ kind: "movie", title: movie.title, subtitle: movie.year ? String(movie.year) : null }), movie.id)),
    { label: "Show Details", icon: <Info {...icon} />, onSelect: () => openMovie(movie) },
    { separator: true },
    saved
      ? { label: "Remove from Watchlist", icon: <BookmarkMinus {...icon} />, onSelect: () => toggleWatchlist(movieEntry(movie)) }
      : { label: "Add to Watchlist", icon: <Bookmark {...icon} />, onSelect: () => toggleWatchlist(movieEntry(movie)) },
    ...watchedEntries,
    // Add-on films aren't in the library to hide.
    ...(inCatalog ? [{ separator: true } as MenuEntry, { label: "Hide from Library", icon: <EyeOff {...icon} />, onSelect: () => hideTitle(movie, "movie") }] : []),
  ];
}

/** A Continue Watching card: play it, move past it, or take it off the row. */
export function continueMenu(item: ContinueItem): MenuEntry[] {
  const { push } = useApp.getState();
  const code = item.subtitle?.split(" · ")[0];
  const items: MenuEntry[] = [{ heading: item.title }];
  if (item.upNext) items.push({ label: code ? `Play ${code}` : "Play", icon: <Play {...icon} />, onSelect: () => resumeContinue(item) });
  else
    items.push(
      { label: "Resume", icon: <Play {...icon} />, onSelect: () => resumeContinue(item) },
      { label: "Play from Beginning", icon: <RotateCcw {...icon} />, onSelect: () => resumeContinue(item, true) },
    );
  items.push({ label: "Mark as Watched", icon: <CircleCheck {...icon} />, onSelect: () => markContinueWatched(item) });
  items.push(
    item.kind === "episode"
      ? { label: "Go to Series", icon: <Info {...icon} />, onSelect: () => push({ type: "series", id: item.itemId.split("|ep|")[0] }) }
      : { label: "Show Details", icon: <Info {...icon} />, onSelect: () => push({ type: "movie", id: item.itemId }) },
  );
  items.push({ separator: true }, { label: "Remove from Continue Watching", icon: <X {...icon} />, destructive: true, onSelect: () => removeFromContinueWatching(item) });
  return items;
}

export function seriesMenu(series: Series): MenuEntry[] {
  const saved = useApp.getState().watchlistIds.has(series.id);
  return [
    { label: "Show Episodes", icon: <Info {...icon} />, onSelect: () => openSeries(series) },
    { separator: true },
    saved
      ? { label: "Remove from Watchlist", icon: <BookmarkMinus {...icon} />, onSelect: () => toggleWatchlist(seriesEntry(series)) }
      : { label: "Add to Watchlist", icon: <Bookmark {...icon} />, onSelect: () => toggleWatchlist(seriesEntry(series)) },
    ...(series.id.startsWith("addon|") ? [] : [{ separator: true } as MenuEntry, { label: "Hide from Library", icon: <EyeOff {...icon} />, onSelect: () => hideTitle(series, "series") }]),
  ];
}

/** A list (Favorites or a group) a channel menu was opened in, which it can rearrange. */
export interface ChannelListContext {
  listId: string;
  /** The list's channels as shown. */
  channels: Channel[];
}

/**
 * Watch (or choose a version), Favorites and groups, rearranging a list, the
 * guide and replays, then renaming, hiding and locking. A row of similar
 * channels is one channel here: what's done to it is done to its versions.
 */
export function channelMenu(channel: Channel, list?: ChannelListContext): MenuEntry[] {
  const { favorites, groups, edits } = useApp.getState();
  const favorite = isFavoriteChannel(favorites, channel);
  const last = lastChannel();
  const versions = channel.versions?.length ?? 0;
  const items: MenuEntry[] = [
    { label: "Watch", icon: <Play {...icon} />, onSelect: () => playChannel(channel) },
    { label: "Watch Full Screen", icon: <Tv {...icon} />, onSelect: () => playChannel(channel, "full") },
    { label: "Add to Multiview", icon: <Grid2x2Plus {...icon} />, onSelect: () => addToMultiview(channel) },
  ];
  if (versions > 1) items.push({ label: `Versions (${versions})…`, icon: <Layers {...icon} />, onSelect: () => chooseVersion(channel) });
  items.push(
    { label: last ? `Last Channel: ${last.name}` : "Last Channel", icon: <Undo2 {...icon} />, shortcut: "L", disabled: !last, onSelect: playLastChannel },
    ...openInEntries({ kind: "channel", channelId: channel.id }, channel.name),
    { separator: true },
    {
      label: favorite ? "Remove from Favorites" : "Add to Favorites",
      icon: favorite ? <StarOff {...icon} /> : <Star {...icon} />,
      onSelect: () => toggleFavoriteChannel(channel),
    },
    ...groups.map((group): MenuEntry => {
      const member = inGroup(group, channel);
      return {
        label: member ? `Remove from ${group.name}` : `Add to ${group.name}`,
        icon: member ? <ListMinus {...icon} /> : <ListPlus {...icon} />,
        onSelect: () => toggleInGroup(group, channel),
      };
    }),
    { label: "New Group…", icon: <FolderPlus {...icon} />, onSelect: () => newGroup(channel) },
  );
  if (list) {
    const at = list.channels.findIndex((c) => c.id === channel.id);
    items.push(
      { separator: true },
      { label: "Move Up", icon: <ArrowUp {...icon} />, disabled: at <= 0, onSelect: () => moveInList(list.listId, list.channels, channel, -1) },
      { label: "Move Down", icon: <ArrowDown {...icon} />, disabled: at < 0 || at >= list.channels.length - 1, onSelect: () => moveInList(list.listId, list.channels, channel, 1) },
    );
  }
  items.push({ separator: true }, { label: "Match Guide…", icon: <CalendarSearch {...icon} />, onSelect: () => useApp.getState().showGuideMatch(channel) });
  if (channel.catchupDays > 0) {
    const offset = channel.catchupOffsetMinutes;
    items.push({ label: offset ? `Catch-up Offset (${offsetLabel(offset)})…` : "Catch-up Offset…", icon: <Timer {...icon} />, onSelect: () => editCatchupOffset(channel) });
  }
  items.push(...editEntries(channel, "Channel", edits.get(channel.id)));
  return items;
}

/** A category of Live TV, Movies or Series: rename, hide or lock it, or pin it to Home as a row. */
export function categoryMenu(category: Category): MenuEntry[] {
  const pinned = isPinned(category.id);
  return [
    { heading: category.name },
    pinned
      ? { label: "Unpin from Home", icon: <PinOff {...icon} />, onSelect: () => pinCategory(category, false) }
      : { label: "Pin to Home", icon: <Pin {...icon} />, onSelect: () => pinCategory(category, true) },
    { separator: true },
    ...editEntries(category, "Category", useApp.getState().edits.get(category.id)),
  ];
}

/** One of My Groups. */
export function groupMenu(group: FavoriteList): MenuEntry[] {
  return [
    { heading: group.name },
    { label: "Rename Group…", icon: <Pencil {...icon} />, onSelect: () => renameGroup(group) },
    { separator: true },
    { label: "Delete Group…", icon: <Trash2 {...icon} />, destructive: true, onSelect: () => deleteGroup(group) },
  ];
}

function editEntries(item: Channel | Category, what: "Channel" | "Category", edit: { displayName: string | null; hidden: boolean; locked: boolean } | undefined): MenuEntry[] {
  const items: MenuEntry[] = [{ label: "Rename…", icon: <Pencil {...icon} />, onSelect: () => renameItem(item, what) }];
  if (edit?.displayName) items.push({ label: "Reset Name", icon: <RotateCcw {...icon} />, onSelect: () => resetName(item) });
  items.push(
    edit?.hidden
      ? { label: `Show ${what}`, icon: <Eye {...icon} />, onSelect: () => setHidden(item, false) }
      : { label: `Hide ${what}`, icon: <EyeOff {...icon} />, onSelect: () => setHidden(item, true) },
    edit?.locked
      ? { label: "Remove Lock", icon: <LockOpen {...icon} />, onSelect: () => setLocked(item, false) }
      : { label: "Lock with PIN", icon: <Lock {...icon} />, onSelect: () => setLocked(item, true) },
  );
  return items;
}

/**
 * Watch, replay, record, remind: what makes sense for the programme's time.
 * A saved programme's reminder turns off and on without leaving the
 * watchlist. From the watchlist (`saved`, the entry as saved) it also offers
 * Show in Guide.
 */
export function programMenu(program: Program, channel: Channel, saved?: WatchlistEntry): MenuEntry[] {
  const { watchlistIds, reminderIds, recordings } = useApp.getState();
  const t = nowSeconds();
  const past = program.end <= t;
  const airing = program.start <= t && t < program.end;
  const onWatchlist = watchlistIds.has(program.id);
  const recording = recordingFor(recordings, channel.id, program.start, program.id);
  const items: MenuEntry[] = [{ heading: program.title }];
  if (airing) items.push({ label: "Watch Now", icon: <Play {...icon} />, onSelect: () => playChannel(channel) });
  if (past && channel.catchupDays > 0 && t - program.start < channel.catchupDays * 86400) {
    items.push({ label: "Play Replay", icon: <RotateCcw {...icon} />, onSelect: () => playReplay(channel, program) });
    const saved = catchupSaveFor(program, channel);
    items.push({ label: saved ? "Saved from Catch-up" : "Save from Catch-up", icon: <Download {...icon} />, disabled: !!saved, onSelect: () => saveFromCatchup(program, channel) });
  }
  if (!past) {
    if (recording && (recording.status === "scheduled" || recording.status === "recording"))
      items.push({
        label: recording.status === "recording" ? "Stop Recording" : "Cancel Recording",
        icon: <CircleStop {...icon} />,
        destructive: true,
        onSelect: () => cancelOrStopRecording(program, channel),
      });
    else items.push({ label: airing ? "Record Now" : "Record", icon: <CircleDot {...icon} />, onSelect: () => recordProgram(program, channel) });
    const series = seriesFor(useSeries.getState().rules, program, channel);
    items.push({ label: series ? "Series Options…" : "Record Series…", icon: <Repeat {...icon} />, onSelect: () => recordSeries(program, channel) });
    if (!airing && !onWatchlist) items.push({ label: "Remind Me & Add to Watchlist", icon: <Bell {...icon} />, onSelect: () => toggleWatchlist(programEntry(program, channel)) });
    if (saved && !airing) items.push({ label: "Show in Guide", icon: <CalendarSearch {...icon} />, onSelect: () => showEntryInGuide(saved) });
  }
  items.push({ separator: true }, { label: "Watch Channel", icon: <Tv {...icon} />, onSelect: () => playChannel(channel) });
  // On the watchlist: the reminder goes off and on by itself, as in Xorvio for Mac.
  if (onWatchlist && !past) {
    const entry = { ...(saved ?? programEntry(program, channel)), remind: reminderIds.has(program.id) };
    items.push({ separator: true });
    if (!airing)
      items.push(
        entry.remind
          ? { label: "Turn Off Reminder", icon: <BellOff {...icon} />, onSelect: () => setReminder(entry, false) }
          : { label: "Remind Me", icon: <Bell {...icon} />, onSelect: () => setReminder(entry, true) },
      );
    items.push({ label: "Remove from Watchlist", icon: <BookmarkMinus {...icon} />, onSelect: () => toggleWatchlist(entry) });
  }
  return items;
}
