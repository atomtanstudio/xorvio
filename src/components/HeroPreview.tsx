// A short, muted excerpt of the featured film itself, faded in behind Home's
// hero after a pause, as Xorvio for Mac's PreviewClip. It's off unless chosen
// in Settings (backdrops otherwise never move), and it streams from the
// person's own source, so it waits until nothing else plays, gives way the
// moment anything does (see previewClips.ts), and only plays files the web
// view plays as they are: a preview never starts ffmpeg.
//
// Trailers aren't used: TMDB's are YouTube videos, which play only in
// YouTube's own player, with its controls and branding over the hero.

import { useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { registerPreview } from "../lib/previewClips";
import { useApp } from "../lib/store";
import type { Movie, ResolvedStream } from "../lib/types";
import { useMultiview } from "../player/multiviewStore";

/** Containers WebKit plays itself. */
const PLAYABLE = new Set(["mp4", "m4v", "mov", "m3u8"]);

const extensionOf = (path: string) => path.split(/[?#]/)[0].split("/").pop()?.split(".").slice(1).pop()?.toLowerCase() ?? "";

/** Whether a film can have a preview: its own stream, in a container the web view plays directly. */
export function canPreview(movie: Movie) {
  const stream = movie.stream;
  switch (stream.type) {
    case "xtream":
      return PLAYABLE.has(stream.extension.toLowerCase());
    case "url":
      return PLAYABLE.has(extensionOf(stream.url));
    case "file":
      return PLAYABLE.has(extensionOf(stream.path));
    case "server":
      return !!stream.extension && PLAYABLE.has(stream.extension.toLowerCase());
    // An add-on film has no stream until a copy is chosen.
    case "addon":
      return false;
  }
}

/** Nothing else is playing or about to, and the window is on screen. */
function free() {
  const { nowPlaying, poppedOut, recordings } = useApp.getState();
  const recording = [...recordings.values()].some((r) => r.status === "recording");
  return !nowPlaying && !poppedOut && !recording && !useMultiview.getState().open && document.visibilityState === "visible";
}

const prefersStill = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

const later = (ms: number, timers: ReturnType<typeof setTimeout>[]) => new Promise<void>((done) => timers.push(setTimeout(done, ms)));

/** Resolves once the video can play, or after `ms` regardless. */
function ready(video: HTMLVideoElement, ms: number) {
  return new Promise<boolean>((done) => {
    if (video.readyState >= 2) return done(true);
    const finish = (ok: boolean) => {
      video.removeEventListener("canplay", yes);
      video.removeEventListener("error", no);
      clearTimeout(timer);
      done(ok);
    };
    const yes = () => finish(true);
    const no = () => finish(false);
    const timer = setTimeout(no, ms);
    video.addEventListener("canplay", yes);
    video.addEventListener("error", no);
  });
}

/** `onPlaying` hears when the clip shows and when it's gone, so the hero holds its title meanwhile. */
export function HeroPreview({ movie, onPlaying }: { movie: Movie; onPlaying?: (playing: boolean) => void }) {
  const video = useRef<HTMLVideoElement>(null);
  const [visible, setVisible] = useState(false);
  const report = useRef(onPlaying);
  report.current = onPlaying;

  useEffect(() => {
    if (prefersStill() || !canPreview(movie)) return;
    let stopped = false;
    let stream: ResolvedStream | null = null;
    const timers: ReturnType<typeof setTimeout>[] = [];
    const stop = () => {
      stopped = true;
      timers.forEach(clearTimeout);
      const element = video.current;
      if (element) {
        element.pause();
        element.removeAttribute("src");
        element.load();
      }
      if (stream) api.releaseStream(stream.url).catch(() => {});
      stream = null;
      setVisible(false);
      report.current?.(false);
    };
    const letGo = registerPreview(stop);
    const offMultiview = useMultiview.subscribe((s) => s.open && stop());

    (async () => {
      // Quick browsing through the hero shouldn't open streams.
      await later(4000, timers);
      if (stopped || !free()) return;
      const opened = await api.play({ kind: "movie", movieId: movie.id }).catch(() => null);
      if (!opened) return;
      if (stopped) {
        api.releaseStream(opened.url).catch(() => {});
        return;
      }
      stream = opened;
      const element = video.current;
      const direct = opened.mode === "native" || (opened.mode === "hls" && !!element?.canPlayType("application/vnd.apple.mpegurl"));
      if (!element || !direct) return stop();
      // Said again here: muted is what lets a clip play without a click.
      element.muted = true;
      element.src = opened.url;
      if (!(await ready(element, 8000)) || stopped) return stop();
      // A little way in, past titles and logos.
      if (Number.isFinite(element.duration) && element.duration > 240) element.currentTime = element.duration * 0.12;
      await element.play().catch(() => stop());
      await later(800, timers);
      if (stopped) return;
      setVisible(true);
      report.current?.(true);
      await later(30000, timers);
      if (stopped) return;
      setVisible(false);
      report.current?.(false);
      await later(1500, timers);
      stop();
    })();

    return () => {
      letGo();
      offMultiview();
      stop();
    };
  }, [movie]);

  return <video ref={video} className={`hero-preview ${visible ? "visible" : ""}`} muted playsInline disablePictureInPicture aria-hidden="true" tabIndex={-1} />;
}
