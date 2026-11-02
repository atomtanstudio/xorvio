// Customize Home: choose, order and hide Home's rows for the profile in use,
// as Xorvio for Mac's HomeCustomizationSheet. Rows are dragged or moved with
// the arrows; rows added with a choice (a category, a genre, an add-on row,
// a Trakt chart or list) are removed rather than switched off.

import { ArrowDown, ArrowUp, GripVertical, Heart, Plus, Search, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api, errorText } from "../lib/api";
import {
  catalogKey,
  catalogRow,
  categoryRow,
  genreRow,
  isRemovable,
  moved,
  rowNote,
  rowTitle,
  standardLayout,
  TRAKT_CHARTS,
  traktChartRow,
  traktListRow,
  traktRecommendedRow,
  useHomeLayout,
  useHomeLayouts,
  type HomeLayout,
  type HomeMode,
  type HomeRow,
} from "../lib/homeLayout";
import { GENRE_NAMES, GENRES, KIND_NAMES, useActiveProfile, useProfiles } from "../lib/profiles";
import { useApp } from "../lib/store";
import type { AddonCatalogRow, Category, CategoryKind, ContentGenre, TraktChart, TraktListInfo, TraktMedia, TraktStatus } from "../lib/types";
import { Sheet } from "./Sheet";
import "../styles/profiles.css";

export function HomeLayoutSheetHost() {
  const open = useProfiles((s) => s.customizingHome);
  return open ? <Opened /> : null;
}

/** Works out which of Home's layouts is in use as the sheet opens, as Home does. */
function Opened() {
  const mode = useHomeMode();
  return mode ? <Loaded mode={mode} /> : null;
}

/** Whether add-ons lead Home: the setting is on and an add-on has catalogues. */
function useHomeMode(): HomeMode | null {
  const addonsLead = useApp((s) => s.settings.addonsLead);
  const [hasCatalogs, setHasCatalogs] = useState<boolean | null>(null);
  useEffect(() => {
    api
      .addons()
      .then((addons) => setHasCatalogs(addons.some((a) => a.enabled && a.catalogs.length > 0)))
      .catch(() => setHasCatalogs(false));
  }, [addonsLead]);
  if (hasCatalogs === null) return null;
  return addonsLead && hasCatalogs ? "addons" : "sources";
}

function Loaded({ mode }: { mode: HomeMode }) {
  const layout = useHomeLayout(mode);
  if (!layout) return null;
  return <HomeLayoutSheet mode={mode} saved={layout} />;
}

function HomeLayoutSheet({ mode, saved }: { mode: HomeMode; saved: HomeLayout }) {
  const profile = useActiveProfile();
  const [layout, setLayout] = useState(saved);
  const [dragging, setDragging] = useState<string | null>(null);
  const [drop, setDrop] = useState<{ id: string; after: boolean } | null>(null);
  const close = () => useProfiles.getState().customizeHome(false);
  const done = async () => {
    await useHomeLayouts
      .getState()
      .save(mode, layout)
      .catch((e) => useApp.getState().showToast(`Home's layout couldn't be saved. ${errorText(e)}`, "error"));
    close();
  };

  const update = (id: string, patch: Partial<HomeRow>) => setLayout((l) => ({ ...l, rows: l.rows.map((row) => (row.id === id ? { ...row, ...patch } : row)) }));
  const remove = (id: string) => setLayout((l) => ({ ...l, rows: l.rows.filter((row) => row.id !== id) }));
  // New rows go near the top, under Continue Watching, so they're easy to find.
  const add = (row: HomeRow) =>
    setLayout((l) => {
      if (l.rows.some((r) => r.id === row.id)) return { ...l, rows: l.rows.map((r) => (r.id === row.id ? { ...r, visible: true } : r)) };
      const at = l.rows[0]?.kind === "continueWatching" ? 1 : 0;
      return { ...l, rows: [...l.rows.slice(0, at), row, ...l.rows.slice(at)] };
    });
  const dropOn = (target: string, after: boolean) => {
    if (!dragging || dragging === target) return;
    setLayout((l) => {
      const rows = l.rows.filter((row) => row.id !== dragging);
      const row = l.rows.find((r) => r.id === dragging)!;
      const at = rows.findIndex((r) => r.id === target) + (after ? 1 : 0);
      rows.splice(at, 0, row);
      return { ...l, rows };
    });
  };

  return (
    <Sheet
      title="Customize Home"
      subtitle={`For the “${profile?.name ?? "Everything"}” profile${mode === "addons" ? ", while add-ons lead" : ""}. Drag rows to reorder them, and switch a row off to hide it.`}
      onClose={close}
      width={640}
      footer={
        <>
          <button className="btn-plain home-layout-reset" onClick={() => setLayout(standardLayout(mode, profile, useHomeLayouts.getState().liveOnly))}>
            Reset to default
          </button>
          <button className="btn-plain" onClick={close}>
            Cancel
          </button>
          <button className="btn-accent" onClick={done}>
            Done
          </button>
        </>
      }
    >
      <section className="settings-group profile-group">
        <div className="settings-card">
          <div className="settings-row">
            <span className="settings-label">Show featured titles at the top</span>
            <button
              role="switch"
              aria-checked={layout.showHero}
              aria-label="Show featured titles at the top"
              className={`toggle ${layout.showHero ? "on" : ""}`}
              onClick={() => setLayout((l) => ({ ...l, showHero: !l.showHero }))}
            />
          </div>
        </div>
      </section>

      <section className="settings-group profile-group">
        <h2 className="section-title">Rows</h2>
        <div className="settings-card home-layout-rows" onDragEnd={() => (setDragging(null), setDrop(null))}>
          {layout.rows.map((row, i) => (
            <div
              key={row.id}
              className={`home-layout-row ${row.visible ? "" : "off"} ${dragging === row.id ? "dragging" : ""} ${drop?.id === row.id ? (drop.after ? "drop-after" : "drop-before") : ""}`}
              draggable
              onDragStart={(e) => {
                e.dataTransfer.effectAllowed = "move";
                e.dataTransfer.setData("text/plain", row.id);
                setDragging(row.id);
              }}
              onDragOver={(e) => {
                if (!dragging) return;
                e.preventDefault();
                const rect = e.currentTarget.getBoundingClientRect();
                setDrop({ id: row.id, after: e.clientY > rect.top + rect.height / 2 });
              }}
              onDrop={(e) => {
                e.preventDefault();
                if (drop) dropOn(drop.id, drop.after);
                setDragging(null);
                setDrop(null);
              }}
            >
              <span className="home-layout-grip" aria-hidden="true">
                <GripVertical size={15} />
              </span>
              <div className="home-layout-text">
                <div className="home-layout-title">{rowTitle(row)}</div>
                {rowNote(row) && <div className="home-layout-note">{rowNote(row)}</div>}
              </div>
              <div className="home-layout-buttons">
                <button className="icon-button" aria-label={`Move ${rowTitle(row)} up`} disabled={i === 0} onClick={() => setLayout((l) => moved(l, row.id, -1, false))}>
                  <ArrowUp size={14} />
                </button>
                <button
                  className="icon-button"
                  aria-label={`Move ${rowTitle(row)} down`}
                  disabled={i === layout.rows.length - 1}
                  onClick={() => setLayout((l) => moved(l, row.id, 1, false))}
                >
                  <ArrowDown size={14} />
                </button>
                {isRemovable(row) && (
                  <button className="icon-button" aria-label={`Remove ${rowTitle(row)}`} title="Remove this row" onClick={() => remove(row.id)}>
                    <X size={14} />
                  </button>
                )}
              </div>
              <button role="switch" aria-checked={row.visible} aria-label={`Show ${rowTitle(row)}`} className={`toggle ${row.visible ? "on" : ""}`} onClick={() => update(row.id, { visible: !row.visible })} />
            </div>
          ))}
        </div>
      </section>

      <AddRow layout={layout} add={add} />
      <AddTraktRow layout={layout} add={add} />
    </Sheet>
  );
}

/** Rows that need a choice: a genre, a category, or one add-on row on its own. */
function AddRow({ layout, add }: { layout: HomeLayout; add: (row: HomeRow) => void }) {
  const profile = useActiveProfile();
  const [genre, setGenre] = useState<ContentGenre>(profile?.genres[0] ?? "action");
  const [genreKind, setGenreKind] = useState<"movie" | "series">("movie");
  const [kind, setKind] = useState<CategoryKind>("movie");
  const [categories, setCategories] = useState<Category[]>([]);
  const [categoryId, setCategoryId] = useState("");
  const [catalogs, setCatalogs] = useState<AddonCatalogRow[]>([]);
  const [catalog, setCatalog] = useState("");
  const present = new Set(layout.rows.map((row) => row.id));

  useEffect(() => {
    let cancelled = false;
    api
      .categories(kind)
      .then((list) => {
        if (cancelled) return;
        setCategories(list);
        setCategoryId(list[0]?.id ?? "");
      })
      .catch(() => setCategories([]));
    return () => {
      cancelled = true;
    };
  }, [kind]);
  useEffect(() => {
    api
      .discoverRows()
      .then((rows) => {
        setCatalogs(rows);
        setCatalog(rows[0] ? catalogKey(rows[0]) : "");
      })
      .catch(() => setCatalogs([]));
  }, []);

  const genreChoice = genreRow(genre, genreKind === "series");
  const category = categories.find((c) => c.id === categoryId);
  const addonRow = catalogs.find((row) => catalogKey(row) === catalog);
  return (
    <section className="settings-group profile-group">
      <h2 className="section-title">Add a row</h2>
      <div className="settings-card">
        <div className="home-layout-add">
          <span className="home-layout-add-label">A genre, from category names and the titles' own genres</span>
          <div style={{ display: "flex", gap: 8 }}>
            <select className="select-input" value={genre} onChange={(e) => setGenre(e.target.value as ContentGenre)} aria-label="Genre">
              {GENRES.map((g) => (
                <option key={g} value={g}>
                  {GENRE_NAMES[g]}
                </option>
              ))}
            </select>
            <select className="select-input" value={genreKind} onChange={(e) => setGenreKind(e.target.value as "movie" | "series")} aria-label="Films or series">
              <option value="movie">Films</option>
              <option value="series">Series</option>
            </select>
          </div>
          <AddButton disabled={present.has(genreChoice.id)} onClick={() => add(genreChoice)} />
        </div>
        <div className="home-layout-add">
          <span className="home-layout-add-label">A category from your sources</span>
          <div style={{ display: "flex", gap: 8, minWidth: 0 }}>
            <select className="select-input" style={{ width: 120, flex: "none" }} value={kind} onChange={(e) => setKind(e.target.value as CategoryKind)} aria-label="Kind">
              {(["live", "movie", "series"] as CategoryKind[]).map((k) => (
                <option key={k} value={k}>
                  {KIND_NAMES[k]}
                </option>
              ))}
            </select>
            <select className="select-input" value={categoryId} onChange={(e) => setCategoryId(e.target.value)} aria-label="Category" disabled={!categories.length}>
              {!categories.length && <option value="">None in this profile</option>}
              {categories.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
          </div>
          <AddButton disabled={!category || present.has(categoryRow(category).id)} onClick={() => category && add(categoryRow(category))} />
        </div>
        {catalogs.length > 0 && (
          <div className="home-layout-add">
            <span className="home-layout-add-label">One add-on row on its own (it leaves the add-on rows)</span>
            <select className="select-input" value={catalog} onChange={(e) => setCatalog(e.target.value)} aria-label="Add-on row">
              {catalogs.map((row) => (
                <option key={catalogKey(row)} value={catalogKey(row)}>
                  {row.title}
                </option>
              ))}
            </select>
            <AddButton disabled={!addonRow || present.has(catalogRow(addonRow).id)} onClick={() => addonRow && add(catalogRow(addonRow))} />
          </div>
        )}
      </div>
    </section>
  );
}

/** Rows from Trakt: a chart, recommendations, the account's own and liked lists, or any public list found by name or its trakt.tv address. */
function AddTraktRow({ layout, add }: { layout: HomeLayout; add: (row: HomeRow) => void }) {
  const [status, setStatus] = useState<TraktStatus | null>(null);
  const [chart, setChart] = useState<TraktChart | "recommended">("trending");
  const [media, setMedia] = useState<TraktMedia>("movies");
  const [own, setOwn] = useState<TraktListInfo[]>([]);
  const [ownKey, setOwnKey] = useState("");
  const present = new Set(layout.rows.map((row) => row.id));

  useEffect(() => {
    api.traktStatus().then(setStatus).catch(() => setStatus(null));
  }, []);
  // The account's own lists, then those it liked.
  useEffect(() => {
    if (!status?.connected) return setOwn([]);
    Promise.all([api.traktLists({ type: "mine" }).catch(() => []), api.traktLists({ type: "liked" }).catch(() => [])]).then(([mine, liked]) => {
      const lists = [...mine, ...liked.filter((l) => !mine.some((m) => m.user === l.user && m.list === l.list))];
      setOwn(lists);
      setOwnKey(lists[0] ? listKey(lists[0]) : "");
    });
  }, [status?.connected]);

  if (!status) return null;
  const chartChoice = chart === "recommended" ? traktRecommendedRow(media) : traktChartRow(chart, chart === "boxOffice" ? "movies" : media);
  const ownList = own.find((l) => listKey(l) === ownKey);
  return (
    <section className="settings-group profile-group">
      <h2 className="section-title">From Trakt</h2>
      <div className="settings-card">
        {!status.configured ? (
          <div className="home-layout-add">
            <span className="home-layout-add-label">Trakt's charts and lists need a Trakt app. Add its client ID in Settings › Trakt.</span>
          </div>
        ) : (
          <>
            <div className="home-layout-add">
              <span className="home-layout-add-label">A chart, of what your sources and add-ons can play</span>
              <div style={{ display: "flex", gap: 8 }}>
                <select className="select-input" value={chart} onChange={(e) => setChart(e.target.value as TraktChart | "recommended")} aria-label="Trakt chart">
                  {TRAKT_CHARTS.map((c) => (
                    <option key={c.chart} value={c.chart}>
                      {c.label}
                    </option>
                  ))}
                  {status.connected && <option value="recommended">Recommended for You</option>}
                </select>
                <select className="select-input" value={chart === "boxOffice" ? "movies" : media} onChange={(e) => setMedia(e.target.value as TraktMedia)} aria-label="Films or series" disabled={chart === "boxOffice"}>
                  <option value="movies">Films</option>
                  <option value="shows">Series</option>
                </select>
              </div>
              <AddButton disabled={present.has(chartChoice.id)} onClick={() => add(chartChoice)} />
            </div>
            {own.length > 0 && (
              <div className="home-layout-add">
                <span className="home-layout-add-label">Your lists, and lists you've liked on Trakt</span>
                <select className="select-input" value={ownKey} onChange={(e) => setOwnKey(e.target.value)} aria-label="Your Trakt list">
                  {own.map((l) => (
                    <option key={listKey(l)} value={listKey(l)}>
                      {l.name}
                      {l.owner !== status.username ? ` (${l.owner})` : ""}
                    </option>
                  ))}
                </select>
                <AddButton disabled={!ownList || present.has(traktListRow(ownList).id)} onClick={() => ownList && add(traktListRow(ownList))} />
              </div>
            )}
            <FindTraktList present={present} add={add} />
          </>
        )}
      </div>
    </section>
  );
}

const listKey = (list: TraktListInfo) => `${list.user}/${list.list}`;

/** Lists found by words or a trakt.tv address; Trakt's most liked until something's typed. */
function FindTraktList({ present, add }: { present: Set<string>; add: (row: HomeRow) => void }) {
  const [text, setText] = useState("");
  const [lists, setLists] = useState<TraktListInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const asked = useRef(0);
  useEffect(() => {
    const at = ++asked.current;
    const query = text.trim();
    const timer = setTimeout(
      () => {
        setError(null);
        api
          .traktLists(query ? { type: "search", text: query } : { type: "popular" })
          .then((found) => at === asked.current && setLists(found))
          .catch((e) => {
            if (at !== asked.current) return;
            setLists([]);
            setError(errorText(e));
          });
      },
      query ? 400 : 0,
    );
    return () => clearTimeout(timer);
  }, [text]);
  return (
    <div className="home-layout-add">
      <span className="home-layout-add-label">Any public list, such as the IMDb Top 250</span>
      <label className="trakt-list-search" style={{ gridColumn: "1 / -1" }}>
        <Search size={14} aria-hidden="true" />
        <input type="search" value={text} onChange={(e) => setText(e.target.value)} placeholder="Search Trakt's lists, or paste a list's link" aria-label="Find a Trakt list" spellCheck={false} />
      </label>
      <div className="trakt-list-results" style={{ gridColumn: "1 / -1" }}>
        {!text.trim() && !!lists?.length && <div className="trakt-list-heading">Most liked on Trakt</div>}
        {lists?.map((list) => {
          const row = traktListRow(list);
          return (
            <div key={row.id} className="trakt-list-result">
              <div className="home-layout-text">
                <div className="home-layout-title">{list.name}</div>
                <div className="home-layout-note">
                  {[`By ${list.owner}`, list.itemCount ? `${list.itemCount.toLocaleString()} titles` : null].filter(Boolean).join(" · ")}
                  {list.likes > 0 && (
                    <span className="trakt-list-likes">
                      <Heart size={10} fill="currentColor" strokeWidth={0} /> {list.likes.toLocaleString()}
                    </span>
                  )}
                </div>
              </div>
              <AddButton disabled={present.has(row.id)} onClick={() => add(row)} />
            </div>
          );
        })}
        {lists?.length === 0 && !!text.trim() && <div className="home-layout-note">{error ?? "No lists found. Try other words, or paste the list's address from trakt.tv."}</div>}
      </div>
    </div>
  );
}

function AddButton({ disabled, onClick }: { disabled: boolean; onClick: () => void }) {
  return (
    <button className="btn-plain" disabled={disabled} onClick={onClick} title={disabled ? "Already on Home" : undefined}>
      <Plus size={14} /> Add
    </button>
  );
}
