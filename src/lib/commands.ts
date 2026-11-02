// The menu bar's commands (src-tauri/src/menu.rs builds the menus). Each
// item, clicked or by its shortcut, arrives as its id, and is performed here
// by whichever part of the interface handles it now: the app shell for
// going places, the player for Playback. In the browser preview there's no
// menu bar, so the same shortcuts are caught here instead.
//
// On a Mac the page sees a shortcut before the menu does, and the menu acts
// only on one the page lets pass. So key handlers elsewhere leave the menu's
// shortcuts alone (`isMenuShortcut`), and each acts once, through the menu;
// the few the web view would scroll with (⌘-arrows, ⌥Space) are done here,
// outside text fields, which keep them. On Windows and Linux the menu takes
// its shortcuts before the page sees them.

import { create } from "zustand";
import { api, isTauri } from "./api";
import { isMac } from "./platform";
import type { MenuState } from "./types";

export type Command =
  | "app.settings"
  | "file.addSource"
  | "file.refreshAll"
  | "view.sidebar"
  | "go.home"
  | "go.watchlist"
  | "go.live"
  | "go.movies"
  | "go.series"
  | "go.recordings"
  | "go.sources"
  | "go.settings"
  | "go.search"
  | "go.back"
  | "playback.playPause"
  | "playback.stop"
  | "playback.skipBack"
  | "playback.skipForward"
  | "playback.goLive"
  | "playback.previousChannel"
  | "playback.nextChannel"
  | "playback.lastChannel"
  | "playback.mute"
  | "playback.multiview"
  | "playback.popOut"
  /** Not in the menu: the player's own way back, which Back uses while it fills the window. */
  | "player.back";


interface Shortcut {
  command: Command;
  /** The character, matched with e.key… */
  key: string;
  /** …or the physical key, for Option combinations (⌥P types "π" on a Mac) and named keys. */
  code: string;
  /** ⌘ on a Mac, Ctrl elsewhere. */
  primary?: boolean;
  shift?: boolean;
  alt?: boolean;
  /** Control as well as ⌘, on a Mac. */
  control?: boolean;
  /** Text fields keep it for moving the cursor (⌘←), so it's left to them there. */
  editing?: boolean;
}

const go = (n: number, command: Command): Shortcut => ({ command, key: String(n), code: `Digit${n}`, primary: true });

/** The menu's shortcuts, as menu.rs gives them: a Mac's and everyone else's. */
const SHORTCUTS: Shortcut[] = isMac
  ? [
      { command: "app.settings", key: ",", code: "Comma", primary: true },
      { command: "file.addSource", key: "n", code: "KeyN", primary: true },
      { command: "file.refreshAll", key: "r", code: "KeyR", primary: true, shift: true },
      { command: "view.sidebar", key: "s", code: "KeyS", primary: true, control: true },
      go(1, "go.home"),
      go(2, "go.watchlist"),
      go(3, "go.live"),
      go(4, "go.movies"),
      go(5, "go.series"),
      go(6, "go.recordings"),
      go(7, "go.sources"),
      { command: "go.search", key: "f", code: "KeyF", primary: true },
      { command: "go.back", key: "[", code: "BracketLeft", primary: true },
      { command: "playback.playPause", key: " ", code: "Space", alt: true, editing: true },
      { command: "playback.skipBack", key: "arrowleft", code: "ArrowLeft", primary: true, editing: true },
      { command: "playback.skipForward", key: "arrowright", code: "ArrowRight", primary: true, editing: true },
      { command: "playback.goLive", key: "l", code: "KeyL", primary: true, shift: true },
      { command: "playback.previousChannel", key: "arrowdown", code: "ArrowDown", primary: true, editing: true },
      { command: "playback.nextChannel", key: "arrowup", code: "ArrowUp", primary: true, editing: true },
      { command: "playback.lastChannel", key: "l", code: "KeyL", primary: true },
      { command: "playback.multiview", key: "m", code: "KeyM", primary: true, shift: true },
      { command: "playback.popOut", key: "p", code: "KeyP", primary: true, alt: true },
    ]
  : [
      { command: "app.settings", key: ",", code: "Comma", primary: true },
      { command: "file.addSource", key: "n", code: "KeyN", primary: true },
      { command: "file.refreshAll", key: "r", code: "KeyR", primary: true, shift: true },
      { command: "view.sidebar", key: "s", code: "KeyS", primary: true, alt: true },
      go(1, "go.home"),
      go(2, "go.watchlist"),
      go(3, "go.live"),
      go(4, "go.movies"),
      go(5, "go.series"),
      go(6, "go.recordings"),
      go(7, "go.sources"),
      { command: "go.search", key: "f", code: "KeyF", primary: true },
      { command: "go.back", key: "arrowleft", code: "ArrowLeft", alt: true, editing: true },
      { command: "playback.goLive", key: "l", code: "KeyL", primary: true, shift: true },
      { command: "playback.lastChannel", key: "l", code: "KeyL", primary: true },
      { command: "playback.multiview", key: "m", code: "KeyM", primary: true, shift: true },
      { command: "playback.popOut", key: "p", code: "KeyP", primary: true, alt: true },
    ];

function shortcutFor(e: KeyboardEvent) {
  const primary = isMac ? e.metaKey : e.ctrlKey;
  const control = isMac && e.ctrlKey;
  return SHORTCUTS.find(
    (s) =>
      !!s.primary === primary &&
      !!s.shift === e.shiftKey &&
      !!s.alt === e.altKey &&
      !!s.control === control &&
      (isMac || !e.metaKey) &&
      (e.key.toLowerCase() === s.key || e.code === s.code),
  );
}

/** One of the menu's shortcuts, which key handlers leave to the menu. */
export function isMenuShortcut(e: KeyboardEvent) {
  return !!shortcutFor(e);
}

const KEY_NAMES: Record<string, string> = { " ": "Space", arrowleft: "←", arrowright: "→", arrowup: "↑", arrowdown: "↓" };

/** A command's shortcut as the menu shows it, for tooltips: ⌥⌘P on a Mac, Ctrl+Alt+P elsewhere. */
export function shortcutLabel(command: Command) {
  const s = SHORTCUTS.find((s) => s.command === command);
  if (!s) return "";
  const key = KEY_NAMES[s.key] ?? s.key.toUpperCase();
  if (isMac) return `${s.control ? "⌃" : ""}${s.alt ? "⌥" : ""}${s.shift ? "⇧" : ""}${s.primary ? "⌘" : ""}${key}`;
  return [s.primary && "Ctrl", s.alt && "Alt", s.shift && "Shift", key].filter(Boolean).join("+");
}

const typingIn = (target: EventTarget | null) =>
  target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement || (target instanceof HTMLElement && target.isContentEditable);

// MARK: Performing

const handlers = new Map<Command, () => void>();

/**
 * Performs a command while the caller is there to: the last one to take a
 * command performs it. Returns the way to let it go.
 */
export function handleCommands(commands: Partial<Record<Command, () => void>>) {
  const taken = Object.entries(commands) as [Command, () => void][];
  for (const [command, perform] of taken) handlers.set(command, perform);
  return () => {
    for (const [command, perform] of taken) if (handlers.get(command) === perform) handlers.delete(command);
  };
}

/** Performs a command, if anything handles it now. */
export function perform(command: Command) {
  handlers.get(command)?.();
}

export function canPerform(command: Command) {
  return handlers.has(command);
}

/**
 * Listens for the menu bar's items (the desktop app) or catches their
 * shortcuts (the browser preview), for as long as the window is open.
 */
export function followMenu() {
  const offMenu = isTauri ? api.onMenu((id) => perform(id as Command)) : () => {};
  const onKey = (e: KeyboardEvent) => {
    const shortcut = shortcutFor(e);
    if (!shortcut || e.defaultPrevented || (shortcut.editing && typingIn(e.target))) return;
    // In the app, the menu has its shortcuts, except the arrows and ⌥Space:
    // the web view would take those for scrolling before the menu saw them,
    // so they're done here (and so not by the menu as well).
    if (isTauri && !shortcut.editing) return;
    e.preventDefault();
    perform(shortcut.command);
  };
  window.addEventListener("keydown", onKey);
  return () => {
    offMenu();
    window.removeEventListener("keydown", onKey);
  };
}

// MARK: What the menu shows

/** What the player says about what's playing, for the Playback menu. */
export interface PlaybackReport {
  playing: boolean;
  paused: boolean;
  muted: boolean;
  seekable: boolean;
  behindLive: boolean;
  channel: boolean;
}

export const usePlaybackReport = create<PlaybackReport>(() => ({ playing: false, paused: false, muted: false, seekable: false, behindLive: false, channel: false }));

/** The player's report, kept only when something in it changed. */
export function reportPlayback(report: PlaybackReport) {
  const now = usePlaybackReport.getState();
  if ((Object.keys(report) as (keyof PlaybackReport)[]).some((key) => now[key] !== report[key])) usePlaybackReport.setState(report);
}

let pending: MenuState | null = null;
let last = "";

/**
 * Tells the menu bar what can act now. Sent once things settle and only
 * when it changed, and again when the window comes to the front (each
 * window says what's true of itself).
 */
export function sendMenuState(state: MenuState) {
  if (!isTauri) return;
  const first = pending === null;
  pending = state;
  if (!first) return;
  queueMicrotask(() => {
    const json = JSON.stringify(pending);
    const sending = pending!;
    pending = null;
    if (json === last) return;
    last = json;
    api.setMenuState(sending).catch(() => {});
  });
}

/**
 * The next state is sent even when it's what was sent before: the menu bar
 * listened to the pop-out window meanwhile, and now listens here again.
 */
export function resendMenuState() {
  last = "";
}

/** Sends `compute()` now, whenever `subscribe` says something changed, and when the window is focused. */
export function followMenuState(compute: () => MenuState, subscribe: (changed: () => void) => () => void) {
  const send = () => sendMenuState(compute());
  const onFocus = () => {
    // The other window may have spoken meanwhile.
    resendMenuState();
    send();
  };
  send();
  const off = subscribe(send);
  window.addEventListener("focus", onFocus);
  return () => {
    off();
    window.removeEventListener("focus", onFocus);
  };
}
