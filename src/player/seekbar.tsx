// The seek bar's extras for films, episodes and recordings: the frame under
// the pointer (from thumbnail sheets the backend makes once per title in
// the background), and a recording's ad breaks marked along it. Times here
// are the title's own, as the seek bar's are, so a remuxed stream that
// restarted at an offset shows the same frames.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { create } from "zustand";
import { api } from "../lib/api";
import { playerClock } from "../lib/format";
import { useApp, type NowPlaying } from "../lib/store";
import type { AdBreak, ResolvedStream, ThumbnailSheet } from "../lib/types";

interface SeekExtras {
  /** The title's thumbnails, once they're made. */
  sheet: ThumbnailSheet | null;
  /** A recording's ad breaks, unless Settings › Recording turns them off. */
  breaks: AdBreak[];
}

export const useSeekExtras = create<SeekExtras>(() => ({ sheet: null, breaks: [] }));

/** How often to ask again while thumbnails are made, and while ad breaks are looked for. */
const THUMBNAILS_AGAIN = 4000;
const BREAKS_AGAIN = 10000;
/** The scrubber's thumb, whose middle runs from half its width in to half its width short of the end. */
const THUMB = 15;

/** The ad break `position` is in, short of its last second (so Skip Ad doesn't flash at its end). */
export function adBreakAt(breaks: AdBreak[], position: number) {
  return breaks.find((b) => position >= b.startSeconds && position < b.endSeconds - 1) ?? null;
}

/**
 * Keeps the playing title's thumbnails and ad breaks in `useSeekExtras`:
 * asked for once a film, episode or recording is playing, and asked again
 * while they're still being made or looked for.
 */
export function useSeekExtrasLoader(nowPlaying: NowPlaying | null, stream: ResolvedStream | null) {
  const commercialSkip = useApp((s) => s.settings.commercialSkip);
  const streamRef = useRef(stream);
  streamRef.current = stream;
  const titled = !!nowPlaying && (nowPlaying.kind === "movie" || nowPlaying.kind === "episode" || nowPlaying.kind === "recording");
  const ready = !!stream && !stream.isLive;
  const itemId = titled ? nowPlaying.itemId : null;
  const sourceId = nowPlaying?.sourceId ?? "";
  const recordingId = nowPlaying?.request.kind === "recording" ? nowPlaying.request.recordingId : null;

  // One sheet per title: a seek's new stream address is the same title.
  useEffect(() => {
    useSeekExtras.setState({ sheet: null });
    if (!itemId || !ready) return;
    let cancelled = false;
    let again: ReturnType<typeof setTimeout> | undefined;
    const ask = async () => {
      const playing = streamRef.current;
      if (!playing || playing.isLive) return;
      try {
        const result = await api.thumbnails(playing.url, itemId, sourceId);
        if (cancelled) return;
        if (result.state === "ready") useSeekExtras.setState({ sheet: result.sheet });
        else if (result.state === "making") again = setTimeout(ask, THUMBNAILS_AGAIN);
        else api.log("info", `no thumbnails: ${result.reason}`).catch(() => {});
      } catch {
        // The seek bar shows the time alone.
      }
    };
    ask();
    return () => {
      cancelled = true;
      clearTimeout(again);
    };
  }, [itemId, sourceId, ready]);

  useEffect(() => {
    useSeekExtras.setState({ breaks: [] });
    if (!recordingId || commercialSkip === "off") return;
    let cancelled = false;
    let again: ReturnType<typeof setTimeout> | undefined;
    const ask = async () => {
      try {
        const found = await api.adBreaks(recordingId);
        if (cancelled) return;
        useSeekExtras.setState({ breaks: found.breaks });
        if (found.state === "finding") again = setTimeout(ask, BREAKS_AGAIN);
      } catch {
        // No marks, then.
      }
    };
    ask();
    return () => {
      cancelled = true;
      clearTimeout(again);
    };
  }, [recordingId, commercialSkip]);
}

/** The ad breaks as a layer over the scrubber's track. */
function breakMarks(breaks: AdBreak[], duration: number) {
  if (!breaks.length || !duration) return undefined;
  const at = (seconds: number) => `${Math.min(100, Math.max(0, (seconds / duration) * 100)).toFixed(3)}%`;
  const stops = breaks.flatMap((b) => [`transparent ${at(b.startSeconds)}`, `var(--ad-mark) ${at(b.startSeconds)}`, `var(--ad-mark) ${at(b.endSeconds)}`, `transparent ${at(b.endSeconds)}`]);
  return `linear-gradient(to right, transparent 0%, ${stops.join(", ")}, transparent 100%)`;
}

/**
 * Wraps the seek bar's scrubber: marks the ad breaks along it, and shows the
 * frame and time under the pointer (or the thumb, while dragging) above it.
 * Until a title's thumbnails are made, the time alone.
 */
export function SeekTrack({ duration, scrub, children }: { duration: number; scrub: number | null; children: ReactNode }) {
  const sheet = useSeekExtras((s) => s.sheet);
  const breaks = useSeekExtras((s) => s.breaks);
  const ref = useRef<HTMLDivElement>(null);
  const [pointer, setPointer] = useState<number | null>(null);
  const width = ref.current?.getBoundingClientRect().width ?? 0;
  const usable = Math.max(1, width - THUMB);
  const timeAt = (clientX: number) => {
    const box = ref.current?.getBoundingClientRect();
    if (!box || !duration) return null;
    return Math.min(1, Math.max(0, (clientX - box.left - THUMB / 2) / Math.max(1, box.width - THUMB))) * duration;
  };
  const time = scrub ?? pointer;
  const shown = time != null && duration > 0 && width > 0;
  return (
    <div
      ref={ref}
      className="seek-track"
      style={{ "--marks": breakMarks(breaks, duration) } as React.CSSProperties}
      onMouseMove={(e) => setPointer(timeAt(e.clientX))}
      onMouseLeave={() => setPointer(null)}
    >
      {children}
      {shown && <SeekPreview sheet={sheet} time={time} x={THUMB / 2 + (Math.min(time, duration) / duration) * usable} trackWidth={width} inBreak={!!adBreakAt(breaks, time)} />}
    </div>
  );
}

/** The frame (when there's a sheet) and the time, above a point on the track. */
function SeekPreview({ sheet, time, x, trackWidth, inBreak }: { sheet: ThumbnailSheet | null; time: number; x: number; trackWidth: number; inBreak: boolean }) {
  const half = (sheet?.width ?? 0) / 2;
  const left = trackWidth > 2 * half ? Math.min(trackWidth - half, Math.max(half, x)) : x;
  return (
    <div className="seek-preview" style={{ left }} aria-hidden>
      {sheet && <SeekFrame sheet={sheet} time={time} />}
      <span className="seek-preview-time">
        {playerClock(time)}
        {inBreak && <span className="seek-preview-note"> · Ad break</span>}
      </span>
    </div>
  );
}

function SeekFrame({ sheet, time }: { sheet: ThumbnailSheet; time: number }) {
  const tile = Math.min(sheet.count - 1, Math.max(0, Math.floor(time / sheet.intervalSeconds)));
  const perSheet = sheet.columns * sheet.rows;
  const image = sheet.images[Math.floor(tile / perSheet)];
  const within = tile % perSheet;
  const column = within % sheet.columns;
  const row = Math.floor(within / sheet.columns);
  if (!image) return null;
  return (
    <div
      className="seek-preview-frame"
      style={{
        width: sheet.width,
        height: sheet.height,
        backgroundImage: `url("${image}")`,
        backgroundPosition: `${-column * sheet.width}px ${-row * sheet.height}px`,
      }}
    />
  );
}
