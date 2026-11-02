import { Bell, BellOff, Bookmark, LayoutGrid, List, Play, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Art, ChannelLogo } from "../components/Artwork";
import { LandscapeCard, NewEpisodeChip, PosterCard } from "../components/Cards";
import { playChannel, playMovie, setReminder, showEntryInGuide } from "../lib/actions";
import { api } from "../lib/api";
import { clockTime, dayName, nowSeconds, timeRange, whenLabel } from "../lib/format";
import { newEpisodeLabel, useNewEpisodeCounts } from "../lib/newEpisodes";
import { trayName } from "../lib/platform";
import { recordingFor, useApp } from "../lib/store";
import type { Channel, Movie, Series, WatchlistEntry } from "../lib/types";
import { openWatchlistEntry, watchlistEntryMenu } from "./Home";
import "../styles/watchlist.css";

type Scope = "all" | "movies" | "series" | "live";
type Sort = "recentlyAdded" | "title" | "year";
type Layout = "posters" | "list";

const SCOPES: [Scope, string][] = [
  ["all", "All"],
  ["movies", "Movies"],
  ["series", "Series"],
  ["live", "Live TV"],
];

interface View {
  scope: Scope;
  sort: Sort;
  layout: Layout;
}

/** The scope, order and layout chosen last, kept in this window's storage. */
const VIEW_KEY = "watchlist.view";

function savedView(): View {
  const view: View = { scope: "all", sort: "recentlyAdded", layout: "posters" };
  try {
    const saved = JSON.parse(localStorage.getItem(VIEW_KEY) ?? "{}") as Partial<View>;
    if (SCOPES.some(([scope]) => scope === saved.scope)) view.scope = saved.scope!;
    if (saved.sort === "title" || saved.sort === "year" || saved.sort === "recentlyAdded") view.sort = saved.sort;
    if (saved.layout === "list" || saved.layout === "posters") view.layout = saved.layout;
  } catch {
    // Unreadable: the defaults.
  }
  return view;
}

/** A film or series as the catalog (or its add-on) has it, for its year and synopsis. */
type Title = Movie | Series;

/**
 * Everything saved to watch, as Xorvio for Mac lays it out: programmes from
 * the guide by day (on now first), then films and series, as posters or a
 * list, narrowed to one kind and ordered as chosen.
 */
export function WatchlistScreen() {
  const libraryRevision = useApp((s) => s.libraryRevision);
  const reminderMinutes = useApp((s) => s.settings.reminderMinutes);
  const go = useApp((s) => s.go);
  const [entries, setEntries] = useState<WatchlistEntry[] | null>(null);
  const [titles, setTitles] = useState<Map<string, Title>>(new Map());
  const [view, setView] = useState<View>(savedView);
  const choose = (change: Partial<View>) =>
    setView((v) => {
      const next = { ...v, ...change };
      try {
        localStorage.setItem(VIEW_KEY, JSON.stringify(next));
      } catch {
        // Not kept: it still applies now.
      }
      return next;
    });

  useEffect(() => {
    api.watchlist().then(setEntries).catch(() => setEntries([]));
  }, [libraryRevision]);

  // Each film's and series' year and synopsis, which the entries don't keep.
  useEffect(() => {
    if (!entries) return;
    let cancelled = false;
    const media = entries.filter((e) => e.kind !== "program");
    Promise.all(media.map((e) => (e.kind === "movie" ? api.movie(e.id) : api.series(e.id)).catch(() => null))).then((found) => {
      if (cancelled) return;
      setTitles(new Map(found.flatMap((title): [string, Title][] => (title ? [[title.id, title]] : []))));
    });
    return () => {
      cancelled = true;
    };
  }, [entries]);

  const programs = useMemo(() => (view.scope === "all" || view.scope === "live" ? (entries ?? []).filter((e) => e.kind === "program") : []), [entries, view.scope]);
  const media = useMemo(() => {
    const kinds = view.scope === "all" ? ["movie", "series"] : view.scope === "movies" ? ["movie"] : view.scope === "series" ? ["series"] : [];
    const chosen = (entries ?? []).filter((e) => kinds.includes(e.kind));
    const year = (e: WatchlistEntry) => titles.get(e.id)?.year ?? (Number(e.subtitle?.match(/^\d{4}$/)?.[0]) || 0);
    if (view.sort === "title") return [...chosen].sort((a, b) => a.title.localeCompare(b.title, undefined, { numeric: true, sensitivity: "base" }));
    if (view.sort === "year") return [...chosen].sort((a, b) => year(b) - year(a) || b.addedAt - a.addedAt);
    return [...chosen].sort((a, b) => b.addedAt - a.addedAt);
  }, [entries, titles, view.scope, view.sort]);

  if (!entries) return <div className="content" />;
  const all = entries;
  return (
    <div className="content">
      <div className="page">
        <div className="page-header">
          <div>
            <h1 className="page-title">Watchlist</h1>
            <p className="page-subtitle">{all.length ? summary(all, reminderMinutes) : "Films, series and programmes from the guide. Programmes leave once they've aired; record them to keep them."}</p>
          </div>
        </div>
        {all.length > 0 && (
          <div className="watchlist-controls">
            <div className="segmented" role="tablist" aria-label="Show">
              {SCOPES.map(([scope, label]) => (
                <button key={scope} role="tab" aria-selected={view.scope === scope} className={view.scope === scope ? "selected" : ""} onClick={() => choose({ scope })}>
                  {label}
                </button>
              ))}
            </div>
            <div style={{ flex: 1 }} />
            {view.scope !== "live" && (
              <select className="popup-select" value={view.sort} onChange={(e) => choose({ sort: e.target.value as Sort })} aria-label="Sort films and series" title="How films and series are ordered">
                <option value="recentlyAdded">Recently Added</option>
                <option value="title">Title</option>
                <option value="year">Release Year</option>
              </select>
            )}
            <div className="segmented icons" aria-label="Layout">
              <button className={view.layout === "posters" ? "selected" : ""} onClick={() => choose({ layout: "posters" })} aria-label="Posters" title="Posters">
                <LayoutGrid size={15} strokeWidth={1.8} />
              </button>
              <button className={view.layout === "list" ? "selected" : ""} onClick={() => choose({ layout: "list" })} aria-label="List" title="List">
                <List size={15} strokeWidth={1.8} />
              </button>
            </div>
          </div>
        )}
        {!all.length ? (
          <div className="empty-state">
            <div>
              <Bookmark size={40} strokeWidth={1.4} />
              <h3>Your watchlist is empty</h3>
              <p>Add films and series with the + button, or a game or show from the guide. Programmes you add remind you before they start, while Xorvio is open or in {trayName}.</p>
              <div style={{ display: "flex", gap: 10, justifyContent: "center" }}>
                <button className="btn btn-glass" onClick={() => go("movies")}>
                  Browse Movies
                </button>
                <button className="btn btn-glass" onClick={() => go("live")}>
                  Open the Guide
                </button>
              </div>
            </div>
          </div>
        ) : !programs.length && !media.length ? (
          <div className="empty-state">
            <div>
              <h3>Nothing here</h3>
              <p>Your watchlist has nothing in {SCOPES.find(([scope]) => scope === view.scope)?.[1]}.</p>
              <button className="btn btn-glass" onClick={() => choose({ scope: "all" })}>
                Show All
              </button>
            </div>
          </div>
        ) : (
          <>
            {programs.length > 0 && <ProgramSection programs={programs} layout={view.layout} />}
            {media.length > 0 && <MediaSection entries={media} titles={titles} layout={view.layout} scope={view.scope} />}
          </>
        )}
      </div>
    </div>
  );
}

/** "2 films · 1 series · 3 live programmes. Xorvio reminds you 5 minutes before each one starts." */
function summary(entries: WatchlistEntry[], reminderMinutes: number) {
  const count = (kind: WatchlistEntry["kind"]) => entries.filter((e) => e.kind === kind).length;
  const [films, series, programmes] = [count("movie"), count("series"), count("program")];
  const parts = [
    films ? `${films} film${films === 1 ? "" : "s"}` : null,
    series ? `${series} series` : null,
    programmes ? `${programmes} live programme${programmes === 1 ? "" : "s"}` : null,
  ].filter(Boolean);
  let text = parts.join(" · ");
  if (programmes) text += reminderMinutes === 0 ? ". Xorvio reminds you as each programme starts." : `. Xorvio reminds you ${reminderMinutes} minute${reminderMinutes === 1 ? "" : "s"} before each one starts.`;
  return text;
}

/** Programmes grouped by day, "On Now" first, as wide cards (their artwork is landscape) or a list. */
function ProgramSection({ programs, layout }: { programs: WatchlistEntry[]; layout: Layout }) {
  const recordings = useApp((s) => s.recordings);
  const t = nowSeconds();
  const groups: { title: string; live: boolean; items: WatchlistEntry[] }[] = [];
  const live = programs.filter((p) => (p.start ?? 0) <= t);
  if (live.length) groups.push({ title: "On Now", live: true, items: live });
  for (const p of programs.filter((p) => (p.start ?? 0) > t).sort((a, b) => (a.start ?? 0) - (b.start ?? 0))) {
    const title = dayName(p.start ?? t);
    const group = groups.find((g) => g.title === title && !g.live);
    if (group) group.items.push(p);
    else groups.push({ title, live: false, items: [p] });
  }
  return (
    <section className="grid-section">
      <h2 className="section-title">Live &amp; Upcoming</h2>
      {groups.map((group) => (
        <div key={group.title} className="day-group">
          <div className={`day-title ${group.live ? "live" : ""}`}>{group.title}</div>
          {layout === "list" ? (
            <div className="watchlist-list">
              {group.items.map((entry) => (
                <ProgramRow key={entry.id} entry={entry} />
              ))}
            </div>
          ) : (
            <div className="landscape-grid">
              {group.items.map((entry) => {
                const recording = entry.channelId && entry.start ? recordingFor(recordings, entry.channelId, entry.start, entry.id) : undefined;
                return (
                  <LandscapeCard
                    key={entry.id}
                    title={entry.title}
                    subtitle={entry.subtitle}
                    imageUrl={entry.artworkUrl}
                    chip={
                      entry.start && entry.start > t ? (
                        <span className="chip" title={entry.remind ? "Reminder on" : "Reminder off"}>
                          {entry.remind ? <Bell size={10} strokeWidth={2.4} /> : <BellOff size={10} strokeWidth={2.4} />} {whenLabel(entry.start)}
                        </span>
                      ) : (
                        <span className="chip" style={{ color: "#ff8a8d" }}>
                          ● Live now
                        </span>
                      )
                    }
                    corner={
                      recording ? (
                        <span className={`rec-badge ${recording.status === "recording" ? "active" : ""}`} style={{ background: "rgba(10,12,16,0.62)" }}>
                          <span className="dot" />
                          REC
                        </span>
                      ) : null
                    }
                    onClick={() => openWatchlistEntry(entry)}
                    onContextMenu={(e) => watchlistEntryMenu(e, entry)}
                  />
                );
              })}
            </div>
          )}
        </div>
      ))}
    </section>
  );
}

/** A programme in the list layout: when, where, and its reminder. */
function ProgramRow({ entry }: { entry: WatchlistEntry }) {
  const [channel, setChannel] = useState<Channel | null>(null);
  useEffect(() => {
    if (entry.channelId) api.channel(entry.channelId).then(setChannel).catch(() => {});
  }, [entry.channelId]);
  const t = nowSeconds();
  const airing = !!entry.start && entry.start <= t;
  const details = [channel?.name, entry.subtitle !== channel?.name ? entry.subtitle : null, entry.start && entry.end ? timeRange(entry.start, entry.end) : null].filter(Boolean).join(" · ");
  return (
    <div className="watchlist-row" onContextMenu={(e) => watchlistEntryMenu(e, entry)}>
      <span className="watchlist-row-time">{airing ? <span className="live-pill">LIVE</span> : entry.start ? clockTime(entry.start) : ""}</span>
      <ChannelLogo src={channel?.logoUrl} name={channel?.name ?? entry.title} size={34} />
      <button className="watchlist-row-text" onClick={() => openWatchlistEntry(entry)}>
        <span className="list-title ellipsis">{entry.title}</span>
        <span className="list-meta">{details}</span>
      </button>
      {!airing && (
        <button
          className={`icon-button watchlist-bell ${entry.remind ? "on" : ""}`}
          onClick={() => setReminder(entry, !entry.remind)}
          aria-label={entry.remind ? "Turn Off Reminder" : "Remind Me"}
          title={entry.remind ? "Reminder on. Click to turn it off." : "Reminder off. Click to be reminded before it starts."}
        >
          {entry.remind ? <Bell size={16} strokeWidth={2} /> : <BellOff size={16} strokeWidth={2} />}
        </button>
      )}
      <button className="btn-plain" onClick={() => (airing && channel ? playChannel(channel) : showEntryInGuide(entry))}>
        {airing ? "Watch" : "Show in Guide"}
      </button>
      <RemoveButton entry={entry} />
    </div>
  );
}

function MediaSection({ entries, titles, layout, scope }: { entries: WatchlistEntry[]; titles: Map<string, Title>; layout: Layout; scope: Scope }) {
  const newEpisodes = useNewEpisodeCounts();
  const badge = (entry: WatchlistEntry) => (entry.kind === "series" && newEpisodes[entry.id] ? <NewEpisodeChip label={newEpisodeLabel(newEpisodes[entry.id])} /> : null);
  return (
    <section className="grid-section">
      <h2 className="section-title">{scope === "movies" ? "Movies" : scope === "series" ? "Series" : "Movies & Series"}</h2>
      {layout === "list" ? (
        <div className="watchlist-list">
          {entries.map((entry) => (
            <MediaRow key={entry.id} entry={entry} title={titles.get(entry.id)} badge={badge(entry)} />
          ))}
        </div>
      ) : (
        <div className="poster-grid">
          {entries.map((entry) => (
            <PosterCard
              key={entry.id}
              title={entry.title}
              meta={meta(entry, titles.get(entry.id))}
              posterUrl={titles.get(entry.id)?.posterUrl ?? entry.artworkUrl}
              eager
              badge={badge(entry)}
              onClick={() => openWatchlistEntry(entry)}
              onContextMenu={(e) => watchlistEntryMenu(e, entry)}
            />
          ))}
        </div>
      )}
    </section>
  );
}

/** "2024 · ★ 7.8", or "Series · 2023". */
function meta(entry: WatchlistEntry, title: Title | undefined) {
  if (!title) return entry.subtitle ?? (entry.kind === "series" ? "Series" : "Film");
  if (entry.kind === "series") return ["Series", title.year].filter(Boolean).join(" · ");
  return [title.year, title.rating ? `★ ${title.rating.toFixed(1)}` : null].filter(Boolean).join(" · ");
}

/** A film or series in the list layout. */
function MediaRow({ entry, title, badge }: { entry: WatchlistEntry; title: Title | undefined; badge: React.ReactNode }) {
  return (
    <div className="watchlist-row" onContextMenu={(e) => watchlistEntryMenu(e, entry)}>
      <button className="watchlist-row-text with-poster" onClick={() => openWatchlistEntry(entry)}>
        <Art className="list-thumb" src={title?.posterUrl ?? entry.artworkUrl} fallback="" />
        <span className="list-text">
          <span className="list-title ellipsis">{entry.title}</span>
          <span className="list-meta">{meta(entry, title)}</span>
          {title?.plot && <span className="list-meta watchlist-plot">{title.plot}</span>}
        </span>
      </button>
      {badge}
      {entry.kind === "movie" && title && "durationSeconds" in title && (
        <button className="btn-plain" onClick={() => playMovie(title)}>
          <Play size={12} fill="currentColor" strokeWidth={0} /> Play
        </button>
      )}
      <RemoveButton entry={entry} />
    </div>
  );
}

function RemoveButton({ entry }: { entry: WatchlistEntry }) {
  return (
    <button className="icon-button watchlist-remove" onClick={() => useApp.getState().setWatchlisted(entry, false)} aria-label="Remove from Watchlist" title="Remove from Watchlist">
      <X size={15} strokeWidth={2} />
    </button>
  );
}
