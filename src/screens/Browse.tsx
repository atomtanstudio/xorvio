import { useVirtualizer } from "@tanstack/react-virtual";
import { Bookmark, Check, ChevronsUpDown, EyeOff, Lock, LockOpen, Pin } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Art } from "../components/Artwork";
import { PosterCard } from "../components/Cards";
import { openMenu } from "../components/Menu";
import { categoryMenu, movieMenu, seriesMenu } from "../components/menus";
import { openMovie, openSeries } from "../lib/actions";
import { api } from "../lib/api";
import { lockAgain, showLocked } from "../lib/channelEdits";
import { duration, firstGenre } from "../lib/format";
import { useFilledPosters } from "../lib/posterFill";
import { useSourceName, useSources } from "../lib/sourceNames";
import { useApp } from "../lib/store";
import { usePinnedCategories } from "../lib/titleEdits";
import type { BrowseSort, Category, Movie, Series, TitleMark, WatchedFilter } from "../lib/types";
import "../styles/browse.css";

const PAGE = 120;
type Item = Movie | Series;

/** An add-on catalogue the grid can show. */
interface CatalogChoice {
  key: string;
  addonId: string;
  catalogId: string;
  name: string;
  genres: string[];
  /** Can be listed without a genre. */
  browsable: boolean;
}

const PROVIDER = "provider";

/**
 * Films or series: filters across the top, then posters or a list. When
 * add-ons lead (as in Xorvio Personal) the grid shows their catalogues first,
 * with the provider's library one choice away. The provider's films list
 * once each however many copies the sources carry, badged with how many.
 */
export function BrowseScreen({ kind }: { kind: "movie" | "series" }) {
  const addonsLead = useApp((s) => s.settings.addonsLead);
  const editsRevision = useApp((s) => s.editsRevision);
  const libraryRevision = useApp((s) => s.libraryRevision);
  const edits = useApp((s) => s.edits);
  const [showHidden, setShowHidden] = useState(false);
  const [categories, setCategories] = useState<Category[]>([]);
  const [genres, setGenres] = useState<string[]>([]);
  const [catalogs, setCatalogs] = useState<CatalogChoice[] | null>(null);
  // Opened from a library row's Show All: that source's titles, not an add-on's.
  const [start] = useState(() => {
    const asked = useApp.getState().browseStart;
    return asked?.kind === kind ? asked : null;
  });
  const [library, setLibrary] = useState<string>(start ? PROVIDER : "");
  const [categoryId, setCategoryId] = useState<string>("");
  const [genre, setGenre] = useState<string>("");
  const [sort, setSort] = useState<BrowseSort>("recentlyAdded");
  const [watched, setWatched] = useState<WatchedFilter>("all");
  const [sourceId, setSourceId] = useState<string>(start?.sourceId ?? "");
  const [layout, setLayout] = useState<"posters" | "list">("posters");
  const [items, setItems] = useState<Item[]>([]);
  const [marks, setMarks] = useState<Map<string, TitleMark>>(new Map());
  const [done, setDone] = useState(false);
  const loading = useRef(false);
  const watchlistIds = useApp((s) => s.watchlistIds);
  const allSources = useSources();
  const sources = useMemo(() => allSources.filter((s) => !s.isPaused), [allSources]);
  const sourceName = useSourceName();

  useEffect(() => {
    if (start) useApp.setState({ browseStart: null });
  }, [start]);

  useEffect(() => {
    api.categories(kind, showHidden).then(setCategories).catch(() => {});
  }, [kind, showHidden, editsRevision]);

  useEffect(() => {
    api.genres(kind).then(setGenres).catch(() => {});
    api
      .addons()
      .then((addons) =>
        setCatalogs(
          addons
            .filter((a) => a.enabled)
            .flatMap((a) =>
              a.catalogs
                .filter((c) => c.mediaType === kind && (c.browsable || c.genres.length > 0))
                .map((c) => ({ key: `${a.id}/${c.id}`, addonId: a.id, catalogId: c.id, name: c.name ?? a.name, genres: c.genres, browsable: c.browsable })),
            )
            // The popular catalogue first, as Personal opens on it.
            .sort((a, b) => Number(b.catalogId === "top") - Number(a.catalogId === "top")),
        ),
      )
      .catch(() => setCatalogs([]));
  }, [kind]);

  // Add-ons lead: open on their first catalogue; otherwise on the provider.
  useEffect(() => {
    if (catalogs === null || library) return;
    setLibrary(addonsLead && catalogs.length ? catalogs[0].key : PROVIDER);
  }, [catalogs, addonsLead, library]);

  // A category that's gone (hidden, locked again) gives way to all of them.
  useEffect(() => {
    if (categoryId && categories.length && !categories.some((c) => c.id === categoryId && !edits.get(c.id)?.hidden)) setCategoryId("");
  }, [categories, categoryId, edits]);
  // A source paused or removed, likewise.
  useEffect(() => {
    if (sourceId && sources.length && !sources.some((s) => s.id === sourceId)) setSourceId("");
  }, [sources, sourceId]);

  const catalog = catalogs?.find((c) => c.key === library) ?? null;
  // A catalogue that needs a genre starts on its first one.
  const catalogGenre = catalog ? genre || (catalog.browsable ? "" : catalog.genres[0] ?? "") : "";

  /** Versions and watched marks for these titles, merged with those known. */
  const loadMarks = useCallback(
    (ids: string[], replace = false) => {
      if (!ids.length) return;
      api
        .titleMarks(kind, ids)
        .then((list) => setMarks((known) => new Map([...(replace ? [] : known), ...list.map((m) => [m.id, m] as const)])))
        .catch(() => {});
    },
    [kind],
  );

  const query = { categoryId: categoryId || null, genre: genre || null, sort, watched, sourceId: sourceId || null, groupVersions: kind === "movie" };
  const loadPage = async (offset: number) => {
    if (loading.current || !library) return;
    loading.current = true;
    try {
      let page: Item[];
      if (catalog) {
        const row = await api.addonCatalog(catalog.addonId, kind, catalog.catalogId, catalogGenre || null, offset);
        page = kind === "movie" ? row.movies : row.series;
        setItems((prev) => {
          const seen = new Set(offset === 0 ? [] : prev.map((i) => i.id));
          const fresh = page.filter((i) => !seen.has(i.id));
          setDone(fresh.length === 0);
          return offset === 0 ? fresh : [...prev, ...fresh];
        });
      } else {
        const q = { ...query, limit: PAGE, offset };
        page = kind === "movie" ? await api.movies(q) : await api.seriesList(q);
        setItems((prev) => (offset === 0 ? page : [...prev, ...page]));
        setDone(page.length < PAGE);
        loadMarks(page.map((i) => i.id));
      }
    } catch {
      setDone(true);
    } finally {
      loading.current = false;
    }
  };

  useEffect(() => {
    setItems([]);
    setMarks(new Map());
    setDone(false);
    loading.current = false;
    loadPage(0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind, library, categoryId, genre, sort, watched, sourceId]);

  // Watching or marking something changes its marks, not the pages loaded.
  const itemsRef = useRef(items);
  itemsRef.current = items;
  useEffect(() => {
    if (libraryRevision && !catalog) loadMarks(itemsRef.current.map((i) => i.id), true);
  }, [libraryRevision, catalog, loadMarks]);

  // Titles hidden from here leave at once, without losing the place.
  const shown = useMemo(() => items.filter((i) => !edits.get(i.id)?.hidden), [items, edits]);

  const open = (item: Item) => (kind === "movie" ? openMovie(item as Movie) : openSeries(item as Series));
  const menu = (e: React.MouseEvent, item: Item) =>
    openMenu(e, kind === "movie" ? movieMenu(item as Movie, { watched: catalog ? undefined : !!marks.get(item.id)?.watched }) : seriesMenu(item as Series));
  const noun = kind === "movie" ? "movies" : "series";
  const filtered = watched !== "all" || !!sourceId || !!categoryId || !!genre;

  return (
    <div className="content browse fixed">
      <div className="filter-bar">
        {catalogs && catalogs.length > 0 && (
          <select
            className="popup-select"
            value={library}
            onChange={(e) => {
              setLibrary(e.target.value);
              setGenre("");
              setCategoryId("");
            }}
          >
            {catalogs.map((c) => (
              <option key={c.key} value={c.key}>
                {c.name}
              </option>
            ))}
            <option value={PROVIDER}>Your Provider</option>
          </select>
        )}
        {catalog ? (
          catalog.genres.length > 0 && (
            <select className="popup-select" value={catalogGenre} onChange={(e) => setGenre(e.target.value)}>
              {catalog.browsable && <option value="">All Genres</option>}
              {catalog.genres.map((g) => (
                <option key={g} value={g}>
                  {g}
                </option>
              ))}
            </select>
          )
        ) : (
          <>
            <CategoryPicker kind={kind} categories={categories} value={categoryId} onChange={setCategoryId} showHidden={showHidden} setShowHidden={setShowHidden} />
            <select className="popup-select" value={genre} onChange={(e) => setGenre(e.target.value)}>
              <option value="">All Genres</option>
              {genres.map((g) => (
                <option key={g} value={g}>
                  {g}
                </option>
              ))}
            </select>
            <select className="popup-select" value={watched} onChange={(e) => setWatched(e.target.value as WatchedFilter)} aria-label="Watched">
              <option value="all">All</option>
              <option value="unwatched">Unwatched</option>
              <option value="inProgress">In Progress</option>
            </select>
            <select className="popup-select" value={sort} onChange={(e) => setSort(e.target.value as BrowseSort)}>
              <option value="recentlyAdded">Recently Added</option>
              <option value="title">Title</option>
              <option value="year">Release Year</option>
              <option value="rating">Rating</option>
            </select>
            {sources.length > 1 && (
              <select className="popup-select" value={sourceId} onChange={(e) => setSourceId(e.target.value)} aria-label="Source">
                <option value="">All Sources</option>
                {sources.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name}
                  </option>
                ))}
              </select>
            )}
          </>
        )}
        <div style={{ flex: 1 }} />
        <span className="filter-count">
          {shown.length.toLocaleString()}
          {done ? "" : "+"} {noun}
        </span>
        <div className="segmented">
          <button className={layout === "posters" ? "selected" : ""} onClick={() => setLayout("posters")}>
            Posters
          </button>
          <button className={layout === "list" ? "selected" : ""} onClick={() => setLayout("list")}>
            List
          </button>
        </div>
      </div>
      {shown.length === 0 && done ? (
        <div className="empty-state">
          <div>
            <h3>No {noun} here</h3>
            <p>
              {watched === "unwatched"
                ? `Everything here has been watched. Choose All to see it again.`
                : watched === "inProgress"
                  ? `Nothing here is part-way through.`
                  : filtered
                    ? "Try another category, genre or source."
                    : `Add a source that carries ${noun}.`}
            </p>
          </div>
        </div>
      ) : layout === "posters" ? (
        <PosterGrid items={shown} marks={marks} watchlistIds={watchlistIds} onOpen={open} onMenu={menu} onEnd={() => !done && loadPage(items.length)} />
      ) : (
        <ListView items={shown} marks={marks} watchlistIds={watchlistIds} sourceName={sourceName} onOpen={open} onMenu={menu} onEnd={() => !done && loadPage(items.length)} kind={kind} />
      )}
    </div>
  );
}

function useWidth(ref: React.RefObject<HTMLElement | null>) {
  const [width, setWidth] = useState(1000);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setWidth(el.clientWidth));
    observer.observe(el);
    setWidth(el.clientWidth);
    return () => observer.disconnect();
  }, [ref]);
  return width;
}

interface GridProps {
  items: Item[];
  marks: Map<string, TitleMark>;
  watchlistIds: Set<string>;
  onOpen: (i: Item) => void;
  onMenu: (e: React.MouseEvent, i: Item) => void;
  onEnd: () => void;
}

function PosterGrid({ items, marks, watchlistIds, onOpen, onMenu, onEnd }: GridProps) {
  const scroller = useRef<HTMLDivElement>(null);
  const width = useWidth(scroller);
  const density = useApp((s) => s.settings.density);
  const cardWidth = density === "compact" ? 128 : 168;
  const gap = density === "compact" ? 22 : 32;
  const inner = width - 2 * 39;
  const columns = Math.max(2, Math.floor((inner + gap) / (cardWidth + gap)));
  const rowHeight = cardWidth * 1.5 + 62;
  const rows = Math.ceil(items.length / columns);
  const virtualizer = useVirtualizer({ count: rows, getScrollElement: () => scroller.current, estimateSize: () => rowHeight, overscan: 3 });
  const virtualRows = virtualizer.getVirtualItems();
  const lastRow = virtualRows[virtualRows.length - 1]?.index ?? 0;
  const firstRow = virtualRows[0]?.index ?? 0;
  const onScreen = useMemo(() => items.slice(firstRow * columns, (lastRow + 1) * columns), [items, firstRow, lastRow, columns]);
  const posters = useFilledPosters(onScreen);

  useEffect(() => {
    if (rows && lastRow >= rows - 3) onEnd();
  }, [lastRow, rows, onEnd]);

  return (
    <div className="browse-scroll" ref={scroller}>
      <div style={{ height: virtualizer.getTotalSize() + 40, position: "relative" }}>
        {virtualRows.map((row) => (
          <div key={row.key} className="poster-row" style={{ top: row.start + 18, gridTemplateColumns: `repeat(${columns}, ${cardWidth}px)`, columnGap: (inner - columns * cardWidth) / Math.max(1, columns - 1) }}>
            {items.slice(row.index * columns, row.index * columns + columns).map((item) => {
              const mark = marks.get(item.id);
              return (
                <PosterCard
                  key={item.id}
                  title={item.title}
                  meta={[item.year, firstGenre(item.genre)].filter(Boolean).join(" · ")}
                  posterUrl={item.posterUrl ?? posters.get(item.id)}
                  saved={watchlistIds.has(item.id)}
                  watched={mark?.watched}
                  progress={mark?.progress}
                  versions={mark?.versions}
                  onClick={() => onOpen(item)}
                  onContextMenu={(e) => onMenu(e, item)}
                />
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
}

function ListView({ items, marks, watchlistIds, sourceName, onOpen, onMenu, onEnd, kind }: GridProps & { kind: "movie" | "series"; sourceName: (id: string) => string }) {
  const scroller = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({ count: items.length, getScrollElement: () => scroller.current, estimateSize: () => 72, overscan: 10 });
  const virtualRows = virtualizer.getVirtualItems();
  const last = virtualRows[virtualRows.length - 1]?.index ?? 0;
  const first = virtualRows[0]?.index ?? 0;
  const onScreen = useMemo(() => items.slice(first, last + 1), [items, first, last]);
  const posters = useFilledPosters(onScreen);
  useEffect(() => {
    if (items.length && last >= items.length - 20) onEnd();
  }, [last, items.length, onEnd]);
  return (
    <div className="browse-scroll" ref={scroller}>
      <div style={{ height: virtualizer.getTotalSize() + 40, position: "relative" }}>
        {virtualRows.map((row) => {
          const item = items[row.index];
          const mark = marks.get(item.id);
          return (
            <button key={item.id} className="list-row" style={{ top: row.start + 12 }} onClick={() => onOpen(item)} onContextMenu={(e) => onMenu(e, item)}>
              <Art className="list-thumb" src={item.posterUrl ?? posters.get(item.id)} fallback="" />
              <span className="list-text">
                <span className="list-title">
                  {item.title}
                  {watchlistIds.has(item.id) && <Bookmark className="list-mark saved" size={12} fill="currentColor" aria-label="On your watchlist" />}
                  {mark?.watched && <Check className="list-mark watched" size={13} strokeWidth={3} aria-label="Watched" />}
                </span>
                <span className="list-meta">{[item.year, item.genre, kind === "movie" && (item as Movie).durationSeconds ? duration((item as Movie).durationSeconds!) : null].filter(Boolean).join(" · ")}</span>
              </span>
              {(mark?.versions ?? 0) > 1 && <span className="list-versions">{mark!.versions} versions</span>}
              {item.rating ? <span className="list-rating">★ {item.rating.toFixed(1)}</span> : null}
              <span className="list-source">{sourceName(item.sourceId)}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

/**
 * The category filter as a list of its own: lock icons on locked ones, a pin
 * on those that are rows on Home, and each category's menu (pin, rename,
 * hide, lock) on a right-click, as Live TV's categories have.
 */
function CategoryPicker({
  kind,
  categories,
  value,
  onChange,
  showHidden,
  setShowHidden,
}: {
  kind: "movie" | "series";
  categories: Category[];
  value: string;
  onChange: (id: string) => void;
  showHidden: boolean;
  setShowHidden: (show: boolean) => void;
}) {
  const edits = useApp((s) => s.edits);
  const parental = useApp((s) => s.parental);
  const pinned = usePinnedCategories();
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState("");
  const box = useRef<HTMLDivElement>(null);
  const editKind = kind === "movie" ? "movieCategory" : "seriesCategory";
  const anyHidden = useMemo(() => [...edits.values()].some((e) => e.hidden && e.kind === editKind), [edits, editKind]);
  const selected = categories.find((c) => c.id === value);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      // The category menu is outside the list; choosing from it keeps the list.
      const target = e.target as HTMLElement;
      if (!box.current?.contains(target) && !target.closest(".menu")) setOpen(false);
    };
    const escape = (e: KeyboardEvent) => e.key === "Escape" && !document.querySelector(".menu") && setOpen(false);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", escape);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", escape);
    };
  }, [open]);
  useEffect(() => {
    if (!open) setFilter("");
  }, [open]);

  const q = filter.trim().toLowerCase();
  const listed = q ? categories.filter((c) => c.name.toLowerCase().includes(q)) : categories;
  const choose = (id: string) => {
    onChange(id);
    setOpen(false);
  };
  const marks = (c: Category) => {
    const edit = edits.get(c.id);
    return (
      <span className="category-marks">
        {pinned.has(c.id) && <Pin size={11} aria-label="On Home" />}
        {edit?.hidden && <EyeOff size={11} aria-label="Hidden" />}
        {edit?.locked && <Lock size={11} aria-label="Locked" />}
      </span>
    );
  };

  return (
    <div className="category-picker" ref={box}>
      <button
        className="popup-select category-button"
        onClick={() => setOpen((o) => !o)}
        onContextMenu={(e) => selected && openMenu(e, categoryMenu(selected))}
        aria-haspopup="listbox"
        aria-expanded={open}
        title={selected ? "Right-click to pin, rename, hide or lock it" : undefined}
      >
        <span className="category-name">{selected?.name ?? "All Categories"}</span>
        {selected && marks(selected)}
        <ChevronsUpDown size={12} className="category-chevron" />
      </button>
      {open && (
        <div className="category-popover" role="listbox">
          {categories.length > 12 && <input className="text-input category-filter" placeholder="Filter categories" value={filter} onChange={(e) => setFilter(e.target.value)} autoFocus />}
          <div className="category-options">
            {!q && (
              <button className={`category-option ${value === "" ? "selected" : ""}`} onClick={() => choose("")} role="option" aria-selected={value === ""}>
                <span className="category-name">All Categories</span>
                {value === "" && <Check size={13} className="category-check" />}
              </button>
            )}
            {listed.map((c) => {
              const hidden = !!edits.get(c.id)?.hidden;
              return (
                <button
                  key={c.id}
                  className={`category-option ${c.id === value ? "selected" : ""} ${hidden ? "dimmed" : ""}`}
                  // A hidden category lists nothing; its menu shows it again.
                  onClick={(e) => (hidden ? openMenu(e, categoryMenu(c)) : choose(c.id))}
                  onContextMenu={(e) => openMenu(e, categoryMenu(c))}
                  role="option"
                  aria-selected={c.id === value}
                >
                  <span className="category-name">{c.name}</span>
                  {marks(c)}
                  {c.id === value && <Check size={13} className="category-check" />}
                </button>
              );
            })}
            {q && listed.length === 0 && <p className="muted small category-empty">No category matches “{filter.trim()}”.</p>}
          </div>
          <div className="category-footer">
            <span className="muted small">Right-click a category to pin it to Home, rename, hide or lock it.</span>
            <div className="category-footer-buttons">
              {(anyHidden || showHidden) && (
                <button className={`btn-plain ${showHidden ? "active" : ""}`} onClick={() => setShowHidden(!showHidden)}>
                  <EyeOff size={13} /> {showHidden ? "Hide Hidden" : "Show Hidden"}
                </button>
              )}
              {parental.hasPin &&
                (parental.unlocked ? (
                  <button className="btn-plain" onClick={lockAgain}>
                    <LockOpen size={13} /> Lock Again
                  </button>
                ) : (
                  parental.lockedCount > 0 && (
                    <button className="btn-plain" onClick={showLocked}>
                      <Lock size={13} /> Show Locked…
                    </button>
                  )
                ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
