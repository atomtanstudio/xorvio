import { Check, Circle, CircleDot, Ellipsis, Layers, Play, Plus, RotateCcw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { openMenu } from "../components/Menu";
import { movieMenu } from "../components/menus";
import { movieEntry, openMovie, playMovie, toggleWatchlist } from "../lib/actions";
import { api } from "../lib/api";
import { duration, playerClock } from "../lib/format";
import { useSourceName } from "../lib/sourceNames";
import { useApp } from "../lib/store";
import type { MediaDetails, Movie, PlaybackProgress, TitleVersion } from "../lib/types";
import { CopiesSection } from "./Copies";
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

/**
 * A film's page: its backdrop across the window with the main actions, then
 * ratings, cast, videos, more like this, the copies the sources carry (one
 * chosen to play) and the details.
 */
export function MovieScreen({ id }: { id: string }) {
  const [movie, setMovie] = useState<Movie | null>(null);
  const [details, setDetails] = useState<MediaDetails | null>(null);
  const [versions, setVersions] = useState<TitleVersion[]>([]);
  const [chosenId, setChosenId] = useState<string>(id);
  const [progress, setProgress] = useState<Map<string, PlaybackProgress>>(new Map());
  const [similar, setSimilar] = useState<Movie[]>([]);
  const watchlistIds = useApp((s) => s.watchlistIds);
  const nowPlaying = useApp((s) => s.nowPlaying);
  const libraryRevision = useApp((s) => s.libraryRevision);
  const sourceName = useSourceName();
  const versionsRef = useRef<HTMLElement>(null);

  useEffect(() => {
    let cancelled = false;
    setChosenId(id);
    api.movie(id).then((m) => {
      if (cancelled || !m) return;
      setMovie(m);
      Promise.all([api.relatedMovies(m.id).catch(() => []), api.movies({ categoryId: m.categoryId, sort: "rating", limit: 20, offset: 0 }).catch(() => [])]).then(
        ([suggested, sameCategory]) => !cancelled && setSimilar(moreLikeThis(m, suggested, sameCategory)),
      );
      if (m.stream.type !== "addon") api.movieVersions(m.id).then((list) => !cancelled && setVersions(list)).catch(() => {});
    });
    api.details(id).then((d) => !cancelled && setDetails(d)).catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [id]);

  // Progress changes after watching or marking, so reload it then, for every copy.
  const ids = versions.length ? versions.map((v) => v.movie.id) : [id];
  const idsKey = ids.join("\n");
  useEffect(() => {
    if (nowPlaying) return;
    api
      .seriesProgress(idsKey.split("\n"))
      .then((list) => setProgress(new Map(list.map((p) => [p.itemId, p]))))
      .catch(() => {});
  }, [idsKey, nowPlaying, libraryRevision]);

  useEffect(() => {
    if (movie) useApp.getState().setPageTitle(id, movie.title);
  }, [id, movie]);

  if (!movie) return <div className="content" />;
  // The copy that plays: the one chosen in Versions, else the one opened.
  const chosen = versions.find((v) => v.movie.id === chosenId)?.movie ?? movie;
  const saved = watchlistIds.has(movie.id);
  const current = progress.get(chosen.id);
  const resumable = current && !current.finished && current.positionSeconds > 30 ? current : null;
  const watched = [...progress.values()].some((p) => p.finished);
  const runtime = chosen.durationSeconds ?? movie.durationSeconds ?? (details?.runtimeMinutes ? details.runtimeMinutes * 60 : null);
  const isAddon = movie.stream.type === "addon";
  const ratings = details?.ratings ?? [];
  const genres = details?.genres.length ? details.genres.join(", ") : movie.genre;
  const chosenVersion = versions.find((v) => v.movie.id === chosen.id);

  return (
    <div className="content title-page">
      <TitleHero
        backdrop={details?.backdropUrls[0] ?? movie.backdropUrl ?? movie.posterUrl}
        trailer={details?.trailerYoutubeId}
        logo={details?.logoUrl}
        title={movie.title}
        facts={
          <Facts
            rating={movie.rating}
            ratings={ratings}
            items={[details?.genres.slice(0, 2).join(", ") || movie.genre, movie.year ?? details?.releaseDate?.slice(0, 4), runtime ? duration(runtime) : null]}
            badges={[...badgesFor(details), watched ? "WATCHED" : null]}
          />
        }
        tagline={details?.tagline}
        overview={details?.overview ?? movie.plot}
        progress={resumable ? resumable.positionSeconds / resumable.durationSeconds : null}
        actions={
          <>
            {(!isAddon || resumable) && (
              <button className="btn btn-primary" onClick={() => playMovie(chosen)}>
                <Play size={17} fill="currentColor" strokeWidth={0} /> {resumable ? `Resume · ${playerClock(resumable.positionSeconds)}` : "Play"}
              </button>
            )}
            {resumable && (
              <button className="btn btn-glass" onClick={() => playMovie(chosen, true)}>
                <RotateCcw size={16} /> Restart
              </button>
            )}
            <TrailerButton youtubeId={details?.trailerYoutubeId} />
            {versions.length > 1 && (
              <button className="btn btn-glass" onClick={() => versionsRef.current?.scrollIntoView({ behavior: "smooth", block: "start" })}>
                <Layers size={16} /> {versions.length} Versions
              </button>
            )}
            <button className="btn btn-glass btn-icon" onClick={() => toggleWatchlist(movieEntry(movie))} aria-label={saved ? "Remove from Watchlist" : "Add to Watchlist"} title={saved ? "Remove from Watchlist" : "Add to Watchlist"}>
              {saved ? <Check size={18} /> : <Plus size={19} />}
            </button>
            <button className="btn btn-icon hero-more" onClick={(e) => openMenu(e, movieMenu(chosen, { watched }), true)} aria-label="More">
              <Ellipsis size={20} />
            </button>
          </>
        }
      />
      <div className="title-body">
        {isAddon && <CopiesSection mediaType="movie" mediaId={movie.stream.type === "addon" ? movie.stream.mediaId : ""} title={movie.title} artwork={movie.backdropUrl} itemId={movie.id} year={movie.year} resumeSeconds={resumable ? resumable.positionSeconds : null} />}
        <RatingsSection ratings={ratings} />
        <CastSection people={details?.people ?? []} fromId={movie.id} />
        <VideosSection videos={details?.videos ?? []} trailerId={details?.trailerYoutubeId} />
        <MoreLikeThis items={similar} onOpen={openMovie} />
        {versions.length > 1 && (
          <section className="title-section" ref={versionsRef}>
            <h2 className="section-title">Versions</h2>
            <p className="section-subtitle">This film is available from more than one place. Choose which to play.</p>
            <div className="versions-list" role="radiogroup">
              {versions.map((v) => {
                const selected = v.movie.id === chosen.id;
                const p = progress.get(v.movie.id);
                const part = p && !p.finished && p.durationSeconds > 0 && p.positionSeconds > 30 ? Math.round((p.positionSeconds / p.durationSeconds) * 100) : null;
                return (
                  <button key={v.movie.id} className={`version-option ${selected ? "selected" : ""}`} onClick={() => setChosenId(v.movie.id)} role="radio" aria-checked={selected}>
                    {selected ? <CircleDot size={17} className="version-radio" /> : <Circle size={17} className="version-radio" />}
                    <span className="version-text">
                      <div className="version-title">{[v.movie.title, v.categoryName].filter(Boolean).join(" — ")}</div>
                      <div className="version-where">{v.sourceName}</div>
                    </span>
                    <span className="version-facts">
                      {part != null && <span>{part}% watched</span>}
                      {p?.finished && <span>Watched</span>}
                      {v.quality && <span className="badge">{v.quality}</span>}
                      {v.extension && <span className="badge">{v.extension.toUpperCase()}</span>}
                    </span>
                  </button>
                );
              })}
            </div>
          </section>
        )}
        <InfoSection
          rows={[
            ["Genre", genres],
            ["Released", releaseDate(details?.releaseDate) ?? movie.year],
            ["Runtime", runtime ? duration(runtime) : null],
            ["Director", details?.director],
            ["Country", details?.country],
            ["Original Title", details?.originalTitle && details.originalTitle.toLowerCase() !== movie.title.toLowerCase() ? details.originalTitle : null],
            ["Rated", details?.ageRating],
            ["Format", formatText(details)],
            ["Source", isAddon ? null : [chosenVersion?.sourceName ?? sourceName(chosen.sourceId), chosenVersion?.categoryName].filter(Boolean).join(" · ")],
            ["Added", addedDate(chosen.addedAt)],
            ["Details from", details?.attribution],
          ]}
        />
      </div>
    </div>
  );
}
