import { Compass } from "lucide-react";
import { useEffect, useState } from "react";
import { PosterCard } from "../components/Cards";
import { openMenu } from "../components/Menu";
import { movieMenu, seriesMenu } from "../components/menus";
import { Shelf } from "../components/Shelf";
import { openMovie, openSeries } from "../lib/actions";
import { api, errorText } from "../lib/api";
import { firstGenre } from "../lib/format";
import { useApp } from "../lib/store";
import type { AddonCatalogRow } from "../lib/types";

/** Every browsable catalogue from the installed add-ons, one row each. */
export function DiscoverScreen() {
  const [rows, setRows] = useState<AddonCatalogRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const watchlistIds = useApp((s) => s.watchlistIds);
  useEffect(() => {
    api.discoverRows().then(setRows).catch((e) => setError(errorText(e)));
  }, []);

  return (
    <div className="content">
      <div className="page" style={{ paddingBottom: 0 }}>
        <h1 className="page-title">Discover</h1>
        <p className="page-subtitle">Catalogues from your add-ons.</p>
      </div>
      {error && <p className="muted page">{error}</p>}
      {rows === null && !error && <div className="spinner" style={{ margin: 40 }} />}
      {rows?.length === 0 && (
        <div className="empty-state">
          <div>
            <Compass size={40} strokeWidth={1.4} />
            <h3>No catalogues</h3>
            <p>Install an add-on with catalogues in Settings › Add-ons.</p>
          </div>
        </div>
      )}
      {rows?.map((row) => (
        <Shelf key={`${row.addonId}/${row.mediaType}/${row.catalogId}`} title={row.title}>
          {row.movies.map((m) => (
            <PosterCard key={m.id} title={m.title} meta={[m.year, firstGenre(m.genre)].filter(Boolean).join(" · ")} posterUrl={m.posterUrl} saved={watchlistIds.has(m.id)} onClick={() => openMovie(m)} onContextMenu={(e) => openMenu(e, movieMenu(m))} />
          ))}
          {row.series.map((s) => (
            <PosterCard key={s.id} title={s.title} meta={[s.year, firstGenre(s.genre)].filter(Boolean).join(" · ")} posterUrl={s.posterUrl} saved={watchlistIds.has(s.id)} onClick={() => openSeries(s)} onContextMenu={(e) => openMenu(e, seriesMenu(s))} />
          ))}
        </Shelf>
      ))}
    </div>
  );
}
