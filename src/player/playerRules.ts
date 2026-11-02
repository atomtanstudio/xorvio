// The player's rules, kept apart from React and the backend so they can be
// checked on their own (`node src/player/playerRules.test.ts`): when a
// stream that dropped is opened again and what to say when it can't be,
// channel numbers typed on the keyboard, how long zapping waits before a
// stream opens, the corner the mini player goes to, and audio and subtitles
// remembered for a series.

import { isMac } from "../lib/platform.ts";
import type { TrackChoice } from "../lib/types.ts";

// MARK: Failures

/** Why something couldn't play, by what can be done about it. */
export type FailureKind =
  /** The connection to the stream went (or the provider's server is down). */
  | "network"
  /** This computer has no connection at all. */
  | "offline"
  /** The provider turned the login down, or the account can't watch now. */
  | "signIn"
  /** The provider doesn't have the stream (any more). */
  | "notFound"
  /** The stream is in a form Xorvio can't play. */
  | "unsupported"
  /** The stream ended, or stopped arriving, with no error. */
  | "ended"
  /** Nothing arrived in the time allowed. */
  | "stalled"
  | "unknown";

export interface Failure {
  kind: FailureKind;
  title: string;
  message: string;
  suggestion: string;
}

/** What went wrong, as the player saw it. */
export interface Problem {
  /** The engine's or the backend's own words. */
  message?: string | null;
  /** The HTTP status the provider (or the gateway) answered with, when it's known. */
  status?: number | null;
  /** A <video> element's error code (MediaError: 2 network, 3 decode, 4 not supported). */
  mediaError?: number | null;
  /** The stream ended without an error: a live channel, or a film well before its end. */
  ended?: boolean;
  /** Nothing played (or nothing more arrived) in the time allowed. */
  stalled?: boolean;
}

export interface FailureContext {
  live: boolean;
  /** The browser's own idea of whether there's a connection. */
  online: boolean;
  /** This title played at some point, so the stream itself is fine. */
  hasPlayed: boolean;
  /** How the gateway serves it ("mpegts", "hls"…), for the suggestion. */
  mode?: string | null;
}

const SIGN_IN = /\b40[17]\b|forbidden|unauthori[sz]ed|access denied|username or password|password|credentials|has expired|signed xorvio out|is missing .*edit the source/i;
const NOT_FOUND = /\b404\b|\b410\b|not found|no longer (in your sources|offers|available)|is gone|no such/i;
const UNSUPPORTED = /not supported|unsupported|can't play|couldn't be decoded|formatunsupported|codecunsupported|needs ffmpeg/i;
const NETWORK = /network|connection|couldn't be reached|timed? ?out|timeout|earlyeof|httpstatuscodeinvalid/i;

/** The words, unless they're an engine's code ("NetworkError: Exception"), which mean nothing to a person. */
function readable(text: string) {
  return text && !/^\w+: \w+$/.test(text.trim()) ? text : null;
}

/**
 * Puts a problem into words, by kind: the gateway's HTTP status first
 * (it's what the provider said), then the engine's words, then the
 * <video> element's error. A decoding error once the title has played is
 * a stream that broke off rather than one that can't be played.
 */
export function classifyFailure(problem: Problem, context: FailureContext): Failure {
  const text = problem.message ?? "";
  const status = problem.status ?? null;
  const channel = context.live ? "channel" : "title";
  if (!context.online) {
    return { kind: "offline", title: "You're offline", message: `${isMac ? "This Mac" : "This computer"} isn't connected to the internet.`, suggestion: "Xorvio keeps your place. Try again once you're connected." };
  }
  if (status === 403 || (status === null && /\b403\b|forbidden|access denied/i.test(text))) {
    return {
      kind: "signIn",
      title: "Access denied by the provider",
      message: "The provider refused this stream (HTTP 403). The account may have expired or reached its connection limit.",
      suggestion: "Close other players using this account, or check the account in Sources.",
    };
  }
  if (status === 401 || status === 407 || (status === null && SIGN_IN.test(text))) {
    return {
      kind: "signIn",
      title: "Sign-in required",
      message: status ? `The provider didn't accept this source's sign-in (HTTP ${status}).` : (readable(text) ?? "The provider didn't accept this source's sign-in."),
      suggestion: "Check the account's details in Sources.",
    };
  }
  if (status === 404 || status === 410 || (status === null && NOT_FOUND.test(text))) {
    return {
      kind: "notFound",
      title: "Stream not found",
      message: status ? `The provider no longer offers this stream (HTTP ${status}).` : (readable(text) ?? "The provider no longer offers this stream."),
      suggestion: "Refresh the source in Sources to update its catalogue.",
    };
  }
  const broke = context.hasPlayed && (problem.mediaError === 3 || problem.mediaError === 4);
  if (!broke && (problem.mediaError === 3 || problem.mediaError === 4 || UNSUPPORTED.test(text))) {
    return {
      kind: "unsupported",
      title: "Format not supported",
      message: problem.mediaError === 3 ? "The video couldn't be decoded." : "This stream is in a format Xorvio can't play.",
      suggestion:
        context.live && context.mode === "mpegts"
          ? "Set this source's live format to HLS in Sources › Edit, or pick another channel."
          : `If there's another copy or version of this ${channel}, choose it instead.`,
    };
  }
  if ((status !== null && status >= 500) || problem.mediaError === 2 || (!problem.ended && !problem.stalled && NETWORK.test(text))) {
    return {
      kind: "network",
      title: "Network problem",
      message:
        status === 502
          ? "The stream couldn't be reached."
          : status === 504
            ? "The provider took too long to answer."
            : status !== null || problem.mediaError === 2
              ? "The connection to the stream was lost."
              : (readable(text) ?? "The connection to the stream was lost."),
      suggestion: "Try again when the connection recovers.",
    };
  }
  if (problem.stalled) {
    return {
      kind: "stalled",
      title: context.hasPlayed ? "The stream stopped" : "The stream didn't start",
      message: context.hasPlayed ? "Nothing more arrived from the provider." : "Nothing arrived from the provider.",
      suggestion: context.live ? "Try again, or pick another channel." : "Try again, or choose another copy.",
    };
  }
  if (problem.ended || broke) {
    return {
      kind: "ended",
      title: "The stream ended unexpectedly",
      message: "The provider stopped sending video.",
      suggestion: context.live ? "Try again, or pick another channel." : "Try again, or choose another copy.",
    };
  }
  return { kind: "unknown", title: "Playback failed", message: readable(text) ?? "Playback stopped unexpectedly.", suggestion: "Try again, or open Diagnostics in Settings for details." };
}

/**
 * Whether a failure is worth trying again by itself. Network trouble always
 * is. Once a live channel has played, so is anything but a format problem:
 * providers refuse or lose a stream for a moment (an old connection still
 * counted, a server restarting) and then carry on. A stream that never
 * started in the time allowed probably won't the next time either.
 */
export function isRecoverable(kind: FailureKind, live: boolean, hasPlayed: boolean): boolean {
  switch (kind) {
    case "network":
    case "ended":
    case "unknown":
      return true;
    case "stalled":
      return hasPlayed;
    case "offline":
    case "signIn":
    case "notFound":
      return live && hasPlayed;
    case "unsupported":
      return false;
  }
}

// MARK: Reconnecting

/**
 * The wait before each attempt to open a stream that dropped, in seconds. A
 * live channel that was playing keeps trying for about two minutes, waiting
 * a little longer each time, which also gives the provider time to let the
 * old connection go. One that never started gives up sooner: it's probably
 * down.
 */
export function reconnectDelays(live: boolean, hasPlayed: boolean): number[] {
  if (live && hasPlayed) return [2, 3, 5, 8, 12, 20, 30, 30];
  if (live) return [2, 4];
  return [1, 2, 4];
}

/** Played this long without a drop, a stream starts counting its attempts again. */
export const STEADY_MS = 30_000;

export interface Attempts {
  /** Attempts made since the last steady play. */
  made: number;
  /** When it last started playing (ms), or null when it hasn't since the last drop. */
  playingSince: number | null;
}

export interface Attempt {
  /** This attempt's number, from 1. */
  number: number;
  of: number;
  /** Seconds to wait before it. */
  delay: number;
}

/** The next attempt after a drop at `now`, or null when they're used up. */
export function nextAttempt(attempts: Attempts, now: number, live: boolean, hasPlayed: boolean): Attempt | null {
  const steady = attempts.playingSince !== null && now - attempts.playingSince > STEADY_MS;
  const made = steady ? 0 : attempts.made;
  const delays = reconnectDelays(live, hasPlayed);
  if (made >= delays.length) return null;
  return { number: made + 1, of: delays.length, delay: delays[made] };
}

/**
 * Where a film or an episode carries on after a drop: where it first
 * dropped, through attempts that failed before getting anywhere. Live
 * channels carry on from live.
 */
export function resumePoint(live: boolean, kept: number | null, position: number): number | null {
  if (live) return null;
  return kept ?? (position > 0 ? position : null);
}

/** "Trying again in 3 s. Attempt 2 of 8." */
export function reconnectStatus(attempt: number, of: number, secondsLeft: number | null): string {
  const count = `Attempt ${attempt} of ${of}.`;
  if (secondsLeft === null) return `The stream dropped. Opening it again… ${count}`;
  return `The stream dropped. Trying again in ${Math.max(1, Math.ceil(secondsLeft))} s. ${count}`;
}

// MARK: Channel numbers

/** Typed digits wait this long for another before the channel changes. */
export const DIGIT_PAUSE_MS = 1400;
/** "No channel 512" stays this long. */
export const NOT_FOUND_MS = 1200;
export const MAX_DIGITS = 5;

/** The number being typed after another key: a sixth digit starts again. Anything but a digit leaves it. */
export function typeDigit(digits: string, key: string): string {
  if (!/^[0-9]$/.test(key)) return digits;
  return (digits.length >= MAX_DIGITS ? "" : digits) + key;
}

/**
 * The channel a typed number means: one from the source playing when
 * several sources number a channel the same, else the first in the list.
 */
export function channelForNumber<T extends { number: number | null; sourceId: string }>(digits: string, channels: T[], preferSource?: string | null): T | null {
  if (!/^[0-9]+$/.test(digits)) return null;
  const number = Number(digits);
  const matches = channels.filter((c) => c.number === number);
  return matches.find((c) => c.sourceId === preferSource) ?? matches[0] ?? null;
}

// MARK: Zapping

/** The wait before a channel changed to with up and down opens its stream. */
export const ZAP_SETTLE_MS = 350;
/** Presses closer together than this are zapping through channels… */
export const RAPID_ZAP_MS = 1000;
/** …and wait longer, so the channels passed on the way never open a provider connection. */
export const RAPID_SETTLE_MS = 600;

/** How long to wait before opening the stream, `sinceLast` ms after the press before (null: none). */
export function zapDelay(sinceLast: number | null): number {
  return sinceLast !== null && sinceLast >= 0 && sinceLast < RAPID_ZAP_MS ? RAPID_SETTLE_MS : ZAP_SETTLE_MS;
}

/** The channel `by` places on from `index` in a list, going round at the ends. */
export function stepChannel(index: number, by: number, count: number): number {
  if (count <= 0) return -1;
  if (index < 0) return by > 0 ? 0 : count - 1;
  return (((index + by) % count) + count) % count;
}

/** The highlight in a list moved by `by`, stopping at the ends. */
export function moveHighlight(index: number, by: number, count: number): number {
  if (count <= 0) return 0;
  return Math.min(count - 1, Math.max(0, index + by));
}

// MARK: The mini player's corner

export type Corner = "topLeft" | "topRight" | "bottomLeft" | "bottomRight";
const CORNERS: Corner[] = ["topLeft", "topRight", "bottomLeft", "bottomRight"];

/** A remembered corner, or the bottom right for anything else. */
export function parseCorner(text: string | null | undefined): Corner {
  return CORNERS.find((c) => c === text) ?? "bottomRight";
}

/** How far ahead (ms) a drag's speed carries the player when it's let go, so a flick goes where it's thrown. */
export const THROW_MS = 160;
/** A press that moves less than this (px) is a click, not a drag. */
export const DRAG_SLOP = 6;

/**
 * The corner a dragged mini player settles in: the quarter of the window
 * its middle is heading for, a short throw on from where it was let go.
 * `velocity` is in px per ms.
 */
export function snapCorner(center: { x: number; y: number }, velocity: { x: number; y: number }, area: { width: number; height: number }): Corner {
  const x = center.x + velocity.x * THROW_MS;
  const y = center.y + velocity.y * THROW_MS;
  const top = y < area.height / 2;
  const left = x < area.width / 2;
  return top ? (left ? "topLeft" : "topRight") : left ? "bottomLeft" : "bottomRight";
}

// MARK: Audio and subtitles for a series

/** Two-letter and alternative codes as the backend's ISO 639-2 ones ("en" and "en-GB" are "eng"). */
const LANGUAGE_CODES: Record<string, string> = {
  en: "eng", fr: "fre", fra: "fre", es: "spa", de: "ger", deu: "ger", it: "ita", pt: "por", nl: "dut", nld: "dut", sv: "swe",
  no: "nor", nb: "nor", nob: "nor", da: "dan", fi: "fin", pl: "pol", ru: "rus", uk: "ukr", tr: "tur", el: "gre", ell: "gre",
  ar: "ara", he: "heb", hi: "hin", ja: "jpn", ko: "kor", zh: "chi", zho: "chi", cs: "cze", ces: "cze", ro: "rum", ron: "rum",
  hu: "hun", hr: "hrv", sr: "srp", bg: "bul", th: "tha", vi: "vie", id: "ind", ms: "may", msa: "may", fa: "per", fas: "per",
};

/** A language code as ISO 639-2, lower case, without a region; null for none or "und". */
export function normalizedLanguage(code: string | null | undefined): string | null {
  const primary = code?.trim().toLowerCase().split(/[-_]/)[0];
  if (!primary || primary === "und") return null;
  return LANGUAGE_CODES[primary] ?? primary;
}

const sameLanguage = (a: string | null | undefined, b: string | null | undefined) => {
  const x = normalizedLanguage(a);
  return x !== null && x === normalizedLanguage(b);
};

/** The choice after picking an audio track: its language, when it has one; otherwise what was remembered stays. */
export function withAudio(choice: TrackChoice | null, language: string | null | undefined): TrackChoice | null {
  const code = normalizedLanguage(language);
  return code ? { ...choice, audioLanguage: code } : choice;
}

/** The choice after picking subtitles: a track, or off (null). */
export function withSubtitles(choice: TrackChoice | null, option: { language: string | null; label: string } | null): TrackChoice {
  return { ...choice, subtitles: option ? { kind: "track", language: normalizedLanguage(option.language), label: option.label } : { kind: "off" } };
}

/**
 * The track (by `id`) in the remembered language, or null to keep what's
 * playing: when it's in that language already, or nothing is.
 */
export function audioFor<T extends { id: number; language: string | null }>(choice: TrackChoice | null, tracks: T[], playing: number | null): number | null {
  const language = choice?.audioLanguage;
  if (!language) return null;
  if (sameLanguage(tracks.find((t) => t.id === playing)?.language, language)) return null;
  return tracks.find((t) => sameLanguage(t.language, language))?.id ?? null;
}

/**
 * The subtitles a remembered choice picks from what this episode offers:
 * "off", an option's key, or null to leave them to the settings (nothing
 * remembered, or nothing in that language yet). The same language and name
 * first, then the same language without forced ones, then any in it.
 * Picture subtitles are left alone: drawing them in encodes the video,
 * which is asked for each time.
 */
export function subtitlesFor<T extends { key: string; label: string; language: string | null; picture: number | null }>(choice: TrackChoice | null, options: T[]): string | null {
  const memory = choice?.subtitles;
  if (!memory) return null;
  if (memory.kind === "off") return "off";
  const text = options.filter((o) => o.picture === null);
  const label = memory.label.toLowerCase();
  if (!memory.language) return text.find((o) => o.label.toLowerCase() === label)?.key ?? null;
  const same = text.filter((o) => sameLanguage(o.language, memory.language));
  const forced = (o: T) => /forced/i.test(o.label);
  return (same.find((o) => o.label.toLowerCase() === label) ?? same.find((o) => !forced(o)) ?? same[0])?.key ?? null;
}
