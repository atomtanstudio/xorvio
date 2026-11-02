// Multiview's rules, kept apart from React and the backend so they can be
// checked on their own (`node src/player/multiviewRules.test.ts`): where
// tiles go, which one is heard, and when Multiview gives way to the player.

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
}

/**
 * Two side by side; three as one large tile with two stacked beside it (as
 * the Swift app had them, sized so their edges line up); four in a 2×2
 * grid. An expanded tile fills the view on its own.
 */
export function layoutFor(ids: string[], expanded: string | null): MultiviewLayout {
  const at = (id: string, column: number, row: number, span = 1): TilePlace => ({ id, column, row, span });
  if (expanded && ids.includes(expanded)) return { columns: 1, rows: 1, places: [at(expanded, 1, 1)], hidden: ids.filter((id) => id !== expanded), invite: null };
  const [a, b, c, d] = ids;
  switch (ids.length) {
    case 0:
      return { columns: 1, rows: 1, places: [], hidden: [], invite: { column: 1, row: 1 } };
    case 1:
      return { columns: 2, rows: 1, places: [at(a, 1, 1)], hidden: [], invite: { column: 2, row: 1 } };
    case 2:
      return { columns: 2, rows: 1, places: [at(a, 1, 1), at(b, 2, 1)], hidden: [], invite: null };
    case 3:
      return { columns: 3, rows: 2, places: [at(a, 1, 1, 2), at(b, 3, 1), at(c, 3, 2)], hidden: [], invite: null };
    default:
      return { columns: 2, rows: 2, places: [at(a, 1, 1), at(b, 2, 1), at(c, 1, 2), at(d, 2, 2)], hidden: ids.slice(MAX_TILES), invite: null };
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
