import { useEffect, useState } from "react";
import { ContinueChip, LandscapeCard } from "../components/Cards";
import { openMenu } from "../components/Menu";
import { continueMenu } from "../components/menus";
import { resumeContinue } from "../lib/actions";
import { api } from "../lib/api";
import { remaining } from "../lib/format";
import { useApp } from "../lib/store";
import type { ContinueItem } from "../lib/types";

export function ContinueWatchingScreen() {
  const libraryRevision = useApp((s) => s.libraryRevision);
  const [items, setItems] = useState<ContinueItem[]>([]);
  useEffect(() => {
    api.continueWatching(200).then(setItems).catch(() => {});
  }, [libraryRevision]);
  const open = resumeContinue;
  return (
    <div className="content">
      <div className="page">
        <h1 className="page-title">Continue Watching</h1>
        <div className="landscape-grid" style={{ marginTop: 18 }}>
          {items.map((item) => (
            <LandscapeCard
              key={item.itemId}
              title={item.title}
              subtitle={item.subtitle}
              imageUrl={item.artworkUrl}
              progress={item.upNext ? null : item.progress}
              progressLabel={item.remainingSeconds ? remaining(item.remainingSeconds) : null}
              chip={item.upNext ? <ContinueChip item={item} /> : null}
              onClick={() => open(item)}
              onContextMenu={(e) => openMenu(e, continueMenu(item))}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
