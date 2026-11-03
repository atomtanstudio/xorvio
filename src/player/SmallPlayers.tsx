// The player when it isn't full screen: docked in Live TV's header, with
// controls when the pointer is over it, and the mini player, which keeps
// playing in a corner while you browse. The mini player can be dragged to
// any corner; it settles there and stays there next time.

import { Expand, Maximize2, Pause, Play, SquareArrowOutUpRight, Volume2, VolumeX, X } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { isTauri } from "../lib/api";
import { shortcutLabel } from "../lib/commands";
import { useApp } from "../lib/store";
import { pauseByPerson } from "./pause";
import { DRAG_SLOP, parseCorner, snapCorner, type Corner } from "./playerRules";

const CORNER_KEY = "player.miniCorner";

function togglePlay(video: RefObject<HTMLVideoElement | null>) {
  const el = video.current;
  if (!el) return;
  if (el.paused) el.play().catch(() => {});
  else pauseByPerson(el);
}

/** Whether the video is muted, kept up to date however it changes (a key, the menu bar). */
function useMuted(video: RefObject<HTMLVideoElement | null>) {
  const [muted, setMuted] = useState(() => video.current?.muted ?? false);
  useEffect(() => {
    const el = video.current;
    if (!el) return;
    const update = () => setMuted(el.muted || el.volume === 0);
    update();
    el.addEventListener("volumechange", update);
    return () => el.removeEventListener("volumechange", update);
  }, [video]);
  return muted;
}

interface SmallProps {
  video: RefObject<HTMLVideoElement | null>;
  paused: boolean;
  onClose: () => void;
  /** Into the pop-out player's window. */
  onPopOut: () => void;
}

/** Over the video docked in Live TV's header: play or pause, mute, pop out, full screen and stop. */
export function DockedControls({ video, paused, onClose, onPopOut }: SmallProps) {
  const muted = useMuted(video);
  return (
    <div className="dock-controls glass" onClick={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()}>
      <button className="player-icon" onClick={() => togglePlay(video)} aria-label={paused ? "Play" : "Pause"} title="Play or pause">
        {paused ? <Play size={16} fill="currentColor" strokeWidth={0} /> : <Pause size={16} fill="currentColor" strokeWidth={0} />}
      </button>
      <button
        className="player-icon"
        onClick={() => {
          const el = video.current;
          if (el) el.muted = !el.muted;
        }}
        aria-label={muted ? "Unmute" : "Mute"}
        title={muted ? "Unmute" : "Mute"}
      >
        {muted ? <VolumeX size={16} /> : <Volume2 size={16} />}
      </button>
      <div style={{ flex: 1 }} />
      {isTauri && (
        <button className="player-icon" onClick={onPopOut} aria-label="Pop out video" title={`Pop out into its own window (${shortcutLabel("playback.popOut")})`}>
          <SquareArrowOutUpRight size={16} />
        </button>
      )}
      <button className="player-icon" onClick={() => useApp.getState().setPresentation("full")} aria-label="Full screen" title="Full screen">
        <Maximize2 size={16} />
      </button>
      <button className="player-icon" onClick={onClose} aria-label="Stop" title="Stop">
        <X size={16} />
      </button>
    </div>
  );
}

/** The mini player's strip under the picture: what's playing, play or pause, pop out and stop. */
export function MiniStrip({ video, paused, onClose, onPopOut }: SmallProps) {
  const nowPlaying = useApp((s) => s.nowPlaying);
  if (!nowPlaying) return null;
  return (
    <>
      <button className="mini-expand glass" onClick={() => useApp.getState().setPresentation("full")} aria-label="Show the full player" title="Full player">
        <Expand size={13} />
      </button>
      <div className="mini-strip" onClick={(e) => e.stopPropagation()}>
        <div className="mini-strip-text">
          <div className="mini-strip-title">{nowPlaying.title}</div>
          {nowPlaying.subtitle && <div className="mini-strip-subtitle">{nowPlaying.subtitle}</div>}
        </div>
        <button className="player-icon" onClick={() => togglePlay(video)} aria-label={paused ? "Play" : "Pause"} title="Play or pause">
          {paused ? <Play size={15} fill="currentColor" strokeWidth={0} /> : <Pause size={15} fill="currentColor" strokeWidth={0} />}
        </button>
        {isTauri && (
          <button className="player-icon" onClick={onPopOut} aria-label="Pop out video" title={`Pop out into its own window (${shortcutLabel("playback.popOut")})`}>
            <SquareArrowOutUpRight size={15} />
          </button>
        )}
        <button className="player-icon" onClick={onClose} aria-label="Stop playback" title="Stop playback">
          <X size={15} />
        </button>
      </div>
    </>
  );
}

function savedCorner(key: string): Corner {
  try {
    return parseCorner(localStorage.getItem(key));
  } catch {
    return "bottomRight";
  }
}

/**
 * Dragging the mini player (by its picture or its strip) to a corner. It
 * follows the pointer, then settles in the corner it's heading for, sliding
 * there from where it was let go; a press that hardly moves is a click.
 * Multiview's inset is dragged the same way, its corner kept under its own
 * `storageKey`.
 */
export function useMiniPlayerCorner(element: RefObject<HTMLDivElement | null>, active: boolean, storageKey = CORNER_KEY) {
  const [corner, setCorner] = useState<Corner>(() => savedCorner(storageKey));
  const [offset, setOffset] = useState<{ x: number; y: number } | null>(null);
  // Where it was let go, for sliding from there into the corner (the same one, too).
  const dropped = useRef<DOMRect | null>(null);
  const [drops, setDrops] = useState(0);
  const dragged = useRef(false);

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const el = element.current;
      if (!active || !el || e.button !== 0 || (e.target as HTMLElement).closest("button, input, a")) return;
      const start = { x: e.clientX, y: e.clientY };
      const rect = el.getBoundingClientRect();
      let moving = false;
      let last = { x: e.clientX, y: e.clientY, t: performance.now() };
      let velocity = { x: 0, y: 0 };
      dragged.current = false;
      const move = (ev: PointerEvent) => {
        const dx = ev.clientX - start.x;
        const dy = ev.clientY - start.y;
        if (!moving && Math.hypot(dx, dy) < DRAG_SLOP) return;
        moving = true;
        const now = performance.now();
        const dt = Math.max(1, now - last.t);
        velocity = { x: (ev.clientX - last.x) / dt, y: (ev.clientY - last.y) / dt };
        last = { x: ev.clientX, y: ev.clientY, t: now };
        setOffset({ x: dx, y: dy });
      };
      const up = (ev: PointerEvent) => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        if (!moving) return;
        dragged.current = true;
        // A pause before letting go is no throw.
        if (performance.now() - last.t > 80) velocity = { x: 0, y: 0 };
        const dx = ev.clientX - start.x;
        const dy = ev.clientY - start.y;
        const next = snapCorner({ x: rect.left + rect.width / 2 + dx, y: rect.top + rect.height / 2 + dy }, velocity, { width: window.innerWidth, height: window.innerHeight });
        dropped.current = new DOMRect(rect.left + dx, rect.top + dy, rect.width, rect.height);
        setOffset(null);
        setCorner(next);
        setDrops((n) => n + 1);
        try {
          localStorage.setItem(storageKey, next);
        } catch {
          // Not remembered; it still goes there.
        }
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
    },
    [active, element, storageKey],
  );

  // Slides from where it was let go into its corner, rather than jumping.
  useLayoutEffect(() => {
    const el = element.current;
    const from = dropped.current;
    dropped.current = null;
    if (!el || !from || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const to = el.getBoundingClientRect();
    el.style.transition = "none";
    el.style.transform = `translate(${from.left - to.left}px, ${from.top - to.top}px)`;
    void el.offsetWidth;
    el.style.transition = "transform 0.28s cubic-bezier(0.2, 0.9, 0.3, 1)";
    el.style.transform = "";
    const done = () => {
      el.style.transition = "";
      el.removeEventListener("transitionend", done);
    };
    el.addEventListener("transitionend", done);
  }, [drops, element]);

  /** Whether the click that just happened ended a drag (and isn't a click); asks once. */
  const takeDragged = useCallback(() => {
    const was = dragged.current;
    dragged.current = false;
    return was;
  }, []);

  return {
    corner,
    style: offset ? { transform: `translate(${offset.x}px, ${offset.y}px)`, transition: "none" } : undefined,
    dragging: !!offset,
    onPointerDown,
    takeDragged,
  };
}
