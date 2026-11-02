// Keeping a stream going. A live channel (or a film) whose stream drops is
// opened again, a few times with longer waits between, before the player
// says why it can't play (playerRules.ts decides when, and the words). Each
// attempt lets the dropped stream go first and then asks the backend for the
// title again, so a provider that allows one connection never sees two for
// one play. Films and episodes carry on from where they dropped, with the
// audio they had.

import { create } from "zustand";
import { api, errorText } from "../lib/api";
import { useApp, type NowPlaying } from "../lib/store";
import type { ResolvedStream, StreamOptions } from "../lib/types";
import { isCasting } from "./cast";
import { classifyFailure, isRecoverable, nextAttempt, resumePoint, type Attempts, type Failure, type Problem } from "./playerRules";

export interface Recovery {
  /** The stream dropped and is being opened again; `nextAt` (ms) is when the next attempt starts, null while one is under way. */
  reconnecting: { attempt: number; of: number; nextAt: number | null } | null;
  /** It couldn't be played, and won't be tried again by itself. */
  failure: Failure | null;
}

export const useRecovery = create<Recovery>(() => ({ reconnecting: null, failure: null }));

// About the title playing; everything starts again with another.
let item: NowPlaying | null = null;
let attempts: Attempts = { made: 0, playingSince: null };
let hasPlayed = false;
let live = false;
/** Where a film or episode carries on from. */
let resumeAt: number | null = null;
/** The stream that dropped, for the audio and subtitles it had. */
let dropped: ResolvedStream | null = null;
/** Seconds to put a film played directly at once its stream loads again. */
let pendingSeek: number | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;

function reset(next: NowPlaying | null) {
  clearTimeout(timer);
  item = next;
  attempts = { made: 0, playingSince: null };
  hasPlayed = false;
  live = next?.kind === "channel";
  resumeAt = null;
  dropped = null;
  pendingSeek = null;
  const state = useRecovery.getState();
  if (state.reconnecting || state.failure) useRecovery.setState({ reconnecting: null, failure: null });
}

// Another title (or none) starts afresh. Multiview taking the channel into a
// tile ends up here too, and its stream is the tile's now, so nothing is released.
useApp.subscribe((now, before) => {
  if (now.nowPlaying !== before.nowPlaying) reset(now.nowPlaying);
});

/** Lets the stream playing go, so the provider counts it closed before another opens. */
function letGo(stream: ResolvedStream | null) {
  if (!stream) return;
  api.releaseStream(stream.url).catch(() => {});
  dropped = stream;
  if (useApp.getState().stream === stream) useApp.setState({ stream: null });
}

/** The stream started playing (again): the card goes, and once it's played a while its attempts count from the start. */
export function streamPlaying() {
  hasPlayed = true;
  if (attempts.playingSince === null) attempts = { ...attempts, playingSince: Date.now() };
  resumeAt = null;
  const state = useRecovery.getState();
  if (state.reconnecting || state.failure) useRecovery.setState({ reconnecting: null, failure: null });
}

/**
 * The stream at `url` dropped or failed. It's opened again when that's worth
 * trying; otherwise the player says why. `position` is where a film or
 * episode was, in its own time.
 */
export async function streamFailed(problem: Problem, url: string, position: number) {
  const { nowPlaying: playing, stream } = useApp.getState();
  if (!playing || playing !== item || stream?.url !== url || isCasting()) return;
  // The gateway knows what the provider answered; the web view's players don't say.
  const status = problem.status ?? (await api.streamFailure(url).catch(() => null));
  // Something else reported the same drop first, or the title changed meanwhile.
  if (useApp.getState().stream?.url !== url || item !== playing) return;
  live = stream.isLive;
  decide({ ...problem, status }, stream, position);
}

function decide(problem: Problem, stream: ResolvedStream | null, position: number) {
  const failure = classifyFailure(problem, { live, online: navigator.onLine, hasPlayed, mode: (stream ?? dropped)?.mode });
  api.log("warn", `playback ${failure.kind}${problem.status ? ` (HTTP ${problem.status})` : ""}: ${problem.message ?? failure.title}`).catch(() => {});
  const attempt = isRecoverable(failure.kind, live, hasPlayed) ? nextAttempt(attempts, Date.now(), live, hasPlayed) : null;
  const reconnecting = !!useRecovery.getState().reconnecting;
  letGo(stream);
  if (!attempt) {
    const tried = reconnecting ? attempts.made : 0;
    const message = tried ? `${failure.message} Xorvio tried to reconnect ${tried} time${tried === 1 ? "" : "s"}.` : failure.message;
    useRecovery.setState({ reconnecting: null, failure: { ...failure, message } });
    return;
  }
  // A failed attempt carries on from where it first dropped, not from where the attempt got to.
  resumeAt = resumePoint(live, reconnecting ? resumeAt : null, position);
  attempts = { made: attempt.number, playingSince: null };
  api.log("info", `reconnecting in ${attempt.delay} s (attempt ${attempt.number} of ${attempt.of})`).catch(() => {});
  useRecovery.setState({ reconnecting: { attempt: attempt.number, of: attempt.of, nextAt: Date.now() + attempt.delay * 1000 }, failure: null });
  clearTimeout(timer);
  timer = setTimeout(reopen, attempt.delay * 1000);
}

/**
 * A film or episode opened again where it dropped. A remuxed one starts
 * there (and with the audio and drawn-in subtitles it had), rather than at
 * the beginning and then seeking, which would start ffmpeg twice; one played
 * directly seeks once it loads.
 */
async function carryOn(fresh: ResolvedStream): Promise<ResolvedStream> {
  if (fresh.isLive) return fresh;
  const from = resumeAt ?? 0;
  if (fresh.mode !== "remux") {
    if (from > 5) pendingSeek = from;
    return fresh;
  }
  const before = dropped?.mode === "remux" ? dropped : null;
  const options: StreamOptions = {};
  if (before && before.audioIndex !== null && before.audioIndex !== fresh.audioIndex) options.audioIndex = before.audioIndex;
  if (before?.burnedSubtitle != null) options.burnSubtitle = before.burnedSubtitle;
  if (before?.audioDelaySeconds) options.audioDelaySeconds = before.audioDelaySeconds;
  if (from <= 5 && !Object.keys(options).length) return fresh;
  return api.restream(fresh.url, from, options);
}

async function reopen() {
  const playing = item;
  const current = () => !!playing && item === playing && useApp.getState().nowPlaying === playing;
  if (!current()) return;
  clearTimeout(timer);
  useRecovery.setState((s) => ({ reconnecting: s.reconnecting && { ...s.reconnecting, nextAt: null } }));
  let fresh: ResolvedStream | null = null;
  try {
    fresh = await api.play(playing!.request);
    if (current()) fresh = await carryOn(fresh);
    if (!current()) {
      api.releaseStream(fresh.url).catch(() => {});
      return;
    }
    useApp.setState({ stream: fresh, playError: null });
  } catch (e) {
    if (fresh) api.releaseStream(fresh.url).catch(() => {});
    if (current()) decide({ message: errorText(e) }, null, resumeAt ?? 0);
  }
}

/** Try Now: the next attempt without waiting for it. */
export function reconnectNow() {
  if (useRecovery.getState().reconnecting?.nextAt == null) return;
  reopen();
}

// Back online while waiting to try again: no point waiting longer.
if (typeof window !== "undefined") window.addEventListener("online", reconnectNow);

/** Try Again, after a failure: from where it was, with every attempt to come. */
export function retry() {
  const { nowPlaying: playing, stream } = useApp.getState();
  if (!playing) return;
  if (item !== playing) reset(playing);
  clearTimeout(timer);
  attempts = { made: 0, playingSince: null };
  letGo(stream);
  useApp.setState({ playError: null });
  useRecovery.setState({ reconnecting: null, failure: null });
  reopen();
}

/** Where to put a film played directly once its reopened stream loads; asks once. */
export function takeResumeSeek(): number | null {
  const at = pendingSeek;
  pendingSeek = null;
  return at;
}

/** Whether the title playing has played at all, so a stall is a drop rather than a slow start. */
export function titleHasPlayed() {
  return hasPlayed;
}
