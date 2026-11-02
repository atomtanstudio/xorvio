// Pieces shared by the film and series pages: the full-window hero (still,
// with the trailer behind it: TitleTrailer.tsx), the trailer button, ratings, cast, videos, "more like this" and the
// information table.

import { Film, Play, Star } from "lucide-react";
import type { ReactNode } from "react";
import { Art, placeholderGradient } from "../components/Artwork";
import { PosterCard } from "../components/Cards";
import { ScrollRow } from "../components/ScrollRow";
import { api, errorText } from "../lib/api";
import { initials } from "../lib/format";
import { imageSrc } from "../lib/images";
import { useApp } from "../lib/store";
import type { CastMember, MediaDetails, RatingSource, TitleRating, VideoLink } from "../lib/types";
import { TitleTrailer } from "./TitleTrailer";
import "../styles/browse.css";

export function TitleHero({
  backdrop,
  trailer,
  logo,
  title,
  facts,
  tagline,
  overview,
  actions,
  progress,
}: {
  backdrop?: string | null;
  /** The trailer's YouTube id, played behind the page. */
  trailer?: string | null;
  logo?: string | null;
  title: string;
  facts: ReactNode;
  tagline?: string | null;
  overview?: string | null;
  actions: ReactNode;
  progress?: number | null;
}) {
  return (
    <section className="title-hero">
      <div className="title-hero-backdrop">
        <Art src={backdrop} eager />
      </div>
      <TitleTrailer youtubeId={trailer} />
      <div className="title-hero-content">
        {logo ? <img className="hero-logo" src={imageSrc(logo)} alt={title} /> : <h1 className={`hero-title ${title.length > 24 ? "long" : ""}`}>{title}</h1>}
        <div className="hero-facts">{facts}</div>
        {tagline && <p className="hero-tagline">{tagline}</p>}
        {overview && <p className="hero-overview title-overview">{overview}</p>}
        <div className="hero-actions">{actions}</div>
        {progress != null && progress > 0 && (
          <div className="hero-progress">
            <div className="progress-track">
              <div style={{ width: `${Math.min(100, progress * 100)}%` }} />
            </div>
          </div>
        )}
      </div>
    </section>
  );
}

/** What a YouTube video id looks like (providers and add-ons pass on whatever they're given). */
const YOUTUBE_ID = /^[\w-]{6,20}$/;

/**
 * Opens a title's trailer on YouTube in the default browser. YouTube's
 * embedded player refuses pages that aren't served over http(s), as the
 * app's own aren't, so it doesn't play in the window, and nothing plays
 * until it's asked for.
 */
export function TrailerButton({ youtubeId }: { youtubeId?: string | null }) {
  if (!youtubeId || !YOUTUBE_ID.test(youtubeId)) return null;
  const open = () => openOnYouTube(youtubeId);
  return (
    <button className="btn btn-glass" onClick={open} title="Watch the trailer on YouTube">
      <Film size={16} /> Trailer
    </button>
  );
}

function openOnYouTube(youtubeId: string) {
  api.openExternal(`https://www.youtube.com/watch?v=${youtubeId}`).catch((e) => useApp.getState().showToast(errorText(e), "error"));
}

/**
 * Genre, year and length after the title's scores: the leading two from
 * where the details came, or the source's own rating when there are none.
 */
export function Facts({ rating, ratings = [], items, badges }: { rating?: number | null; ratings?: TitleRating[]; items: (string | number | null | undefined)[]; badges: (string | null | undefined)[] }) {
  const shown = items.filter((i) => i !== null && i !== undefined && i !== "");
  return (
    <>
      {ratings.length ? ratings.slice(0, 2).map((r) => <RatingChip key={r.source} rating={r} />) : rating ? <span className="rating-pill">★ {rating.toFixed(1)}</span> : null}
      {shown.map((item, i) => (
        <span key={i} className={i > 0 ? "dot-sep" : ""}>
          {item}
        </span>
      ))}
      <span className="badges">
        {badges.filter(Boolean).map((b) => (
          <span key={b} className="badge">
            {b}
          </span>
        ))}
      </span>
    </>
  );
}

const RATING_NAMES: Record<RatingSource, string> = { tmdb: "TMDB", imdb: "IMDb" };

/** A score with who gave it. Other companies' logos aren't used; a star stands for all of them. */
function RatingChip({ rating }: { rating: TitleRating }) {
  return (
    <span className="rating-chip" aria-label={`${RATING_NAMES[rating.source]} ${rating.score.toFixed(1)}`}>
      <Star size={10} fill="currentColor" strokeWidth={0} /> {RATING_NAMES[rating.source]} <strong>{rating.score.toFixed(1)}</strong>
    </span>
  );
}

const votes = new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 });

export function RatingsSection({ ratings }: { ratings: TitleRating[] }) {
  if (!ratings.length) return null;
  return (
    <section className="title-section">
      <h2 className="section-title">Ratings</h2>
      <div className="ratings-row">
        {ratings.map((r) => (
          <div key={r.source} className="rating-block">
            <div className="rating-source">
              <Star size={11} fill="currentColor" strokeWidth={0} /> {RATING_NAMES[r.source]}
            </div>
            <div className="rating-value">
              {r.score.toFixed(1)}
              <span className="muted small"> / 10</span>
            </div>
            {r.votes ? <div className="rating-votes">{votes.format(r.votes)} votes</div> : null}
          </div>
        ))}
      </div>
    </section>
  );
}

/** Trailers, teasers and clips as thumbnails; each opens on YouTube, as the Trailer button does. Left out when the trailer is all there is. */
export function VideosSection({ videos, trailerId }: { videos: VideoLink[]; trailerId?: string | null }) {
  const playable = videos.filter((v) => YOUTUBE_ID.test(v.youtubeId));
  if (!playable.length || (playable.length === 1 && playable[0].youtubeId === trailerId)) return null;
  return (
    <section className="title-section">
      <h2 className="section-title">Videos</h2>
      <p className="section-subtitle">Trailers and clips open on YouTube</p>
      <ScrollRow className="poster-strip" wrapClassName="strip-wrap videos">
        {playable.map((v) => (
          <button key={v.youtubeId} className="video-card card" onClick={() => openOnYouTube(v.youtubeId)} title="Open on YouTube">
            <Art className="video-thumb" src={`https://i.ytimg.com/vi/${v.youtubeId}/hqdefault.jpg`} fallback={v.title}>
              <div className="video-play">
                <span>
                  <Play size={18} fill="currentColor" strokeWidth={0} />
                </span>
              </div>
            </Art>
            <div className="video-title">{v.title}</div>
            {v.kind && <div className="video-kind">{v.kind}</div>}
          </button>
        ))}
      </ScrollRow>
    </section>
  );
}

const longDate = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "long", year: "numeric" });
const mediumDate = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short", year: "numeric" });

/** "1999-03-30" as the system writes a date; the text itself when it isn't one. */
export function releaseDate(text: string | null | undefined) {
  const match = text?.match(/^(\d{4})-(\d{2})-(\d{2})/);
  return match ? longDate.format(new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]))) : (text ?? null);
}

/** When a title was added to its source. */
export function addedDate(at: number | null | undefined) {
  return at ? mediumDate.format(new Date(at * 1000)) : null;
}

/** "3840×2160 · HEVC · HDR · 5.1 audio". */
export function formatText(details: MediaDetails | null) {
  const video = details?.video;
  if (!video) return null;
  const channels = details?.audioChannels;
  const audio = channels ? (channels >= 6 ? `${channels === 8 ? "7.1" : "5.1"} audio` : `${channels}-channel audio`) : null;
  return [`${video.width}×${video.height}`, video.codec?.toUpperCase(), video.isHdr ? "HDR" : null, audio].filter(Boolean).join(" · ");
}

export function badgesFor(details: MediaDetails | null) {
  if (!details) return [];
  const h = details.video?.height ?? 0;
  const channels = details.audioChannels ?? 0;
  return [details.ageRating, h >= 2000 ? "4K" : h >= 1000 ? "HD" : null, details.video?.isHdr ? "HDR" : null, channels === 6 ? "5.1" : channels === 8 ? "7.1" : null];
}

export function CastSection({ people, fromId }: { people: CastMember[]; fromId: string }) {
  const push = useApp((s) => s.push);
  if (!people.length) return null;
  return (
    <section className="title-section">
      <h2 className="section-title">Cast</h2>
      <p className="section-subtitle">Choose someone to see their other work</p>
      <ScrollRow className="cast-row" wrapClassName="strip-wrap cast">
        {people.slice(0, 16).map((p) => (
          <button key={p.name} className="cast-member" onClick={() => push({ type: "person", name: p.name, tmdbId: p.tmdbId, fromId })}>
            <div className="cast-photo" style={{ background: placeholderGradient(p.name) }}>
              {p.photoUrl ? <img src={imageSrc(p.photoUrl)} alt="" loading="lazy" /> : <span>{initials(p.name)}</span>}
            </div>
            <div className="cast-name">{p.name}</div>
            {p.role && <div className="cast-role">{p.role}</div>}
          </button>
        ))}
      </ScrollRow>
    </section>
  );
}

/** TMDB's suggestions first, then the same category's best rated, each title once and never the title itself. */
export function moreLikeThis<T extends { id: string; title: string; year: number | null }>(self: T, suggested: T[], sameCategory: T[], limit = 20): T[] {
  const key = (item: T) => `${item.title.toLowerCase()}|${item.year ?? ""}`;
  const seen = new Set([key(self)]);
  const chosen: T[] = [];
  for (const item of [...suggested, ...sameCategory]) {
    if (chosen.length === limit) break;
    if (item.id === self.id || seen.has(key(item))) continue;
    seen.add(key(item));
    chosen.push(item);
  }
  return chosen;
}

export function MoreLikeThis<T extends { id: string; title: string; year: number | null; posterUrl: string | null; genre: string | null }>({ items, onOpen }: { items: T[]; onOpen: (item: T) => void }) {
  if (!items.length) return null;
  return (
    <section className="title-section">
      <h2 className="section-title">More Like This</h2>
      <ScrollRow className="poster-strip" wrapClassName="strip-wrap posters">
        {items.map((item) => (
          <PosterCard key={item.id} title={item.title} meta={[item.year, item.genre?.split(",")[0]].filter(Boolean).join(" · ")} posterUrl={item.posterUrl} onClick={() => onOpen(item)} />
        ))}
      </ScrollRow>
    </section>
  );
}

export function InfoSection({ rows }: { rows: [string, ReactNode][] }) {
  const shown = rows.filter(([, v]) => v !== null && v !== undefined && v !== "");
  if (!shown.length) return null;
  return (
    <section className="title-section">
      <h2 className="section-title">Information</h2>
      <dl className="info-grid">
        {shown.map(([k, v]) => (
          <div key={k}>
            <dt>{k}</dt>
            <dd>{v}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
