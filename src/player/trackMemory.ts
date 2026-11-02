// Audio and subtitles chosen while watching a series, remembered for its
// other episodes, as the Swift app's TrackChoices did: a Spanish dub,
// English subtitles, or subtitles off. The library keeps them (one
// preference per series, track_choices.rs); playerRules.ts matches them to
// each episode's own tracks.

import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import type { NowPlaying } from "../lib/store";
import type { TrackChoice } from "../lib/types";

export interface TrackMemory {
  /** What's remembered for the series playing; null for none, or for a film or channel. */
  choice: TrackChoice | null;
  /** Read from the library (or nothing to read), so it can be applied before the stream starts. */
  ready: boolean;
  /** Changes what's remembered, for an episode; does nothing otherwise. */
  remember(change: (choice: TrackChoice | null) => TrackChoice | null): void;
}

export function useTrackMemory(item: NowPlaying | null): TrackMemory {
  const seriesId = item?.kind === "episode" ? (item.seriesId ?? null) : null;
  const [loaded, setLoaded] = useState<{ seriesId: string; choice: TrackChoice | null } | null>(null);
  const latest = useRef(loaded);
  latest.current = loaded;

  useEffect(() => {
    if (!seriesId || latest.current?.seriesId === seriesId) return;
    let cancelled = false;
    api
      .trackChoice(seriesId)
      .catch(() => null)
      .then((choice) => !cancelled && setLoaded({ seriesId, choice }));
    return () => {
      cancelled = true;
    };
  }, [seriesId]);

  const ready = !seriesId || loaded?.seriesId === seriesId;
  const choice = seriesId && loaded?.seriesId === seriesId ? loaded.choice : null;

  const remember = useCallback(
    (change: (choice: TrackChoice | null) => TrackChoice | null) => {
      if (!seriesId) return;
      const before = latest.current?.seriesId === seriesId ? latest.current.choice : null;
      const after = change(before);
      if (JSON.stringify(after) === JSON.stringify(before)) return;
      setLoaded({ seriesId, choice: after });
      api.setTrackChoice(seriesId, after).catch((e) => api.log("warn", `couldn't remember the series' tracks: ${e}`).catch(() => {}));
    },
    [seriesId],
  );

  return { choice, ready, remember };
}
