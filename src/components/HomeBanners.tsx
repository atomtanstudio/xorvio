// Notes at the top of Home's rows, as Xorvio for Mac's banners: a source
// that couldn't refresh (with Review, to Sources), and the demo library
// alone. They go once the reason does; there's nothing to dismiss.

import { Sparkles, TriangleAlert, UserRoundX } from "lucide-react";
import type { ReactNode } from "react";
import { useApp } from "../lib/store";
import type { SourceSummary } from "../lib/types";

function Banner({ icon, tone, text, action, onAction }: { icon: ReactNode; tone: "warning" | "accent"; text: string; action: string; onAction: () => void }) {
  return (
    <div className={`home-banner ${tone}`} role="status">
      <span className="home-banner-icon">{icon}</span>
      <span className="home-banner-text">{text}</span>
      <button className="btn-plain" onClick={onAction}>
        {action}
      </button>
    </div>
  );
}

export function HomeBanners({ sources }: { sources: SourceSummary[] | null }) {
  const go = useApp((s) => s.go);
  if (!sources?.length) return null;
  const failing = sources.filter((s) => !s.config.isPaused && s.status.lastError);
  const demoOnly = sources.every((s) => s.config.kind === "demo");
  if (!failing.length && !demoOnly) return null;
  return (
    <div className="home-banners">
      {failing.map((source) => (
        <Banner
          key={source.config.id}
          tone="warning"
          icon={source.status.authenticationExpired ? <UserRoundX size={16} strokeWidth={2} /> : <TriangleAlert size={16} strokeWidth={2} />}
          text={`${source.config.name} couldn't refresh. You're seeing its last successful catalogue.`}
          action="Review"
          onAction={() => go("sources")}
        />
      ))}
      {demoOnly && (
        <Banner
          tone="accent"
          icon={<Sparkles size={16} strokeWidth={2} />}
          text="You're exploring demonstration data. Add a folder of your own videos, or your own playlist or provider."
          action="Add Source"
          onAction={() => go("sources")}
        />
      )}
    </div>
  );
}
