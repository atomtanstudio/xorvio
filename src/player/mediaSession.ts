// Now Playing and the media keys. While the video plays, WebKit makes Xorvio
// the Now Playing app (Control Center, the menu bar) and passes the
// keyboard's play/pause, next and previous keys to navigator.mediaSession,
// so this says what's playing and handles those keys; nothing native is
// needed. When the player closes, WebKit drops Xorvio from Now Playing.

import { useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { nowSeconds } from "../lib/format";
import { imageSrc } from "../lib/images";
import type { NowPlaying } from "../lib/store";
import { pauseByPerson } from "./pause";

interface MediaSessionProps {
  item: NowPlaying | null;
  video: React.RefObject<HTMLVideoElement | null>;
  isLive: boolean;
  paused: boolean;
  /** In the title's own timeline (a remuxed stream's element starts at its offset). */
  position: number;
  duration: number;
  speed: number;
  onSeek: (seconds: number) => void;
  /** Next and previous: channel up and down, or the next episode. Null where there's none, which hides the button. */
  onNext: (() => void) | null;
  onPrevious: (() => void) | null;
  /** Play and pause somewhere other than the video: Multiview's tiles, or a TV being cast to. */
  onPlay?: () => void;
  onPause?: () => void;
}

const ACTIONS: MediaSessionAction[] = ["play", "pause", "seekto", "seekbackward", "seekforward", "nexttrack", "previoustrack"];

// A web view without it simply has no Now Playing.
const session = typeof navigator !== "undefined" && "mediaSession" in navigator ? navigator.mediaSession : null;

export function useMediaSession({ item, video, isLive, paused, position, duration, speed, onSeek, onNext, onPrevious, onPlay, onPause }: MediaSessionProps) {
  const programme = useProgrammeOnNow(isLive && item?.channel ? item.channel.id : null);

  // The title, then the episode, the year or the programme on now.
  useEffect(() => {
    if (!session) return;
    if (!item) {
      session.metadata = null;
      return;
    }
    const artist = (isLive && item.channel ? programme : item.subtitle) ?? "";
    const artwork = item.artworkUrl ?? item.channel?.logoUrl;
    try {
      session.metadata = new MediaMetadata({ title: item.title, artist, artwork: artwork ? [{ src: imageSrc(artwork) }] : [] });
    } catch {
      // Artwork with an address the web view can't read: the words alone.
      session.metadata = new MediaMetadata({ title: item.title, artist });
    }
  }, [item, isLive, programme]);

  useEffect(() => {
    if (session) session.playbackState = !item ? "none" : paused ? "paused" : "playing";
  }, [item, paused]);

  // Where it is, every few seconds and whenever it pauses or jumps; the
  // system moves the time along in between. Live channels have no length.
  const at = useRef(position);
  at.current = position;
  const step = Math.floor(position / 5);
  useEffect(() => {
    if (!session?.setPositionState) return;
    try {
      if (!item || isLive || !duration) session.setPositionState();
      else session.setPositionState({ duration, position: Math.min(Math.max(0, at.current), duration), playbackRate: speed });
    } catch {
      // A position the web view refuses; Now Playing shows no time.
    }
  }, [item, isLive, duration, speed, paused, step]);

  // The keys, which read the latest player state when pressed.
  const latest = useRef({ video, onSeek, onNext, onPrevious, onPlay, onPause });
  latest.current = { video, onSeek, onNext, onPrevious, onPlay, onPause };
  const hasNext = !!onNext;
  const hasPrevious = !!onPrevious;
  useEffect(() => {
    if (!session || !item) return;
    const set = (action: MediaSessionAction, handler: MediaSessionActionHandler | null) => {
      try {
        session.setActionHandler(action, handler);
      } catch {
        // An action this web view doesn't know.
      }
    };
    const now = () => latest.current;
    set("play", () => {
      const { onPlay, video } = now();
      if (onPlay) onPlay();
      else video.current?.play().catch(() => {});
    });
    set("pause", () => {
      const { onPause, video } = now();
      if (onPause) onPause();
      else pauseByPerson(video.current);
    });
    // A live channel can't seek; without handlers the system leaves the controls out.
    set("seekto", isLive ? null : (d) => d.seekTime != null && now().onSeek(d.seekTime));
    set("seekbackward", isLive ? null : (d) => now().onSeek(at.current - (d.seekOffset ?? 10)));
    set("seekforward", isLive ? null : (d) => now().onSeek(at.current + (d.seekOffset ?? 10)));
    set("nexttrack", hasNext ? () => now().onNext?.() : null);
    set("previoustrack", hasPrevious ? () => now().onPrevious?.() : null);
    return () => ACTIONS.forEach((action) => set(action, null));
  }, [item, isLive, hasNext, hasPrevious]);
}

/** The title of the programme on a live channel now, checked each minute. */
function useProgrammeOnNow(channelId: string | null) {
  const [title, setTitle] = useState<string | null>(null);
  useEffect(() => {
    setTitle(null);
    if (!channelId) return;
    let cancelled = false;
    const load = () =>
      api
        .nowNext([channelId])
        .then(([row]) => {
          const t = nowSeconds();
          if (!cancelled) setTitle(row?.programs.find((p) => p.start <= t && t < p.end)?.title ?? null);
        })
        .catch(() => {});
    load();
    const timer = setInterval(load, 60_000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [channelId]);
  return title;
}
