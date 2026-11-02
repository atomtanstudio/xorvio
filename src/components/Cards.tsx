import { Bookmark, Check, Layers, Play, Sparkles } from "lucide-react";
import type { ReactNode } from "react";
import type { ContinueItem, UpNextReason } from "../lib/types";
import { Art } from "./Artwork";

interface LandscapeCardProps {
  title: string;
  subtitle?: string | null;
  imageUrl?: string | null;
  progress?: number | null;
  progressLabel?: string | null;
  chip?: ReactNode;
  corner?: ReactNode;
  onClick?: () => void;
  onContextMenu?: (e: React.MouseEvent) => void;
}

/** The 16:9 card used by Continue Watching, the watchlist and live rows. */
export function LandscapeCard({ title, subtitle, imageUrl, progress, progressLabel, chip, corner, onClick, onContextMenu }: LandscapeCardProps) {
  return (
    <button className="card landscape-card" onClick={onClick} onContextMenu={onContextMenu}>
      <Art className="card-art" src={imageUrl} fallback={title}>
        {chip && <div className="card-chip">{chip}</div>}
        {corner && <div className="card-corner">{corner}</div>}
        <div className="card-caption">
          <div className="card-title">{title}</div>
          {subtitle && <div className="card-subtitle">{subtitle}</div>}
          {progress != null && progress > 0 && (
            <div className="card-progress">
              <div className="progress-track">
                <div style={{ width: `${Math.min(100, progress * 100)}%` }} />
              </div>
              {progressLabel && <span>{progressLabel}</span>}
            </div>
          )}
        </div>
      </Art>
    </button>
  );
}

interface PosterCardProps {
  title: string;
  meta?: string | null;
  posterUrl?: string | null;
  progress?: number | null;
  saved?: boolean;
  /** Watched to the end (or marked so). */
  watched?: boolean;
  /** Copies of the film the sources carry, badged when there's more than one. */
  versions?: number;
  onClick?: () => void;
  onContextMenu?: (e: React.MouseEvent) => void;
  hideText?: boolean;
  /** Load at once rather than when scrolled near; for short lists. */
  eager?: boolean;
  /** Over the poster's corner: "New Episode". */
  badge?: ReactNode;
}

/** A 2:3 poster with the title and year below. */
export function PosterCard({ title, meta, posterUrl, progress, saved, watched, versions, onClick, onContextMenu, hideText, eager, badge }: PosterCardProps) {
  return (
    <button className="card poster-card" onClick={onClick} onContextMenu={onContextMenu} title={hideText ? title : undefined}>
      <Art className="card-art" src={posterUrl} fallback={title} eager={eager}>
        {badge && <div className="card-chip">{badge}</div>}
        {saved && <Bookmark className="bookmark" size={16} fill="currentColor" />}
        {(watched || (versions ?? 0) > 1) && (
          <div className="poster-chips">
            {watched && (
              <span className="chip watched">
                <Check size={10} strokeWidth={3} /> Watched
              </span>
            )}
            {(versions ?? 0) > 1 && (
              <span className="chip">
                <Layers size={10} strokeWidth={2.4} /> {versions} versions
              </span>
            )}
          </div>
        )}
        {progress != null && progress > 0 && (
          <div className="poster-progress">
            <div className="progress-track accent">
              <div style={{ width: `${Math.min(100, progress * 100)}%` }} />
            </div>
          </div>
        )}
      </Art>
      {!hideText && (
        <>
          <div className="card-title">{title}</div>
          {meta && <div className="card-meta">{meta}</div>}
        </>
      )}
    </button>
  );
}

/** Netflix-style ranked poster: an outlined numeral behind the poster's edge. */
export function Top10Card({ rank, ...poster }: PosterCardProps & { rank: number }) {
  return (
    <div className="top10-card">
      <div className="rank" aria-hidden="true">
        {rank}
      </div>
      <PosterCard {...poster} hideText />
    </div>
  );
}

export function UpNextChip({ label }: { label: string }) {
  return (
    <span className="chip">
      <Play size={9} fill="currentColor" strokeWidth={0} /> {label}
    </span>
  );
}

/** "New Episode" on a series with episodes not looked at yet, or a Continue Watching card. */
export function NewEpisodeChip({ label = "New Episode" }: { label?: string }) {
  return (
    <span className="chip chip-new">
      <Sparkles size={10} strokeWidth={2.4} /> {label}
    </span>
  );
}

const UP_NEXT_LABELS: Record<UpNextReason, string> = { newEpisode: "New Episode", next: "Up Next", latest: "Latest Episode", start: "Start Watching" };

/** What a Continue Watching card says about an episode not started yet, as Xorvio for Mac chose. */
export function ContinueChip({ item }: { item: ContinueItem }) {
  if (item.upNext) return item.reason === "newEpisode" ? <NewEpisodeChip /> : <UpNextChip label={UP_NEXT_LABELS[item.reason ?? "next"]} />;
  return item.progress === 0 ? <UpNextChip label="Start Watching" /> : null;
}
