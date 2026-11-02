// The player's actions as plain functions, for its keys, the macOS menu bar
// and anything else outside the player. They work on whichever player is
// showing (the main window's or the pop-out's, each in its own window), and
// do nothing when nothing plays.

import { useApp } from "../lib/store";
import { pauseByPerson } from "./pause";

export { channelDown, channelUp } from "./zap";
export { closeChannelList, toggleChannelList, toggleNowNext } from "./LiveOverlays";
export { reconnectNow, retry } from "./recovery";

/** What only the mounted player can do, since it knows where the title is and how its stream seeks. */
interface PlayerHandlers {
  /** Seconds back (negative) or forward, within what can be seeked. */
  skip(seconds: number): void;
  /** Another copy of an add-on title, from the same moment. */
  switchCopy(): void;
  toggleFullscreen(): void;
}

let video: HTMLVideoElement | null = null;
let handlers: PlayerHandlers | null = null;

/** The player's video element and what it does, while it's mounted. */
export function registerPlayer(element: HTMLVideoElement | null, actions: PlayerHandlers | null) {
  video = element;
  handlers = actions;
}

const playing = () => (useApp.getState().nowPlaying ? video : null);

export function togglePlayPause() {
  const el = playing();
  if (!el) return;
  if (el.paused) el.play().catch(() => {});
  else pauseByPerson(el);
}

/** Plays again after something paused it for a moment (the copies list closing unpicked). */
export function resumePlayback() {
  playing()
    ?.play()
    .catch(() => {});
}

export function pausePlayback() {
  playing()?.pause();
}

export function toggleMute() {
  const el = playing();
  if (el) el.muted = !el.muted;
}

/** Volume up or down by `by` (0.1 is a step), unmuting. */
export function changeVolume(by: number) {
  const el = playing();
  if (!el) return;
  el.volume = Math.min(1, Math.max(0, el.volume + by));
  el.muted = false;
}

export function skip(seconds: number) {
  handlers?.skip(seconds);
}

export function switchCopy() {
  handlers?.switchCopy();
}

export function toggleFullscreen() {
  handlers?.toggleFullscreen();
}

type PipVideo = HTMLVideoElement & { webkitSetPresentationMode?: (mode: "inline" | "picture-in-picture") => void; webkitPresentationMode?: string };

/** Picture in Picture, or back from it. */
export async function togglePictureInPicture() {
  const el = playing() as PipVideo | null;
  if (!el) return;
  try {
    if (document.pictureInPictureElement) await document.exitPictureInPicture();
    else if (el.requestPictureInPicture) await el.requestPictureInPicture();
    else if (el.webkitSetPresentationMode) el.webkitSetPresentationMode(el.webkitPresentationMode === "picture-in-picture" ? "inline" : "picture-in-picture");
  } catch {
    useApp.getState().showToast("Picture in Picture isn't available for this video right now.", "error");
  }
}
