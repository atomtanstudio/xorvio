// Multiview: two to four things at once, in the main window where the player
// shows (never a window of its own). A tile is a live channel or a film or
// episode (from a media folder, a media server or a provider), and each one
// plays its own stream and fails on its own; one tile is heard.
// Live tiles play directly until they're paused: pausing one moves it onto a
// live buffer of its own from then (unless the live buffer is off), so it can
// be rewound and caught up on, and the buffer's ffmpeg and disk are spent
// only on paused tiles. The buffer takes over the tile's connection rather
// than adding one (the gateway closes the direct stream before the buffer
// opens), and Go Live lets the buffer go and plays the channel directly again.
// A film or episode pauses and skips as it does in the player, and keeps its
// place for Continue Watching.
//
// Two tiles can also be an inset: one filling the view and the other small
// in a corner over it, swapped with X. A film or episode going small pauses
// and plays again when it's big; a channel plays on silently in the corner,
// so the game can sit there through a film and be swapped back when it's on.
//
// Opening Multiview takes over what's playing: a live channel's stream
// carries on in the first tile, and a film or episode carries on from where
// it was. Closing it carries on with the tile being heard in the player,
// since that's the one being followed, and lets the others go; so does
// taking tiles away until one is left.

import { create } from "zustand";
import { confirmAction } from "../components/Confirm";
import { channelItem } from "../lib/actions";
import { api, errorText } from "../lib/api";
import { nowSeconds } from "../lib/format";
import { useSources } from "../lib/sources";
import { useApp, type NowPlaying } from "../lib/store";
import type { Channel, ResolvedStream } from "../lib/types";
import { sameChannel } from "../lib/versions";
import { playerPosition, SAVE_PROGRESS_EVENT } from "./controls";
import { leavePlayerFullscreen } from "./fullscreen";
import {
  afterRemove,
  allPaused,
  buffersOnPause,
  canInset,
  focusAfterAdd,
  focusAfterRemove,
  hideChanges,
  insetChanges,
  listenTo,
  MAX_TILES,
  moveTile as moved,
  skipInBuffer,
  stepAudio,
  tileToKeep,
  toggleExpanded,
  usesConnection,
  watchingFrom,
  type InsetTile,
  type PauseChange,
} from "./multiviewRules";
import { bufferRange, watchedEnough } from "./playerRules";

export interface Tile {
  id: string;
  /** Order of creation. Tiles are drawn in it, so moving one never restarts its video. */
  seq: number;
  /** A live channel (with its `channel`), or a film, episode, replay or recording. */
  item: NowPlaying;
  stream: ResolvedStream | null;
  /** Why it isn't playing: the stream couldn't be had, or stopped. */
  error: string | null;
  /** Each try at a stream. One that arrives after a swap, a retry or removal is let go. */
  attempt: number;
  /** Paused by someone (the tile's own button or keys, or Pause All), or by the inset. */
  paused: boolean;
  /** Paused by going small in the inset, so it plays again when it's big. */
  insetPaused: boolean;
  /** Moving onto its buffer after a pause; the direct stream ends meanwhile. */
  startingBuffer: boolean;
  /** Its buffer opens where the pause was, at the buffer's start. */
  holdAtStart: boolean;
  /** The buffer couldn't start, and the direct stream went with the try: playing tunes in again. */
  retune: boolean;
  /** Seconds behind live, while it plays from its buffer. */
  behind: number;
  /** A film or episode: seconds in and how long it is, as its video last said (each second or so). */
  position: number;
  duration: number;
  /** A film or episode that played to its end. */
  ended: boolean;
}

/** The picker: for another tile, for a tile's place, or for the inset over what's playing. */
export type Picker = { purpose: "add" } | { purpose: "swap"; tileId: string } | { purpose: "inset" };

interface MultiviewState {
  open: boolean;
  /** In layout order: with the inset on, the first fills the view and the second is the inset. */
  tiles: Tile[];
  /** The tile heard. */
  audio: string | null;
  /** The tile filling the view. */
  expanded: string | null;
  /** Two tiles as one filling the view and the other small in a corner. */
  inset: boolean;
  /** The inset is out of sight for now. */
  insetHidden: boolean;
  picker: Picker | null;
}

const closed = { open: false, tiles: [] as Tile[], audio: null, expanded: null, inset: false, insetHidden: false };

export const useMultiview = create<MultiviewState>(() => ({ ...closed, picker: null }));

const state = () => useMultiview.getState();
const app = () => useApp.getState();

/** A live channel (rather than a film, an episode, or a replay of what was on). */
export const isLiveItem = (item: NowPlaying) => item.kind === "channel" && !!item.channel;

/** The live channel a tile plays; null for a film, episode, replay or recording. */
export const tileChannel = (tile: Tile): Channel | null => (isLiveItem(tile.item) ? (tile.item.channel ?? null) : null);

/** The same thing to watch: versions of one channel are one channel. */
export function sameItem(a: NowPlaying, b: NowPlaying) {
  if (isLiveItem(a) && isLiveItem(b)) return sameChannel(a.channel!, b.channel!);
  return !isLiveItem(a) && !isLiveItem(b) && a.itemId === b.itemId;
}

/** A tile starting, as it does and after a swap, retry or Go Live: playing, live, from the start of its stream. */
const fresh = { error: null, paused: false, insetPaused: false, startingBuffer: false, holdAtStart: false, retune: false, behind: 0, ended: false };

let seq = 0;
function newTile(item: NowPlaying, stream: ResolvedStream | null = null): Tile {
  seq += 1;
  return { id: `tile-${seq}`, seq, item, stream, attempt: 0, position: item.resumeFrom ?? 0, duration: item.durationHint ?? 0, ...fresh };
}

/** The tiles' video elements, for skipping within their buffers. */
const videos = new Map<string, HTMLVideoElement>();

export function registerTileVideo(id: string, video: HTMLVideoElement | null) {
  if (video) videos.set(id, video);
  else videos.delete(id);
}

export const tileVideo = (id: string | null) => (id ? (videos.get(id) ?? null) : null);

const tileById = (id: string) => state().tiles.find((t) => t.id === id);

function update(id: string, change: Partial<Tile>) {
  useMultiview.setState((s) => ({ tiles: s.tiles.map((t) => (t.id === id ? { ...t, ...change } : t)) }));
}

/** Where a film or episode is, for Continue Watching. Live tiles have nothing to keep. */
function saveProgress(tile: Tile, finished = false) {
  const { item, position, duration } = tile;
  if ((item.kind !== "movie" && item.kind !== "episode" && item.kind !== "recording") || !duration || tile.error) return;
  api
    .saveProgress({ itemId: item.itemId, kind: item.kind, sourceId: item.sourceId, seriesId: item.seriesId ?? null, positionSeconds: position, durationSeconds: duration, finished: finished || watchedEnough(position, duration), updatedAt: nowSeconds() })
    .catch(() => {});
}

function release(tile: Tile) {
  saveProgress(tile);
  if (tile.stream) api.releaseStream(tile.stream.url).catch(() => {});
}

/** Asks for a tile's stream, straight from its source. */
async function load(id: string) {
  const asked = tileById(id);
  if (!asked) return;
  try {
    const stream = await api.play(asked.item.request, true);
    if (tileById(id)?.attempt !== asked.attempt) {
      api.releaseStream(stream.url).catch(() => {});
      return;
    }
    update(id, { stream });
    const { item } = asked;
    // In the history, as the player puts what it plays there.
    if (isLiveItem(item)) api.recordChannelWatched(item.itemId).catch(() => {});
    else if (item.kind === "movie" || item.kind === "episode") api.recordWatched(item.itemId, item.kind, item.sourceId, item.title, item.subtitle ?? null).catch(() => {});
  } catch (error) {
    if (tileById(id)?.attempt === asked.attempt) update(id, { error: errorText(error) });
  }
}

/** The tile again from a fresh stream: another item, or the same one (Retry, from where it was). */
function restart(id: string, item?: NowPlaying) {
  const tile = tileById(id);
  if (!tile) return;
  release(tile);
  const next = item ?? (isLiveItem(tile.item) ? tile.item : { ...tile.item, resumeFrom: tile.position });
  update(id, { item: next, stream: null, attempt: tile.attempt + 1, position: next.resumeFrom ?? 0, duration: next.durationHint ?? 0, ...fresh });
  load(id);
}

/** What the player has on that Multiview can carry on with: a live channel, or a film or episode (from where it is). */
function playingItem(): NowPlaying | null {
  const { nowPlaying } = app();
  if (!nowPlaying) return null;
  if (isLiveItem(nowPlaying)) return nowPlaying;
  // A replay's channel is in the guide, not live: it carries on as a title.
  return { ...nowPlaying, resumeFrom: playerPosition() || nowPlaying.resumeFrom };
}

const sourceKind = (sourceId: string) => useSources.getState().sources?.find((s) => s.config.id === sourceId)?.config.kind ?? null;

/** The tiles whose streams take one of their source's connections. */
const connected = (tiles: { id: string; item: NowPlaying }[]) =>
  tiles.filter((t) => usesConnection(sourceKind(t.item.sourceId), isLiveItem(t.item))).map((t) => ({ id: t.id, sourceId: t.item.sourceId }));

/**
 * Warns, as recording does, before a tile would take the provider past the
 * streams it allows, counting the source's other tiles and the recordings
 * running on it. A media server's films and a media folder's videos take
 * nothing. Going ahead is the person's call: providers' counts aren't always
 * right.
 */
async function roomFor(item: NowPlaying, others: { id: string; item: NowPlaying }[], except?: string) {
  if (!usesConnection(sourceKind(item.sourceId), isLiveItem(item))) return true;
  const warning = await api.connectionWarning(item.sourceId, watchingFrom(connected(others), item.sourceId, except)).catch(() => null);
  if (!warning) return true;
  return confirmAction({
    title: "Over your provider's limit",
    message: `${warning} ${item.title} may not start, or another stream may stop.`,
    confirmLabel: "Add Anyway",
  });
}

/** What the player has on, as the first tile: a live channel keeps its stream, a film or episode starts again where it was. */
function takeFromPlayer(): Tile | null {
  const { nowPlaying, stream } = app();
  const playing = playingItem();
  if (!nowPlaying || !playing) return null;
  // A channel's stream carries on in the tile, unless it's the live buffer: tiles play directly until paused.
  const keep = isLiveItem(playing) && stream && !stream.timeshift ? stream : null;
  // The player's own saving stops with it; the tile keeps the place from here.
  window.dispatchEvent(new Event(SAVE_PROGRESS_EVENT));
  if (stream && !keep) api.releaseStream(stream.url).catch(() => {});
  useApp.setState({ nowPlaying: null, stream: null, presentation: "hidden", playError: null });
  return newTile(playing, keep);
}

/** Opens Multiview with what's playing, if anything is, then `item`; as an inset over what's playing, with `inset`. */
function start(item: NowPlaying | null, inset = false) {
  const first = takeFromPlayer();
  const tiles = first ? [first] : [];
  if (item && !tiles.some((t) => sameItem(t.item, item))) tiles.push(newTile(item));
  const asInset = inset && canInset(tiles.length);
  useMultiview.setState({ ...closed, open: true, tiles, audio: tiles[0]?.id ?? null, inset: asInset });
  if (asInset) settleInset();
  for (const tile of tiles) if (!tile.stream) load(tile.id);
}

/**
 * Closes Multiview. The tile kept carries on in the player, full size: a
 * live channel with its stream as it is (a buffer it was paused into
 * included; with the live buffer always on, the player starts it again from
 * a buffer of its own), a film or episode from where it was. The others are
 * let go, and their buffers with them.
 */
function leave(keepId: string | null) {
  const { tiles } = state();
  const keep = tiles.find((t) => t.id === keepId) ?? null;
  for (const tile of tiles) if (tile !== keep) release(tile);
  useMultiview.setState({ ...closed });
  if (!keep) return;
  const channel = tileChannel(keep);
  if (!channel) {
    release(keep);
    app().play({ ...keep.item, resumeFrom: keep.ended ? 0 : keep.position }, "full");
    return;
  }
  const item = channelItem(channel);
  const { stream } = keep;
  const usable = !keep.error && !keep.retune && !keep.startingBuffer;
  if (stream && usable && (stream.timeshift || app().settings.liveBuffer !== "always")) {
    useApp.setState((s) => ({
      nowPlaying: item,
      stream,
      playError: null,
      presentation: "full",
      channelTrail: [channel, ...s.channelTrail.filter((c) => !sameChannel(c, channel))].slice(0, 20),
    }));
    api.recordChannelWatched(channel.id).catch(() => {});
  } else {
    release(keep);
    app().play(item, "full");
  }
}

// Anything else starting to play (a reminder clicked, the menu bar) takes the
// window: Multiview lets its tiles go.
useApp.subscribe((s, before) => {
  if (s.nowPlaying && s.nowPlaying !== before.nowPlaying && state().open) leave(null);
});

/** "Add to Multiview" on a channel: with something on, Multiview starts with the two. */
export async function addToMultiview(channel: Channel) {
  return addItem(channelItem(channel));
}

/** A channel, film or episode in Multiview: as another tile, or with what's playing. */
async function addItem(item: NowPlaying) {
  if (state().open) return addTile(item);
  if (!(await roomBesidePlayer(item))) return;
  start(item);
}

/** Room for `item` beside what the player has on, which carries on in Multiview. */
async function roomBesidePlayer(item: NowPlaying) {
  const playing = playingItem();
  if (playing && sameItem(playing, item)) return true;
  return roomFor(item, playing ? [{ id: "player", item: playing }] : []);
}

async function addTile(item: NowPlaying) {
  const already = () => state().tiles.find((t) => sameItem(t.item, item));
  const there = already();
  if (there) {
    listen(there.id);
    app().showToast(`${item.title} is in Multiview already`);
    return;
  }
  if (state().tiles.length >= MAX_TILES) {
    app().showToast("Multiview shows up to four at once. Remove one to add another.");
    return;
  }
  if (!(await roomFor(item, state().tiles))) return;
  // Closed, filled or added elsewhere while the warning was up.
  if (!state().open || state().tiles.length >= MAX_TILES || already()) return;
  const tile = newTile(item);
  // A third tile shows them all side by side: the inset is for two.
  useMultiview.setState((s) => ({ tiles: [...s.tiles, tile], ...focusAfterAdd(s, tile.id), inset: false, insetHidden: false }));
  load(tile.id);
}

/** Something else in a tile, in its place. */
export async function swapTile(id: string, item: NowPlaying) {
  const tile = tileById(id);
  if (!tile || sameItem(tile.item, item)) return;
  const there = state().tiles.find((t) => t.id !== id && sameItem(t.item, item));
  if (there) {
    listen(there.id);
    app().showToast(`${item.title} is in Multiview already`);
    return;
  }
  if (!(await roomFor(item, state().tiles, id))) return;
  restart(id, item);
  if (state().inset) settleInset();
}

export function retryTile(id: string) {
  restart(id);
}

/**
 * The player's error for a tile's stream, unless it's been restarted since,
 * or the stream ended because the tile is moving onto its buffer.
 */
export function failTile(id: string, attempt: number, message: string) {
  const tile = tileById(id);
  if (tile?.attempt === attempt && !tile.startingBuffer && !tile.retune) update(id, { error: message });
}

export function removeTile(id: string) {
  const { tiles } = state();
  const tile = tileById(id);
  if (!tile) return;
  const left = tiles.filter((t) => t.id !== id);
  switch (afterRemove(left.length)) {
    case "stop":
      leave(null);
      // Nothing carries on, so a full screen the player or Multiview made ends too.
      leavePlayerFullscreen();
      return;
    case "player":
      return leave(left[0].id);
    case "stay":
      release(tile);
      useMultiview.setState((s) => ({ tiles: left, ...focusAfterRemove(s.tiles.map((t) => t.id), s, id) }));
  }
}

/** Back to the player with the tile being heard. */
export function closeMultiview() {
  const { tiles, audio } = state();
  leave(tileToKeep(tiles.map((t) => t.id), audio));
}

/** One tile on its own in the player, the rest let go. */
export function watchInPlayer(id: string) {
  leave(id);
}

export function listen(id: string) {
  useMultiview.setState((s) => listenTo(s, id));
}

/** The next or previous tile heard (the media keys). */
export function listenNext(by: 1 | -1) {
  const { tiles, audio } = state();
  const next = stepAudio(tiles.map((t) => t.id), audio, by);
  if (next) listen(next);
}

export function toggleExpand(id: string) {
  if (state().inset) return;
  useMultiview.setState((s) => toggleExpanded(s, id));
}

export function moveTile(id: string, by: 1 | -1) {
  useMultiview.setState((s) => ({ tiles: moved(s.tiles, id, by) }));
}

// MARK: The inset

const insetTile = (tile: Tile): InsetTile => ({ id: tile.id, live: !!tileChannel(tile), paused: tile.paused, insetPaused: tile.insetPaused });

function applyPauses(changes: PauseChange[]) {
  for (const { id, paused, insetPaused } of changes) {
    const tile = tileById(id);
    if (!tile) continue;
    if (paused && !tile.paused) saveProgress(tile);
    update(id, { paused, insetPaused });
  }
}

/** The big one heard and playing, the small one as the inset leaves it (insetChanges). */
function settleInset() {
  const [big, small] = state().tiles;
  if (!big || !small) return;
  useMultiview.setState({ audio: big.id, expanded: null });
  applyPauses(insetChanges(insetTile(big), insetTile(small)));
}

/** The two side by side, or as an inset: the one heard fills the view, the other goes small. */
export function toggleInset() {
  const { tiles, inset, audio } = state();
  if (inset) return useMultiview.setState({ inset: false, insetHidden: false });
  if (!canInset(tiles.length)) return;
  const ordered = tiles[1]?.id === audio ? [tiles[1], tiles[0]] : tiles;
  useMultiview.setState({ tiles: ordered, inset: true, insetHidden: false });
  settleInset();
}

/**
 * X: the big picture and the inset change places, and the sound goes with
 * the big one. A film or episode going small pauses; one the inset paused
 * plays again when it's big. A hidden inset shows again.
 */
export function swapInset() {
  const { tiles, inset } = state();
  if (!inset || tiles.length !== 2) return;
  useMultiview.setState({ tiles: [tiles[1], tiles[0]], insetHidden: false });
  settleInset();
}

/** The inset out of sight, or back: a film or episode in it waits meanwhile, and a channel plays on. */
export function toggleInsetHidden() {
  const { tiles, inset, insetHidden, audio } = state();
  const [big, small] = tiles;
  if (!inset || !small) return;
  if (insetHidden) return useMultiview.setState({ insetHidden: false });
  applyPauses(hideChanges(insetTile(small)));
  // What's out of sight isn't heard.
  useMultiview.setState({ insetHidden: true, audio: audio === small.id ? big.id : audio });
}

/** The player's Add an Inset and X: choose something to have in a corner of what's playing. In Multiview, X swaps. */
export function insetCommand() {
  const { open, tiles, inset } = state();
  if (!open) return choose({ purpose: "inset" });
  if (inset) return swapInset();
  if (canInset(tiles.length)) toggleInset();
}

async function startInset(item: NowPlaying) {
  if (!(await roomBesidePlayer(item))) return;
  start(item, true);
}

// MARK: Pausing and rewinding

/**
 * Pauses or plays one tile. Pausing a live channel playing directly moves
 * it onto a buffer of its own from now, so it carries on from the pause,
 * behind live; one that couldn't be buffered tunes in again when played.
 * A film or episode pauses as it does in the player, and keeps its place.
 */
export function setTilePaused(id: string, paused: boolean) {
  const tile = tileById(id);
  if (!tile || tile.error || (tile.paused === paused && !tile.insetPaused)) return;
  if (!paused && tile.retune) return restart(id);
  if (paused) saveProgress(tile);
  // Played or paused by someone now, so the inset leaves it be.
  update(id, { paused, insetPaused: false, ended: paused && tile.ended });
  // Played again at its end: from the start.
  if (!paused && tile.ended) seekTile(id, 0);
  if (paused && buffersOnPause(app().settings.liveBuffer, tile.stream)) startBuffer(id);
}

/** Moves a paused tile's direct stream onto a buffer that starts now. */
async function startBuffer(id: string) {
  const tile = tileById(id);
  const from = tile?.stream;
  if (!tile || !from || tile.startingBuffer) return;
  update(id, { startingBuffer: true });
  try {
    const buffered = await api.startTimeshift(from.url);
    // Swapped, retried, gone live or removed meanwhile: that buffer isn't wanted.
    if (tileById(id)?.stream !== from) {
      api.releaseStream(buffered.url).catch(() => {});
      return;
    }
    update(id, { stream: buffered, startingBuffer: false, holdAtStart: true });
  } catch (error) {
    api.log("warn", `a Multiview tile's buffer couldn't start: ${errorText(error)}`).catch(() => {});
    if (tileById(id)?.stream !== from) return;
    update(id, { startingBuffer: false, retune: true });
    app().showToast(`Pause and rewind isn't available on ${tile.item.title}.`, "error");
  }
}

/** Pauses every tile, or plays them all when they're all paused. */
export function toggleAllPaused() {
  const { tiles } = state();
  const paused = !allPaused(tiles);
  for (const tile of tiles) setTilePaused(tile.id, paused);
}

/**
 * Back to live: a tile on a buffer lets it go and plays its channel
 * directly again, so the buffer's ffmpeg and disk go with it. The buffer's
 * connection closes before the channel's own opens.
 */
export async function goLiveTile(id: string) {
  const tile = tileById(id);
  if (!tile || !tileChannel(tile)) return;
  if (!tile.stream?.timeshift) return setTilePaused(id, false);
  const attempt = tile.attempt + 1;
  update(id, { stream: null, attempt, ...fresh });
  await api.releaseStream(tile.stream.url).catch(() => {});
  if (tileById(id)?.attempt === attempt) load(id);
}

/**
 * Skips `by` seconds back (negative) or forward: within a live tile's
 * buffer (one playing directly has nothing to skip through), or through a
 * film or episode, as the player seeks it.
 */
export function skipTile(id: string, by: number) {
  const tile = tileById(id);
  const video = tileVideo(id);
  if (!tile || !video) return;
  if (!tileChannel(tile)) return seekTile(id, tile.position + by);
  const range = tile.stream?.timeshift ? bufferRange(video) : null;
  if (range) video.currentTime = skipInBuffer(range, video.currentTime, by);
}

/** A film or episode to `target` seconds in: in its video, or, for one ffmpeg remuxes, from a stream starting there. */
export async function seekTile(id: string, target: number) {
  const tile = tileById(id);
  const video = tileVideo(id);
  const stream = tile?.stream;
  if (!tile || !video || !stream || tileChannel(tile)) return;
  const to = Math.max(0, tile.duration ? Math.min(tile.duration - 2, target) : target);
  update(id, { position: to, ended: false });
  if (stream.mode !== "remux") {
    video.currentTime = to - stream.startOffsetSeconds;
    return;
  }
  try {
    const next = await api.seekStream(stream.url, to);
    if (tileById(id)?.stream === stream) update(id, { stream: next });
  } catch (error) {
    // It plays on where it was.
    app().showToast(errorText(error), "error");
  }
}

/** How far behind live a tile plays, as its video reports it. */
export function reportBehind(id: string, behind: number) {
  if (tileById(id) && tileById(id)?.behind !== behind) update(id, { behind });
}

/** Where a film or episode is, as its video reports it, each second or so. Saved for Continue Watching every ten seconds of playing. */
export function reportPosition(id: string, position: number, duration: number) {
  const tile = tileById(id);
  if (!tile || (Math.abs(tile.position - position) < 0.5 && tile.duration === duration)) return;
  update(id, { position, duration });
  if (Math.floor(position / 10) !== Math.floor(tile.position / 10)) saveProgress({ ...tile, position, duration });
}

/** A film or episode at its end: kept as watched, and paused there. */
export function endTile(id: string) {
  const tile = tileById(id);
  if (!tile) return;
  saveProgress(tile, true);
  update(id, { ended: true, paused: true, insetPaused: false });
}

/** The tile heard, which the keys and the menu bar's Playback items act on. */
export function heardTile() {
  const { tiles, audio } = state();
  return tiles.find((t) => t.id === audio) ?? null;
}

export function toggleHeardPaused() {
  const tile = heardTile();
  if (tile) setTilePaused(tile.id, !tile.paused);
}

export function choose(picker: Picker | null) {
  useMultiview.setState({ picker });
}

/** A channel, film or episode chosen in the picker. */
export function picked(item: NowPlaying) {
  const { picker } = state();
  useMultiview.setState({ picker: null });
  if (picker?.purpose === "swap") swapTile(picker.tileId, item);
  else if (picker?.purpose === "inset") startInset(item);
  else addItem(item);
}

/**
 * The player's Multiview button and ⇧⌘M: choose something to watch beside
 * what's on (the player carries on until it's chosen). ⇧⌘M in Multiview
 * goes back to the player.
 */
export function toggleMultiview() {
  if (state().open) closeMultiview();
  else choose({ purpose: "add" });
}
