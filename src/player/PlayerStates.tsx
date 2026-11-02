// What the player says over the video when it isn't simply playing: a
// stream being opened again after it dropped (with a countdown and Try Now),
// and one that can't play, by kind, with what to do about it.

import { Film, KeyRound, SearchX, TriangleAlert, WifiOff } from "lucide-react";
import { useEffect, useState } from "react";
import { reconnectStatus, type Failure } from "./playerRules";
import { reconnectNow } from "./recovery";

export function ReconnectingCard({ attempt, of, nextAt, compact }: { attempt: number; of: number; nextAt: number | null; compact: boolean }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (nextAt === null) return;
    const timer = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(timer);
  }, [nextAt]);
  return (
    <div className="player-center player-state">
      <div className={`player-state-card ${compact ? "compact" : "glass"}`} role="status">
        <div className="spinner" />
        <strong>Reconnecting…</strong>
        {!compact && <p className="tabular">{reconnectStatus(attempt, of, nextAt === null ? null : (nextAt - now) / 1000)}</p>}
        {nextAt !== null && (
          <button className="btn btn-glass btn-small" onClick={reconnectNow}>
            Try Now
          </button>
        )}
      </div>
    </div>
  );
}

const ICONS: Partial<Record<Failure["kind"], typeof Film>> = { offline: WifiOff, signIn: KeyRound, notFound: SearchX, unsupported: Film };

interface FailureProps {
  failure: Failure;
  /** The title that can't play. */
  title: string;
  /** Docked or in the corner: the headline and Try Again only. */
  compact: boolean;
  onRetry: () => void;
  onClose: () => void;
  /** Sources, where the account or the source can be put right; left out where there's nothing there to do. */
  onSources?: (() => void) | null;
  /** For an add-on title: another of its copies instead. */
  onChooseCopy?: (() => void) | null;
}

export function FailureCard({ failure, title, compact, onRetry, onClose, onSources, onChooseCopy }: FailureProps) {
  const Icon = ICONS[failure.kind] ?? TriangleAlert;
  return (
    <div className="player-center player-state">
      <div className={`player-state-card failed ${compact ? "compact" : "glass"}`} role="alert">
        <Icon size={compact ? 20 : 30} className="player-state-icon" />
        <strong>{failure.title}</strong>
        {!compact && (
          <>
            <p>
              Can't play {title}. {failure.message}
            </p>
            <p className="suggestion">{failure.suggestion}</p>
          </>
        )}
        <div className="player-state-actions">
          <button className="btn btn-glass btn-small" onClick={onRetry}>
            Try Again
          </button>
          {!compact && onChooseCopy && (
            <button className="btn btn-glass btn-small" onClick={onChooseCopy}>
              Choose Another Copy
            </button>
          )}
          {!compact && !onChooseCopy && onSources && (
            <button className="btn btn-glass btn-small" onClick={onSources}>
              Open Sources
            </button>
          )}
          {!compact && (
            <button className="btn btn-glass btn-small" onClick={onClose}>
              Close
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
