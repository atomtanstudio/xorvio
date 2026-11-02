// What comes with an episode: the next one, to offer near the end and play
// when this one finishes, and the parts worth skipping (intro, recap, end
// credits) from IntroDB.

import { useEffect, useState } from "react";
import { playEpisode } from "../lib/actions";
import { api } from "../lib/api";
import { episodeCode } from "../lib/format";
import { useApp, type NowPlaying } from "../lib/store";
import type { Episode, EpisodeSegments, Series } from "../lib/types";
import { playCopy, similarCopy } from "../screens/Copies";

export interface NextEpisode {
  episode: Episode;
  series: Series | null;
}

/** The episode after `current`: later in its season, or the next season's first. Specials only follow specials. */
function after(episodes: Episode[], current: Episode) {
  const order = (e: Episode) => e.season * 100000 + e.number;
  return episodes
    .filter((e) => (current.season === 0 || e.season > 0) && order(e) > order(current))
    .sort((a, b) => order(a) - order(b))[0];
}

/** The series' IMDb id, which IntroDB knows episodes by. */
async function imdbOf(seriesId: string, series: Series | null) {
  if (series?.imdbId) return series.imdbId;
  const addonId = seriesId.startsWith("addon|series|") ? seriesId.slice("addon|series|".length) : null;
  if (addonId?.startsWith("tt")) return addonId;
  const details = await api.details(seriesId).catch(() => null);
  return details?.imdbId ?? null;
}

export function useEpisodeExtras(item: NowPlaying | null) {
  const [next, setNext] = useState<NextEpisode | null>(null);
  const [segments, setSegments] = useState<EpisodeSegments | null>(null);
  const itemId = item?.kind === "episode" ? item.itemId : null;
  useEffect(() => {
    setNext(null);
    setSegments(null);
    if (!itemId) return;
    const seriesId = itemId.split("|ep|")[0];
    let cancelled = false;
    (async () => {
      const [series, episodes] = await Promise.all([api.series(seriesId).catch(() => null), api.episodes(seriesId).catch(() => [] as Episode[])]);
      const current = episodes.find((e) => e.id === itemId);
      if (cancelled || !current) return;
      const following = after(episodes, current);
      setNext(following ? { episode: following, series } : null);
      // Skip times come from IntroDB, which Settings › Playback can keep out of it.
      if (!useApp.getState().settings.introDb) return;
      const imdb = await imdbOf(seriesId, series);
      if (!imdb || cancelled) return;
      const found = await api.episodeSegments(imdb, current.season, current.number).catch(() => null);
      if (!cancelled) setSegments(found);
    })();
    return () => {
      cancelled = true;
    };
  }, [itemId]);
  return { next, segments };
}

/**
 * Plays the next episode in the same presentation. An add-on episode plays
 * the copy most like the one before (same add-on and quality); when none
 * can play, its copies are offered to choose from.
 */
export async function playNextEpisode(next: NextEpisode, playing: NowPlaying) {
  const { episode, series } = next;
  if (episode.stream.type !== "addon") {
    playEpisode(episode, series);
    return;
  }
  const request = {
    mediaType: "series",
    mediaId: episode.stream.mediaId,
    itemId: episode.id,
    title: series?.title ?? playing.title,
    subtitle: `${episodeCode(episode.season, episode.number)} · ${episode.title}`,
    artwork: episode.stillUrl ?? series?.backdropUrl,
    year: series?.year,
    episode: { season: episode.season, number: episode.number },
  };
  const copies = await api.addonStreams("series", episode.stream.mediaId).catch(() => []);
  const copy = similarCopy(copies, playing.copy);
  if (copy) playCopy(copy, request);
  else useApp.getState().showCopies(request);
}
