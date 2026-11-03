// Multiview: two to four live channels at once, in the main window where the
// player shows (never a window of its own). Each tile plays its own stream,
// a provider connection of its own, and fails on its own; one tile is heard.
// Tiles play directly until they're paused: pausing one moves it onto a live
// buffer of its own from then (unless the live buffer is off), so it can be
// rewound and caught up on, and the buffer's ffmpeg and disk are spent only
// on paused tiles. The buffer takes over the tile's connection rather than
// adding one (the gateway closes the direct stream before the buffer opens),
// and Go Live lets the buffer go and plays the channel directly again.
//
// Opening Multiview takes over the live channel playing: its stream carries
// on in the first tile. Closing it carries on with the tile being heard in
// the player, since that's the one being followed, and lets the others go;
// so does taking tiles away until one is left.

import { create } from "zustand";
import { confirmAction } from "../components/Confirm";
import { channelItem } from "../lib/actions";
import { api, errorText } from "../lib/api";
import { useApp } from "../lib/store";
import type { Channel, ResolvedStream } from "../lib/types";
import { sameChannel } from "../lib/versions";
import { leavePlayerFullscreen } from "./fullscreen";
import {
  afterRemove,
  allPaused,
  buffersOnPause,
  focusAfterAdd,
  focusAfterRemove,
  listenTo,
  MAX_TILES,
  moveTile as moved,
  skipInBuffer,
  stepAudio,
  tileToKeep,
  toggleExpanded,
  watchingFrom,
} from "./multiviewRules";
import { bufferRange } from "./playerRules";

export interface Tile {
  id: string;
  /** Order of creation. Tiles are drawn in it, so moving one never restarts its video. */
  seq: number;
  channel: Channel;
  stream: ResolvedStream | null;
  /** Why it isn't playing: the stream couldn't be had, or stopped. */
  error: string | null;
  /** Each try at a stream. One that arrives after a swap, a retry or removal is let go. */
  attempt: number;
  /** Paused by someone: the tile's own button or keys, or Pause All. */
  paused: boolean;
  /** Moving onto its buffer after a pause; the direct stream ends meanwhile. */
  startingBuffer: boolean;
  /** Its buffer opens where the pause was, at the buffer's start. */
  holdAtStart: boolean;
  /** The buffer couldn't start, and the direct stream went with the try: playing tunes in again. */
  retune: boolean;
  /** Seconds behind live, while it plays from its buffer. */
  behind: number;
}

/** The channel picker: for another tile, or to change one tile's channel. */
export type Picker = { purpose: "add" } | { purpose: "swap"; tileId: string };

interface MultiviewState {
  open: boolean;
  /** In layout order. */
  tiles: Tile[];
  /** The tile heard. */
  audio: string | null;
  /** The tile filling the view. */
  expanded: string | null;
  picker: Picker | null;
}

const closed = { open: false, tiles: [] as Tile[], audio: null, expanded: null };

export const useMultiview = create<MultiviewState>(() => ({ ...closed, picker: null }));

const state = () => useMultiview.getState();
const app = () => useApp.getState();

/** A tile playing live, as it starts and after a swap, retry or Go Live. */
const live = { error: null, paused: false, startingBuffer: false, holdAtStart: false, retune: false, behind: 0 };

let seq = 0;
function newTile(channel: Channel, stream: ResolvedStream | null = null): Tile {
  seq += 1;
  return { id: `tile-${seq}`, seq, channel, stream, attempt: 0, ...live };
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

function release(tile: Tile) {
  if (tile.stream) api.releaseStream(tile.stream.url).catch(() => {});
}

/** Asks for a tile's stream, straight from its source. */
async function load(id: string) {
  const asked = tileById(id);
  if (!asked) return;
  try {
    const stream = await api.play({ kind: "channel", channelId: asked.channel.id }, true);
    if (tileById(id)?.attempt !== asked.attempt) {
      api.releaseStream(stream.url).catch(() => {});
      return;
    }
    update(id, { stream });
  } catch (error) {
    if (tileById(id)?.attempt === asked.attempt) update(id, { error: errorText(error) });
  }
}

/** The tile again from a fresh stream, live, on another channel or the same one (Retry). */
function restart(id: string, channel?: Channel) {
  const tile = tileById(id);
  if (!tile) return;
  release(tile);
  update(id, { channel: channel ?? tile.channel, stream: null, attempt: tile.attempt + 1, ...live });
  load(id);
}

/** The live channel the player has on. Replays and recordings aren't live. */
function playingChannel(): Channel | null {
  const { nowPlaying } = app();
  return nowPlaying?.kind === "channel" && nowPlaying.channel ? nowPlaying.channel : null;
}

const sourcesOf = (tiles: Tile[]) => tiles.map((t) => ({ id: t.id, sourceId: t.channel.sourceId }));

/**
 * Warns, as recording does, before a tile would take the provider past the
 * streams it allows, counting the source's other tiles and the recordings
 * running on it. Going ahead is the person's call: providers' counts aren't
 * always right.
 */
async function roomFor(channel: Channel, others: { id: string; sourceId: string }[], except?: string) {
  const warning = await api.connectionWarning(channel.sourceId, watchingFrom(others, channel.sourceId, except)).catch(() => null);
  if (!warning) return true;
  return confirmAction({
    title: "Over your provider's limit",
    message: `${warning} ${channel.name} may not start, or another stream may stop.`,
    confirmLabel: "Add Anyway",
  });
}

/** Opens Multiview with the live channel on, if there is one, then `channel`. */
function start(channel: Channel | null) {
  const { nowPlaying, stream } = app();
  const playing = playingChannel();
  const tiles: Tile[] = [];
  if (playing) {
    // Its stream carries on in the first tile, unless it's the live buffer: tiles play directly until paused.
    const direct = stream && !stream.timeshift ? stream : null;
    if (stream && !direct) api.releaseStream(stream.url).catch(() => {});
    tiles.push(newTile(playing, direct));
    useApp.setState({ nowPlaying: null, stream: null, presentation: "hidden", playError: null });
  } else if (nowPlaying) app().stop();
  if (channel && !tiles.some((t) => sameChannel(t.channel, channel))) tiles.push(newTile(channel));
  useMultiview.setState({ ...closed, open: true, tiles, audio: tiles[0]?.id ?? null });
  for (const tile of tiles) if (!tile.stream) load(tile.id);
}

/**
 * Closes Multiview. The tile kept carries on in the player, full size, with
 * its stream as it is (a buffer it was paused into included); with the live
 * buffer always on, the player starts it again from a buffer of its own. The
 * others are let go, and their buffers with them.
 */
function leave(keepId: string | null) {
  const { tiles } = state();
  const keep = tiles.find((t) => t.id === keepId) ?? null;
  for (const tile of tiles) if (tile !== keep) release(tile);
  useMultiview.setState({ ...closed });
  if (!keep) return;
  const item = channelItem(keep.channel);
  const { stream } = keep;
  const usable = !keep.error && !keep.retune && !keep.startingBuffer;
  if (stream && usable && (stream.timeshift || app().settings.liveBuffer !== "always")) {
    useApp.setState((s) => ({
      nowPlaying: item,
      stream,
      playError: null,
      presentation: "full",
      channelTrail: [keep.channel, ...s.channelTrail.filter((c) => !sameChannel(c, keep.channel))].slice(0, 20),
    }));
    api.recordChannelWatched(keep.channel.id).catch(() => {});
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

/** "Add to Multiview" on a channel: with a live channel on, Multiview starts with the two. */
export async function addToMultiview(channel: Channel) {
  if (state().open) return addTile(channel);
  const playing = playingChannel();
  const fresh = !playing || !sameChannel(playing, channel);
  if (fresh && !(await roomFor(channel, playing ? [{ id: "player", sourceId: playing.sourceId }] : []))) return;
  start(channel);
}

async function addTile(channel: Channel) {
  const already = () => state().tiles.find((t) => sameChannel(t.channel, channel));
  const there = already();
  if (there) {
    listen(there.id);
    app().showToast(`${channel.name} is in Multiview already`);
    return;
  }
  if (state().tiles.length >= MAX_TILES) {
    app().showToast("Multiview shows up to four channels. Remove one to add another.");
    return;
  }
  if (!(await roomFor(channel, sourcesOf(state().tiles)))) return;
  // Closed, filled or added elsewhere while the warning was up.
  if (!state().open || state().tiles.length >= MAX_TILES || already()) return;
  const tile = newTile(channel);
  useMultiview.setState((s) => ({ tiles: [...s.tiles, tile], ...focusAfterAdd(s, tile.id) }));
  load(tile.id);
}

/** Another channel in a tile, in its place. */
export async function swapTile(id: string, channel: Channel) {
  const tile = tileById(id);
  if (!tile || sameChannel(tile.channel, channel)) return;
  const there = state().tiles.find((t) => t.id !== id && sameChannel(t.channel, channel));
  if (there) {
    listen(there.id);
    app().showToast(`${channel.name} is in Multiview already`);
    return;
  }
  if (!(await roomFor(channel, sourcesOf(state().tiles), id))) return;
  restart(id, channel);
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
  useMultiview.setState((s) => toggleExpanded(s, id));
}

export function moveTile(id: string, by: 1 | -1) {
  useMultiview.setState((s) => ({ tiles: moved(s.tiles, id, by) }));
}

// MARK: Pausing and rewinding

/**
 * Pauses or plays one tile. Pausing a live channel playing directly moves
 * it onto a buffer of its own from now, so it carries on from the pause,
 * behind live; one that couldn't be buffered tunes in again when played.
 */
export function setTilePaused(id: string, paused: boolean) {
  const tile = tileById(id);
  if (!tile || tile.error || tile.paused === paused) return;
  if (!paused && tile.retune) return restart(id);
  update(id, { paused });
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
    app().showToast(`Pause and rewind isn't available on ${tile.channel.name}.`, "error");
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
  if (!tile) return;
  if (!tile.stream?.timeshift) return setTilePaused(id, false);
  const attempt = tile.attempt + 1;
  update(id, { stream: null, attempt, ...live });
  await api.releaseStream(tile.stream.url).catch(() => {});
  if (tileById(id)?.attempt === attempt) load(id);
}

/** Skips `by` seconds back (negative) or forward within a tile's buffer. A tile playing directly has nothing to skip through. */
export function skipTile(id: string, by: number) {
  const tile = tileById(id);
  const video = tileVideo(id);
  const range = tile?.stream?.timeshift && video ? bufferRange(video) : null;
  if (video && range) video.currentTime = skipInBuffer(range, video.currentTime, by);
}

/** How far behind live a tile plays, as its video reports it. */
export function reportBehind(id: string, behind: number) {
  if (tileById(id) && tileById(id)?.behind !== behind) update(id, { behind });
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

/** A channel chosen in the picker. */
export function picked(channel: Channel) {
  const { picker } = state();
  useMultiview.setState({ picker: null });
  if (picker?.purpose === "swap") swapTile(picker.tileId, channel);
  else addToMultiview(channel);
}

/**
 * The player's Multiview button and ⇧⌘M: choose a channel to watch beside
 * the one on (the player carries on until one is chosen). ⇧⌘M in Multiview
 * goes back to the player.
 */
export function toggleMultiview() {
  if (state().open) closeMultiview();
  else choose({ purpose: "add" });
}
