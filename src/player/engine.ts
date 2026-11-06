// Attaches a gateway stream to the <video> element with the right engine.
// WebKit (macOS) plays HLS natively, with AC-3/E-AC-3 and HEVC, so it's used
// there; elsewhere hls.js does. Raw MPEG-TS, and anything the gateway remuxed
// with ffmpeg, goes through mpegts.js.

import type Hls from "hls.js";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import type { PlaybackSupport, ResolvedStream, Settings } from "../lib/types";
import type { Problem } from "./playerRules";

export interface AudioTrackInfo {
  id: number;
  label: string;
  language: string | null;
}

export interface EngineHandle {
  destroy(): void;
  audioTracks(): AudioTrackInfo[];
  setAudioTrack(id: number): void;
}

/**
 * Tells the gateway what this web view's Media Source decodes, so remuxing
 * copies Dolby audio and HEVC where it can and converts them where it can't,
 * and what the <video> element plays by itself, so VP9 and AV1 files play
 * directly where they can.
 */
export function reportPlaybackSupport() {
  const source = (window as unknown as { ManagedMediaSource?: typeof MediaSource }).ManagedMediaSource ?? window.MediaSource;
  const has = (type: string) => {
    try {
      return !!source?.isTypeSupported(type);
    } catch {
      return false;
    }
  };
  const video = document.createElement("video");
  const plays = (type: string) => video.canPlayType(type) === "probably";
  const support: PlaybackSupport = {
    hevc: has('video/mp4; codecs="hvc1.1.6.L120.90"'),
    ac3: has('audio/mp4; codecs="ac-3"'),
    eac3: has('audio/mp4; codecs="ec-3"'),
    vp9: plays('video/mp4; codecs="vp09.00.10.08"') || plays('video/webm; codecs="vp9"'),
    av1: plays('video/mp4; codecs="av01.0.08M.08"'),
  };
  api.setPlaybackSupport(support).catch(() => {});
}

const nothing: EngineHandle = { destroy() {}, audioTracks: () => [], setAudioTrack() {} };

type Latency = Settings["liveLatency"];

/**
 * How far behind live hls.js plays (Settings › Live TV latency), in target
 * durations; "automatic" keeps its defaults. WebKit's own HLS player, which
 * plays HLS on macOS, sets its own.
 */
function hlsLatency(latency: Latency) {
  if (latency === "low") return { liveSyncDurationCount: 2, liveMaxLatencyDurationCount: 4 };
  if (latency === "minimal") return { liveSyncDurationCount: 1, liveMaxLatencyDurationCount: 3, maxLiveSyncPlaybackRate: 1.2 };
  return {};
}

/** The same for mpegts.js, in seconds, which already chases the live edge. */
function mpegtsLatency(latency: Latency) {
  if (latency === "low") return { liveBufferLatencyMaxLatency: 1, liveBufferLatencyMinRemain: 0.3 };
  if (latency === "minimal") return { enableStashBuffer: false, liveBufferLatencyMaxLatency: 0.8, liveBufferLatencyMinRemain: 0.2 };
  return {};
}

/**
 * Plays `stream` in `video`. `onFatal` hears of what the engine can't get
 * past by itself, in words; `onProblem`, when given, hears instead, with the
 * HTTP status where the engine knows it and a live stream that ended (the
 * player decides whether to open the stream again).
 */
export async function attachStream(
  video: HTMLVideoElement,
  stream: ResolvedStream,
  onFatal: (message: string) => void,
  onProblem?: (problem: Problem) => void,
): Promise<EngineHandle> {
  const report = onProblem ?? ((problem: Problem) => onFatal(problem.message ?? "Playback stopped unexpectedly."));
  // A buffered channel plays from this computer, where being behind live is the point.
  const latency: Latency = stream.isLive && !stream.timeshift ? useApp.getState().settings.liveLatency : "automatic";
  switch (stream.mode) {
    case "native":
      video.src = stream.url;
      return nativeTracks(video);

    case "hls": {
      if (video.canPlayType("application/vnd.apple.mpegurl")) {
        video.src = stream.url;
        return nativeTracks(video);
      }
      const { default: HlsClass } = await import("hls.js");
      if (!HlsClass.isSupported()) {
        report({ message: "This system can't play HLS streams." });
        return nothing;
      }
      const hls: Hls = new HlsClass({
        lowLatencyMode: false,
        backBufferLength: stream.isLive ? 30 : 90,
        maxBufferLength: 30,
        enableWorker: true,
        ...hlsLatency(latency),
      });
      hls.on(HlsClass.Events.ERROR, (_, data) => {
        if (!data.fatal) return;
        const status = data.response?.code ?? null;
        // A refusal won't change by asking again; the player says why.
        if (onProblem && data.type === HlsClass.ErrorTypes.NETWORK_ERROR && status && status >= 400 && status < 500) onProblem({ message: describeHlsError(data.details), status });
        else if (data.type === HlsClass.ErrorTypes.NETWORK_ERROR) hls.startLoad();
        else if (data.type === HlsClass.ErrorTypes.MEDIA_ERROR) hls.recoverMediaError();
        else report({ message: describeHlsError(data.details) });
      });
      hls.loadSource(stream.url);
      hls.attachMedia(video);
      return {
        destroy: () => hls.destroy(),
        audioTracks: () => hls.audioTracks.map((t, i) => ({ id: i, label: t.name || t.lang || `Track ${i + 1}`, language: t.lang ?? null })),
        setAudioTrack: (id) => {
          hls.audioTrack = id;
        },
      };
    }

    // The gateway remuxes to MPEG-TS on the fly (no length, no ranges), so it
    // goes through mpegts.js like a live channel.
    case "remux":
    case "mpegts": {
      const mpegts = (await import("mpegts.js")).default;
      if (!mpegts.isSupported()) {
        report({ message: "This system can't play MPEG-TS streams." });
        return nothing;
      }
      const remux = stream.mode === "remux";
      const player = mpegts.createPlayer(
        { type: "mpegts", isLive: stream.isLive, url: stream.url },
        {
          enableWorker: true,
          liveBufferLatencyChasing: stream.isLive,
          lazyLoad: false,
          autoCleanupSourceBuffer: true,
          // A remuxed film seeks by starting ffmpeg again, never into what's
          // already played, so less of that is kept (the default is 3 minutes).
          ...(remux ? { autoCleanupMaxBackwardDuration: 60, autoCleanupMinBackwardDuration: 30 } : {}),
          ...mpegtsLatency(latency),
        },
      );
      const stopReporting = remux ? reportPosition(video, stream.url) : () => {};
      player.on(mpegts.Events.ERROR, (type: string, detail: string, info?: { code?: number }) =>
        report({ message: `${type}: ${detail}`, status: typeof info?.code === "number" && info.code >= 400 ? info.code : null }),
      );
      // A live stream has no end: its connection closing is a drop, which is
      // better noticed now than once what's buffered has played out.
      if (stream.isLive && onProblem) player.on(mpegts.Events.LOADING_COMPLETE, () => onProblem({ ended: true, message: "The live stream ended." }));
      player.attachMediaElement(video);
      player.load();
      return {
        destroy: () => {
          stopReporting();
          player.pause();
          player.unload();
          player.detachMediaElement();
          player.destroy();
        },
        audioTracks: () => [],
        setAudioTrack() {},
      };
    }
  }
}

/**
 * Tells the gateway where a remuxed stream has got to, about once a second
 * while it plays. ffmpeg copies a film far faster than it plays, and Media
 * Source holds only a couple of hundred megabytes: mpegts.js, told it's
 * full, drops the connection and never asks again, so the film would stop a
 * few minutes in. The gateway keeps ffmpeg a minute or so ahead of what it
 * hears instead (crates/xorvio-media/src/pace.rs). Paused, nothing is
 * reported and ffmpeg waits where it is.
 */
function reportPosition(video: HTMLVideoElement, url: string) {
  let last = -Infinity;
  const report = () => {
    const now = performance.now();
    if (now - last < 1000) return;
    last = now;
    api.streamPosition(url, video.currentTime).catch(() => {});
  };
  video.addEventListener("timeupdate", report);
  return () => video.removeEventListener("timeupdate", report);
}

/** WebKit exposes a file's or HLS stream's audio tracks on the element itself. */
function nativeTracks(video: HTMLVideoElement): EngineHandle {
  type NativeTrack = { id: string; label: string; language: string; enabled: boolean };
  const list = () => (video as unknown as { audioTracks?: ArrayLike<NativeTrack> }).audioTracks;
  return {
    destroy: () => {
      video.removeAttribute("src");
      video.load();
    },
    audioTracks: () => Array.from(list() ?? []).map((t, i) => ({ id: i, label: t.label || t.language || `Track ${i + 1}`, language: t.language || null })),
    setAudioTrack: (id) => Array.from(list() ?? []).forEach((t, i) => (t.enabled = i === id)),
  };
}

function describeHlsError(details: string) {
  if (/manifest/i.test(details)) return "The stream's playlist couldn't be loaded. The channel may be offline.";
  if (/key/i.test(details)) return "The stream's encryption key couldn't be loaded.";
  return "The stream stopped and couldn't be recovered.";
}

/** A plain-English reason for a <video> element error. */
export function describeMediaError(error: MediaError | null) {
  switch (error?.code) {
    case MediaError.MEDIA_ERR_NETWORK:
      return "The connection to the stream was lost.";
    case MediaError.MEDIA_ERR_DECODE:
      return "The video couldn't be decoded.";
    case MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED:
      return "This format isn't supported, or the stream is offline.";
    default:
      return "Playback stopped unexpectedly.";
  }
}
