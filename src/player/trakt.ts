// Telling Trakt what's playing. Trakt wants to hear when a film or episode
// starts, pauses and stops, with how far through it is, and marks it watched
// itself when it stops past 80%. The backend checks that Trakt is connected
// and scrobbling is on, and quietly does nothing otherwise, so this just
// reports what the video does.

import { useEffect, useRef, type RefObject } from "react";
import { api } from "../lib/api";
import type { NowPlaying } from "../lib/store";
import type { TraktScrobbleAction } from "../lib/types";

export function useTraktScrobble(item: NowPlaying | null, video: RefObject<HTMLVideoElement | null>, position: number, duration: number) {
  // Read when an event comes, so the effect doesn't restart as time passes.
  const latest = useRef({ position, duration });
  latest.current = { position, duration };
  const itemId = item?.itemId;
  const kind = item?.kind;

  useEffect(() => {
    const el = video.current;
    if (!el || !itemId || (kind !== "movie" && kind !== "episode")) return;
    const percent = () => {
      const { position, duration } = latest.current;
      return duration > 0 ? Math.min(100, Math.max(0, (position / duration) * 100)) : 0;
    };
    // Buffering and a remuxed stream restarting both fire "playing" again;
    // Trakt only needs to hear when something changed.
    let last: TraktScrobbleAction | null = null;
    const send = (action: TraktScrobbleAction, progress = percent()) => {
      if (action === last) return;
      last = action;
      api.traktScrobble(itemId, kind, action, progress).catch(() => {});
    };
    const onPlaying = () => send("start");
    const onPause = () => {
      if (last === "start" && !el.ended) send("pause");
    };
    const onEnded = () => send("stop", 100);
    el.addEventListener("playing", onPlaying);
    el.addEventListener("pause", onPause);
    el.addEventListener("ended", onEnded);
    return () => {
      el.removeEventListener("playing", onPlaying);
      el.removeEventListener("pause", onPause);
      el.removeEventListener("ended", onEnded);
      // Closed, or on to another title: stopped where it was.
      if (last && last !== "stop") send("stop");
    };
  }, [itemId, kind, video]);
}
