// A Trakt chart, list or recommendations in full, opened from its row's
// Show All: every title of it the sources carry or an add-on can play, in
// Trakt's order. A list's titles show their place in it (the IMDb Top 250's
// ranks, say).

import { useEffect, useState } from "react";
import { PosterCard } from "../components/Cards";
import { openMenu } from "../components/Menu";
import { movieMenu, seriesMenu } from "../components/menus";
import { openMovie, openSeries } from "../lib/actions";
import { api, errorText } from "../lib/api";
import { firstGenre } from "../lib/format";
import { traktNote, traktPageTitle } from "../lib/homeLayout";
import { useApp } from "../lib/store";
import type { TraktRow, TraktRowRef } from "../lib/types";

/** The most titles a page asks for: Trakt's longest lists are read this far. */
const MOST = 1000;

export function TraktListScreen({ trakt }: { trakt: TraktRowRef }) {
  const libraryRevision = useApp((s) => s.libraryRevision);
  const addonsLead = useApp((s) => s.settings.addonsLead);
  const watchlistIds = useApp((s) => s.watchlistIds);
  const [row, setRow] = useState<TraktRow | null>(null);
  const [error, setError] = useState<string | null>(null);
  const sourceKey = JSON.stringify(trakt.source);
  useEffect(() => {
    let cancelled = false;
    api
      .traktRow(trakt.source, MOST)
      .then((found) => {
        if (cancelled) return;
        setRow(found);
        setError(null);
      })
      .catch((e) => !cancelled && setError(errorText(e)));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sourceKey, libraryRevision, addonsLead]);

  const ranked = trakt.source.type === "list";
  const count = row && row.listed > 0 ? `${row.items.length.toLocaleString()} of ${row.listed.toLocaleString()} you can watch` : null;
  return (
    <div className="content">
      <div className="page">
        <h1 className="page-title">{traktPageTitle(trakt)}</h1>
        <p className="page-subtitle">{[traktNote(trakt), count].filter(Boolean).join(" · ")}</p>
        {row && row.items.length > 0 && (
          <div className="poster-grid" style={{ marginTop: 22 }}>
            {row.items.map((item) => {
              const title = "movie" in item ? item.movie : item.series;
              const open = () => ("movie" in item ? openMovie(item.movie) : openSeries(item.series));
              const menu = (e: React.MouseEvent) => openMenu(e, "movie" in item ? movieMenu(item.movie) : seriesMenu(item.series));
              const meta = [ranked ? `No. ${item.rank}` : null, title.year, firstGenre(title.genre)].filter(Boolean).join(" · ");
              return (
                <PosterCard key={title.id} title={title.title} meta={meta} posterUrl={title.posterUrl} saved={watchlistIds.has(title.id)} onClick={open} onContextMenu={menu} />
              );
            })}
          </div>
        )}
        {row && !row.items.length && (
          <div className="empty-state">
            <div>
              <h3>Nothing here to watch yet</h3>
              <p>None of this list's titles are in your sources, and none of your add-ons can play them.</p>
            </div>
          </div>
        )}
        {!row && error && (
          <div className="empty-state">
            <div>
              <h3>Trakt couldn't be reached</h3>
              <p>{error}</p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
