import { Check, CircleCheck, CircleDashed, Ellipsis, Grid2x2, ListChecks, Play, Plus, RectangleHorizontal, RotateCcw, TriangleAlert } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Art } from "../components/Artwork";
import { openMenu, type MenuEntry } from "../components/Menu";
import { ScrollRow } from "../components/ScrollRow";
import { seriesMenu } from "../components/menus";
import { openSeries, playEpisode, seriesEntry, toggleWatchlist } from "../lib/actions";
import { api, errorText } from "../lib/api";
import { duration, episodeCode, nowSeconds, remaining, shortDate } from "../lib/format";
import { useNewEpisodes } from "../lib/newEpisodes";
import { useSourceName } from "../lib/sourceNames";
import { useApp } from "../lib/store";
import type { Episode, MediaDetails, PlaybackProgress, Series } from "../lib/types";
import {
  addedDate,
  badgesFor,
  CastSection,
  Facts,
  formatText,
  InfoSection,
  MoreLikeThis,
  moreLikeThis,
  RatingsSection,
  releaseDate,
  TitleHero,
  TrailerButton,
  VideosSection,
} from "./TitleParts";

/** Every episode of a season at once, in a grid, rather than a row to scroll; kept between visits. */
const SHOW_ALL_KEY = "xorvio.episodesShowAll";
function readShowAll() {
  try {
    return localStorage.getItem(SHOW_ALL_KEY) === "1";
  } catch {
    return false;
  }
}
function keepShowAll(on: boolean) {
  try {
    localStorage.setItem(SHOW_ALL_KEY, on ? "1" : "0");
  } catch {
    // Kept for this visit only.
  }
}

export function SeriesScreen({ id }: { id: string }) {
  const [series, setSeries] = useState<Series | null>(null);
  const [details, setDetails] = useState<MediaDetails | null>(null);
  const [episodes, setEpisodes] = useState<Episode[] | null>(null);
  const [episodesError, setEpisodesError] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(readShowAll);
  const sourceName = useSourceName();
  const [progress, setProgress] = useState<Map<string, PlaybackProgress>>(new Map());
  const [season, setSeason] = useState<number | null>(null);
  const [similar, setSimilar] = useState<Series[]>([]);
  const watchlistIds = useApp((s) => s.watchlistIds);
  const nowPlaying = useApp((s) => s.nowPlaying);

  useEffect(() => {
    let cancelled = false;
    api.series(id).then((s) => {
      if (cancelled || !s) return;
      setSeries(s);
      Promise.all([api.relatedSeries(s.id).catch(() => []), api.seriesList({ categoryId: s.categoryId, sort: "rating", limit: 20, offset: 0 }).catch(() => [])]).then(
        ([suggested, sameCategory]) => !cancelled && setSimilar(moreLikeThis(s, suggested, sameCategory)),
      );
    });
    api.details(id).then((d) => !cancelled && setDetails(d)).catch(() => {});
    // Opening a series clears its "New Episode" badge.
    useNewEpisodes.getState().seen(id);
    return () => {
      cancelled = true;
    };
  }, [id]);

  // A provider that can't be reached says so, with the chance to try again.
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let cancelled = false;
    setEpisodes(null);
    setEpisodesError(null);
    api
      .episodes(id)
      .then((list) => !cancelled && setEpisodes(list))
      .catch((error) => {
        if (cancelled) return;
        setEpisodesError(errorText(error));
        setEpisodes([]);
      });
    return () => {
      cancelled = true;
    };
  }, [id, attempt]);

  const [progressLoaded, setProgressLoaded] = useState(false);
  const loadProgress = useCallback(() => {
    if (!episodes) return;
    api
      .seriesProgress(episodes.map((e) => e.id))
      .then((list) => setProgress(new Map(list.map((p) => [p.itemId, p]))))
      .catch(() => {})
      .finally(() => setProgressLoaded(true));
  }, [episodes]);

  useEffect(() => {
    if (!nowPlaying) loadProgress();
  }, [loadProgress, nowPlaying]);

  // Numbered seasons in order, specials last.
  const seasons = useMemo(() => [...new Set((episodes ?? []).map((e) => e.season))].sort((a, b) => (a === 0 ? 1 : b === 0 ? -1 : a - b)), [episodes]);

  // The episode to resume: the latest one in progress, else the one after the
  // furthest finished (a season marked watched at once shares one time).
  const next = useMemo(() => {
    if (!episodes?.length) return null;
    const touched = episodes.filter((e) => progress.has(e.id)).sort((a, b) => progress.get(b.id)!.updatedAt - progress.get(a.id)!.updatedAt)[0];
    const regular = episodes.find((e) => e.season > 0) ?? episodes[0];
    if (!touched) return { episode: regular, resume: false };
    // Under half a minute in counts as not started, as on Home.
    const p = progress.get(touched.id)!;
    if (!p.finished && p.positionSeconds > 30) return { episode: touched, resume: true };
    let last = -1;
    episodes.forEach((e, i) => progress.get(e.id)?.finished && (last = i));
    return episodes[last + 1] ? { episode: episodes[last + 1], resume: false } : null;
  }, [episodes, progress]);

  /** Marks episodes watched or unwatched, then refreshes the page and Continue Watching. */
  const mark = useCallback(
    async (list: Episode[], watched: boolean) => {
      const items = list
        .filter((e) => (progress.get(e.id)?.finished ?? false) !== watched)
        .map((e) => ({ itemId: e.id, kind: "episode" as const, sourceId: e.sourceId, seriesId: e.seriesId, positionSeconds: 0, durationSeconds: 0, finished: true, updatedAt: nowSeconds() }));
      if (!items.length) return;
      try {
        await api.setWatched(items, watched);
      } catch (error) {
        useApp.getState().showToast(errorText(error), "error");
      }
      loadProgress();
      useApp.setState((s) => ({ libraryRevision: s.libraryRevision + 1 }));
    },
    [progress, loadProgress],
  );

  // Open at the season to watch next, once progress is known, with its pill in view.
  const centredSeason = useRef(false);
  useEffect(() => {
    if (season == null && seasons.length && progressLoaded) setSeason(next?.episode.season ?? seasons[0]);
  }, [seasons, season, next, progressLoaded]);
  useEffect(() => {
    if (season == null || centredSeason.current) return;
    const chip = document.querySelector<HTMLElement>(".season-chip.selected");
    const row = chip?.parentElement;
    if (!chip || !row) return;
    centredSeason.current = true;
    row.scrollLeft = chip.offsetLeft - (row.clientWidth - chip.offsetWidth) / 2;
  }, [season]);

  useEffect(() => {
    if (series) useApp.getState().setPageTitle(id, series.title);
  }, [id, series]);

  if (!series) return <div className="content" />;
  const saved = watchlistIds.has(series.id);
  const isAddon = series.id.startsWith("addon|");
  const play = (e: Episode, fromStart = false) => playEpisode(e, series, fromStart);
  const seasonEpisodes = (episodes ?? []).filter((e) => e.season === season);
  const seasonInfo = details?.seasons.find((s) => s.number === season);
  const nextProgress = next ? progress.get(next.episode.id) : undefined;
  const watchedCount = seasonEpisodes.filter((e) => progress.get(e.id)?.finished).length;
  const seasonName = (n: number) => (n === 0 ? "Specials" : `Season ${n}`);
  const seasonWatched = (n: number) => {
    const list = (episodes ?? []).filter((e) => e.season === n);
    return list.length > 0 && list.every((e) => progress.get(e.id)?.finished);
  };
  const regularSeasons = seasons.filter((n) => n > 0).length;
  const toggleShowAll = () => {
    keepShowAll(!showAll);
    setShowAll(!showAll);
  };
  const seasonMenu = (n: number): MenuEntry[] => {
    const list = (episodes ?? []).filter((e) => e.season === n);
    return [
      { label: `Mark ${seasonName(n)} as Watched`, icon: <CircleCheck size={15} />, onSelect: () => mark(list, true) },
      { label: `Mark ${seasonName(n)} as Unwatched`, icon: <CircleDashed size={15} />, onSelect: () => mark(list, false) },
    ];
  };

  return (
    <div className="content title-page">
      <TitleHero
        backdrop={details?.backdropUrls[0] ?? series.backdropUrl ?? series.posterUrl}
        trailer={details?.trailerYoutubeId}
        logo={details?.logoUrl}
        title={series.title}
        facts={<Facts rating={series.rating} ratings={details?.ratings} items={[details?.genres.slice(0, 2).join(", ") || series.genre, series.year ?? details?.releaseDate?.slice(0, 4), seasons.length ? `${seasons.length} season${seasons.length === 1 ? "" : "s"}` : null]} badges={badgesFor(details)} />}
        overview={details?.overview ?? series.plot}
        progress={nextProgress && !nextProgress.finished ? nextProgress.positionSeconds / nextProgress.durationSeconds : null}
        actions={
          <>
            {next && (
              <button className="btn btn-primary" onClick={() => play(next.episode)}>
                <Play size={17} fill="currentColor" strokeWidth={0} /> {next.resume ? "Resume" : progress.size ? "Play" : "Play"} {episodeCode(next.episode.season, next.episode.number).replace(" ", " ")}
              </button>
            )}
            <TrailerButton youtubeId={details?.trailerYoutubeId} />
            <button className="btn btn-glass btn-icon" onClick={() => toggleWatchlist(seriesEntry(series))} aria-label={saved ? "Remove from Watchlist" : "Add to Watchlist"}>
              {saved ? <Check size={18} /> : <Plus size={19} />}
            </button>
            <button className="btn btn-icon hero-more" onClick={(e) => openMenu(e, seriesMenu(series), true)} aria-label="More">
              <Ellipsis size={20} />
            </button>
          </>
        }
      />
      <div className="title-body">
        <section className="title-section">
          <div className="section-header">
            <div>
              <h2 className="section-title">Episodes</h2>
              {season != null && (
                <p className="section-subtitle">
                  {seasonName(season)} · {seasonEpisodes.length} episode{seasonEpisodes.length === 1 ? "" : "s"}
                  {seasonInfo?.airDate ? ` · ${seasonInfo.airDate.slice(0, 4)}` : ""}
                  {watchedCount > 0 ? (watchedCount === seasonEpisodes.length ? " · All watched" : ` · ${watchedCount} of ${seasonEpisodes.length} watched`) : ""}
                </p>
              )}
            </div>
            <div className="section-actions">
              {seasonEpisodes.length > 3 && (
                <button className="link-button" onClick={toggleShowAll} title={showAll ? "Show the season as a row to scroll" : "Show every episode of the season at once"}>
                  {showAll ? <RectangleHorizontal size={15} /> : <Grid2x2 size={15} />} {showAll ? "Show as Row" : "Show All"}
                </button>
              )}
              {episodes && episodes.length > 0 && season != null && (
                <button
                  className="link-button watched-menu"
                  onClick={(e) =>
                    openMenu(
                      e,
                      [
                        ...seasonMenu(season),
                        { separator: true },
                        { label: "Mark Series as Watched", icon: <ListChecks size={15} />, onSelect: () => mark(episodes.filter((x) => x.season > 0), true) },
                        { label: "Mark Series as Unwatched", icon: <CircleDashed size={15} />, onSelect: () => mark(episodes, false) },
                      ],
                      true,
                    )
                  }
                >
                  <CircleCheck size={15} /> Watched
                </button>
              )}
            </div>
          </div>
          {episodes === null ? (
            <div className="spinner" />
          ) : episodesError && episodes.length === 0 ? (
            <div className="episodes-problem" role="alert">
              <TriangleAlert size={18} />
              <div>
                <h3>Episodes couldn't be loaded</h3>
                <p>{episodesError}</p>
                <button className="btn-plain" onClick={() => setAttempt((n) => n + 1)}>
                  Try Again
                </button>
              </div>
            </div>
          ) : episodes.length === 0 ? (
            <p className="muted">Your source didn't list any episodes for this series.</p>
          ) : (
            <>
              {seasons.length > 1 && (
                <ScrollRow className="season-chips" wrapClassName="season-wrap">
                  {seasons.map((s) => {
                    const year = details?.seasons.find((x) => x.number === s)?.airDate?.slice(0, 4);
                    return (
                      <button key={s} className={`season-chip ${s === season ? "selected" : ""}`} onClick={() => setSeason(s)} onContextMenu={(e) => openMenu(e, seasonMenu(s))}>
                        {seasonWatched(s) && <CircleCheck size={13} strokeWidth={2.4} className="season-done" aria-label="Watched" />}
                        {seasonName(s)} {year && <span>{year}</span>}
                      </button>
                    );
                  })}
                </ScrollRow>
              )}
              <EpisodeStrip
                key={season ?? -1}
                grid={showAll}
                episodes={seasonEpisodes}
                progress={progress}
                onPlay={play}
                menu={(e) => {
                  const watched = progress.get(e.id)?.finished ?? false;
                  const all = episodes ?? [];
                  const upTo = all.slice(0, all.indexOf(e) + 1).filter((x) => x.season > 0 || x.season === e.season);
                  return [
                    { heading: `${episodeCode(e.season, e.number)} · ${e.title}` },
                    { label: "Play", icon: <Play size={15} />, onSelect: () => play(e) },
                    { label: "Play from Beginning", icon: <RotateCcw size={15} />, onSelect: () => play(e, true) },
                    { separator: true },
                    watched
                      ? { label: "Mark as Unwatched", icon: <CircleDashed size={15} />, onSelect: () => mark([e], false) }
                      : { label: "Mark as Watched", icon: <CircleCheck size={15} />, onSelect: () => mark([e], true) },
                    { label: "Mark Watched Up to Here", icon: <ListChecks size={15} />, onSelect: () => mark(upTo, true) },
                  ];
                }}
              />
            </>
          )}
        </section>
        <RatingsSection ratings={details?.ratings ?? []} />
        <CastSection people={details?.people ?? []} fromId={series.id} />
        <VideosSection videos={details?.videos ?? []} trailerId={details?.trailerYoutubeId} />
        {!isAddon && <MoreLikeThis items={similar} onOpen={openSeries} />}
        <InfoSection
          rows={[
            ["Genre", details?.genres.length ? details.genres.join(", ") : series.genre],
            ["First Aired", releaseDate(details?.releaseDate) ?? series.year],
            ["Created By", details?.creators.join(", ")],
            ["Network", details?.network],
            ["Episodes", episodes?.length ? `${episodes.length} in ${regularSeasons || seasons.length} season${(regularSeasons || seasons.length) === 1 ? "" : "s"}` : null],
            ["Episode Length", details?.runtimeMinutes ? `About ${duration(details.runtimeMinutes * 60)}` : null],
            ["Original Title", details?.originalTitle && details.originalTitle.toLowerCase() !== series.title.toLowerCase() ? details.originalTitle : null],
            ["Rated", details?.ageRating],
            ["Format", formatText(details)],
            ["Source", isAddon ? null : sourceName(series.sourceId)],
            ["Added", addedDate(series.addedAt)],
            ["Details from", details?.attribution],
          ]}
        />
      </div>

    </div>
  );
}

/** A season's episodes as a row to scroll, or (`grid`) all at once. */
function EpisodeStrip({ episodes, progress, onPlay, menu, grid }: { episodes: Episode[]; progress: Map<string, PlaybackProgress>; onPlay: (e: Episode) => void; menu: (e: Episode) => MenuEntry[]; grid?: boolean }) {
  const cards = episodes.map((e) => {
    const p = progress.get(e.id);
    const fraction = p && !p.finished && p.durationSeconds > 0 ? p.positionSeconds / p.durationSeconds : null;
    return (
      <button key={e.id} className="episode-card card" onClick={() => onPlay(e)} onContextMenu={(ev) => openMenu(ev, menu(e))}>
        <Art className="episode-still" src={e.stillUrl} fallback={`Episode ${e.number}`}>
          <div className="episode-play">
            <Play size={20} fill="currentColor" strokeWidth={0} />
          </div>
          {fraction != null && (
            <div className="episode-progress">
              <div className="progress-track">
                <div style={{ width: `${fraction * 100}%` }} />
              </div>
            </div>
          )}
          {p?.finished && (
            <span className="chip watched-chip">
              <Check size={10} strokeWidth={3} /> Watched
            </span>
          )}
        </Art>
        <div className="episode-meta">
          <span className="episode-number">Episode {e.number}</span>
          {e.durationSeconds ? <span> · {Math.round(e.durationSeconds / 60)} min</span> : null}
          {e.airDate ? <span> · Aired {shortDate(e.airDate)}</span> : null}
          {fraction != null && p ? <span className="left"> · {remaining(p.durationSeconds - p.positionSeconds)}</span> : null}
        </div>
        <div className="episode-title">{e.title}</div>
        {e.plot && <div className="episode-plot">{e.plot}</div>}
      </button>
    );
  });
  if (grid) return <div className="episode-grid">{cards}</div>;
  return (
    <ScrollRow className="episode-strip" wrapClassName="episode-wrap">
      {cards}
    </ScrollRow>
  );
}
