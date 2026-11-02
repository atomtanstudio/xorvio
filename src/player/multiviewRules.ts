// Multiview's rules, kept apart from React and the backend so they can be
// checked on their own (`node src/player/multiviewRules.test.ts`): where
// tiles go, which one is heard, when Multiview gives way to the player,
// pausing and rewinding tiles, and the inset (one tile small in a corner of
// the other).

/** Tiles at once: four streams is about what a provider and a Mac manage. */
export const MAX_TILES = 4;

/** Where a tile sits in the layout's grid of 16:9 cells (CSS grid lines, from 1). */
export interface TilePlace {
  id: string;
  column: number;
  row: number;
  /** Cells it covers each way: the large tile of three covers two. */
  span: number;
}

export interface MultiviewLayout {
  columns: number;
  rows: number;
  places: TilePlace[];
  /** Tiles that keep playing out of sight while another fills the view. */
  hidden: string[];
  /** The cell beside a lone tile that asks for another channel. */
  invite: { column: number; row: number } | null;
  /** The tile shown small in a corner, over the one filling the view. */
  inset: string | null;
}

/**
 * Two side by side; three as one large tile with two stacked beside it (as
 * the Swift app had them, sized so their edges line up); four in a 2×2
 * grid. An expanded tile fills the view on its own. With the inset on, two
 * tiles are one filling the view (the first) and the other small over it.
 */
export function layoutFor(ids: string[], expanded: string | null, inset = false): MultiviewLayout {
  const at = (id: string, column: number, row: number, span = 1): TilePlace => ({ id, column, row, span });
  const none = { hidden: [], invite: null, inset: null };
  if (inset && canInset(ids.length)) return { ...none, columns: 1, rows: 1, places: [at(ids[0], 1, 1)], inset: ids[1] };
  if (expanded && ids.includes(expanded)) return { ...none, columns: 1, rows: 1, places: [at(expanded, 1, 1)], hidden: ids.filter((id) => id !== expanded) };
  const [a, b, c, d] = ids;
  switch (ids.length) {
    case 0:
      return { ...none, columns: 1, rows: 1, places: [], invite: { column: 1, row: 1 } };
    case 1:
      return { ...none, columns: 2, rows: 1, places: [at(a, 1, 1)], invite: { column: 2, row: 1 } };
    case 2:
      return { ...none, columns: 2, rows: 1, places: [at(a, 1, 1), at(b, 2, 1)] };
    case 3:
      return { ...none, columns: 3, rows: 2, places: [at(a, 1, 1, 2), at(b, 3, 1), at(c, 3, 2)] };
    default:
      return { ...none, columns: 2, rows: 2, places: [at(a, 1, 1), at(b, 2, 1), at(c, 1, 2), at(d, 2, 2)], hidden: ids.slice(MAX_TILES) };
  }
}

/**
 * The largest grid of 16:9 cells, `gap` apart, that fits the space, so every
 * tile keeps the picture's shape and the whole layout letterboxes instead.
 */
export function fitGrid(space: { width: number; height: number }, layout: { columns: number; rows: number }, gap: number) {
  const { columns, rows } = layout;
  const cell = Math.min((space.width - (columns - 1) * gap) / columns, ((space.height - (rows - 1) * gap) / rows) * (16 / 9));
  if (!(cell > 0)) return { width: 0, height: 0 };
  return { width: Math.floor(cell * columns + (columns - 1) * gap), height: Math.floor(cell * (9 / 16) * rows + (rows - 1) * gap) };
}

/** Which tile is heard, and which (if any) fills the view. */
export interface Focus {
  audio: string | null;
  expanded: string | null;
}

/** A new tile is heard only when nothing else is, and shows in the grid. */
export function focusAfterAdd(focus: Focus, id: string): Focus {
  return { audio: focus.audio ?? id, expanded: null };
}

/** Taking away the tile that's heard moves the sound to the first one left. */
export function focusAfterRemove(ids: string[], focus: Focus, removed: string): Focus {
  const left = ids.filter((id) => id !== removed);
  return {
    audio: focus.audio === removed || !focus.audio ? (left[0] ?? null) : focus.audio,
    expanded: focus.expanded === removed ? null : focus.expanded,
  };
}

/** Hearing a tile while another fills the view brings it to the front, so what's heard is what's seen. */
export function listenTo(focus: Focus, id: string): Focus {
  return { audio: id, expanded: focus.expanded && focus.expanded !== id ? id : focus.expanded };
}

/** Expanding a tile also makes it the one heard; the same again returns to the grid. */
export function toggleExpanded(focus: Focus, id: string): Focus {
  return focus.expanded === id ? { ...focus, expanded: null } : { audio: id, expanded: id };
}

/** The tile keys 1–4 pick, in layout order. */
export function tileForKey(ids: string[], key: string): string | null {
  return /^[1-4]$/.test(key) ? (ids[Number(key) - 1] ?? null) : null;
}

/** The tile next or previous to the one heard, going round (the media keys). */
export function stepAudio(ids: string[], audio: string | null, by: 1 | -1): string | null {
  if (!ids.length) return null;
  const at = audio ? ids.indexOf(audio) : -1;
  return ids[(Math.max(at, 0) + (at < 0 ? 0 : by) + ids.length) % ids.length];
}

/** A tile moved earlier or later in the layout (the large tile of three is the first). */
export function moveTile<T extends { id: string }>(tiles: T[], id: string, by: 1 | -1): T[] {
  const from = tiles.findIndex((t) => t.id === id);
  const to = from + by;
  if (from < 0 || to < 0 || to >= tiles.length) return tiles;
  const moved = [...tiles];
  [moved[from], moved[to]] = [moved[to], moved[from]];
  return moved;
}

/**
 * What happens once a tile is taken away, by how many are left: the last
 * one standing goes back to the player (Multiview of one is just the
 * player), and with none left nothing plays.
 */
export function afterRemove(left: number): "stay" | "player" | "stop" {
  return left === 0 ? "stop" : left === 1 ? "player" : "stay";
}

/** The tile that carries on in the player when Multiview closes: the one being heard. */
export function tileToKeep(ids: string[], audio: string | null): string | null {
  return audio && ids.includes(audio) ? audio : (ids[0] ?? null);
}

/** Streams from a source playing in other tiles, for the provider's connection limit. */
export function watchingFrom(tiles: { id: string; sourceId: string }[], sourceId: string, except?: string): number {
  return tiles.filter((t) => t.sourceId === sourceId && t.id !== except).length;
}

/**
 * Whether a tile's stream takes one of its source's connections: a
 * provider's films count as its channels do, a media server's films and
 * episodes don't take its tuners (only its channels do), and a media
 * folder's videos are files on this computer.
 */
export function usesConnection(sourceKind: string | null | undefined, live: boolean): boolean {
  if (sourceKind === "folder") return false;
  if (sourceKind === "plex" || sourceKind === "jellyfin" || sourceKind === "emby") return live;
  return true;
}

// MARK: Pausing and rewinding

/** The live buffer setting: never, from a pause, or always. */
export type LiveBufferSetting = "off" | "paused" | "always";

/**
 * Whether pausing a tile moves it onto a buffer of its own, from then on: a
 * live channel playing directly, unless the live buffer is turned off. Tiles
 * play directly until they're paused, so the buffer's ffmpeg and disk are
 * spent only on paused tiles, whatever the setting is for the player.
 */
export function buffersOnPause(setting: LiveBufferSetting, stream: { isLive: boolean; timeshift?: unknown } | null): boolean {
  return setting !== "off" && !!stream && stream.isLive && !stream.timeshift;
}

/** How far behind live a buffered tile is, to the second: none at the edge, or without a buffer. */
export function secondsBehind(range: { start: number; end: number } | null, current: number): number {
  return range ? Math.max(0, Math.round(range.end - current)) : 0;
}

/** Where a skip of `by` seconds lands in a buffer: within what's kept, short of its very end. */
export function skipInBuffer(range: { start: number; end: number }, current: number, by: number): number {
  return Math.min(Math.max(current + by, range.start), range.end - 1);
}

/** Where a buffered tile carries on from: where it was, unless it was paused for longer than the buffer keeps. */
export function resumeAt(range: { start: number; end: number } | null, current: number): number {
  return range && current < range.start ? range.start + 1 : current;
}

/** Pause All pauses every tile unless they're all paused already, when it plays them all. Tiles that failed don't count. */
export function allPaused(tiles: { paused: boolean; error: string | null }[]): boolean {
  const playing = tiles.filter((t) => !t.error);
  return playing.length > 0 && playing.every((t) => t.paused);
}

// MARK: The inset

/** Swaps the inset and the big picture; in the player, adds an inset. */
export const INSET_KEY = "x";

/** The inset is for two tiles: something on in a corner of what's being watched. */
export function canInset(count: number): boolean {
  return count === 2;
}

/** What the inset needs to know of a tile: a channel plays on, a film or episode can wait. */
export interface InsetTile {
  id: string;
  live: boolean;
  paused: boolean;
  /** Paused by going small, rather than by someone. */
  insetPaused: boolean;
}

export type PauseChange = { id: string; paused: boolean; insetPaused: boolean };

/**
 * What the two do once one is big and the other small (after a swap, or as
 * the inset starts): a film or episode going small pauses, so nothing is
 * missed while it's out of the way, and one the inset paused plays again
 * when it's big. A channel plays on either way, silently when it's small,
 * so you see when the game's back on. A tile someone paused stays paused.
 */
export function insetChanges(big: InsetTile, small: InsetTile): PauseChange[] {
  const changes: PauseChange[] = [];
  if (big.insetPaused) changes.push({ id: big.id, paused: false, insetPaused: false });
  if (!small.live && !small.paused) changes.push({ id: small.id, paused: true, insetPaused: true });
  return changes;
}

/** Hiding the inset: a film or episode playing in it waits until it's shown again, and a channel plays on out of sight. */
export function hideChanges(small: InsetTile): PauseChange[] {
  return !small.live && !small.paused ? [{ id: small.id, paused: true, insetPaused: true }] : [];
}
