import { Bell, CalendarSearch, CircleDot, CircleStop, Play, Search as SearchIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { Art, ChannelLogo } from "../components/Artwork";
import { PosterCard } from "../components/Cards";
import { openMenu } from "../components/Menu";
import { channelMenu, movieMenu, programMenu, seriesMenu } from "../components/menus";
import { Shelf } from "../components/Shelf";
import { cancelOrStopRecording, openMovie, openSeries, playChannel, programEntry, recordProgram, toggleWatchlist } from "../lib/actions";
import { api } from "../lib/api";
import { clockTime, dayName, duration, firstGenre, nowSeconds } from "../lib/format";
import { showInGuide } from "../lib/guideJump";
import { imageSrc } from "../lib/images";
import { useFilledPosters } from "../lib/posterFill";
import { useSourceName } from "../lib/sourceNames";
import { recordingFor, useApp } from "../lib/store";
import type { Movie, ProgramHit, SearchResults, Series } from "../lib/types";
import "../styles/browse.css";

/** Listings shown before "Show All". */
const LISTINGS = 30;

/** Everything that matches, each with its source, and guide programmes ready to watch, record or be reminded of. */
export function SearchScreen({ query }: { query: string }) {
  const [results, setResults] = useState<SearchResults | null>(null);
  const [addonResults, setAddonResults] = useState<{ movies: Movie[]; series: Series[] }>({ movies: [], series: [] });
  const [allListings, setAllListings] = useState(false);
  const addonsLead = useApp((s) => s.settings.addonsLead);
  const sourceName = useSourceName();

  useEffect(() => {
    let cancelled = false;
    setAllListings(false);
    const timer = setTimeout(() => {
      api.search(query).then((r) => !cancelled && setResults(r)).catch(() => {});
      api.addonSearch(query).then((r) => !cancelled && setAddonResults(r)).catch(() => {});
    }, 180);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [query]);

  if (!results) return <div className="content" />;
  const nothing = !results.channels.length && !results.movies.length && !results.series.length && !results.programs.length && !addonResults.movies.length && !addonResults.series.length;
  const upcoming = results.programs.filter((p) => p.program.end > nowSeconds()).sort((a, b) => a.program.start - b.program.start);
  const total = results.channels.length + results.movies.length + results.series.length + upcoming.length + addonResults.movies.length + addonResults.series.length;

  const addonShelves = (
    <>
      {addonResults.movies.length > 0 && <TitleShelf title="Films from Add-ons" items={addonResults.movies} kind="movie" />}
      {addonResults.series.length > 0 && <TitleShelf title="Series from Add-ons" items={addonResults.series} kind="series" />}
    </>
  );

  return (
    <div className="content search-page">
      <div className="page-header page search-header" style={{ paddingBottom: 0 }}>
        <h1 className="page-title">Results for “{query}”</h1>
        {!nothing && (
          <span className="result-count">
            {total.toLocaleString()} result{total === 1 ? "" : "s"}
          </span>
        )}
      </div>
      {nothing ? (
        <div className="empty-state">
          <div>
            <SearchIcon size={40} strokeWidth={1.4} />
            <h3>No matches</h3>
            <p>Check the spelling, or try a channel number, an actor or part of a title. Hidden and locked categories aren't searched.</p>
          </div>
        </div>
      ) : (
        <>
          {upcoming.length > 0 && (
            <section className="shelf">
              <div className="shelf-header">
                <h2 className="shelf-title">On TV</h2>
              </div>
              <div className="program-results">
                {(allListings ? upcoming : upcoming.slice(0, LISTINGS)).map((hit) => (
                  <ProgramResult key={hit.program.id} hit={hit} source={sourceName(hit.channel.sourceId)} />
                ))}
                {upcoming.length > LISTINGS && (
                  <button className="link-button program-more" onClick={() => setAllListings(!allListings)}>
                    {allListings ? "Show Fewer" : `Show All ${upcoming.length.toLocaleString()} Listings`}
                  </button>
                )}
              </div>
            </section>
          )}
          {addonsLead && addonShelves}
          {results.movies.length > 0 && <TitleShelf title="Films" items={results.movies} kind="movie" withSource />}
          {results.series.length > 0 && <TitleShelf title="Series" items={results.series} kind="series" withSource />}
          {!addonsLead && addonShelves}
          {results.channels.length > 0 && (
            <Shelf title="Channels">
              {results.channels.map((c) => (
                <button key={c.id} className="card channel-card" onClick={() => playChannel(c)} onContextMenu={(e) => openMenu(e, channelMenu(c))}>
                  <Art className="card-art" fallback="">
                    <div className="logo-plate">{c.logoUrl ? <img src={imageSrc(c.logoUrl)} alt="" /> : <ChannelLogo name={c.name} size={60} />}</div>
                  </Art>
                  <div className="card-title" style={{ marginTop: 8, fontWeight: 600 }}>
                    {c.number != null ? `${c.number} · ` : ""}
                    {c.name}
                  </div>
                  {sourceName(c.sourceId) && <span className="source-tag">{sourceName(c.sourceId)}</span>}
                </button>
              ))}
            </Shelf>
          )}
        </>
      )}
    </div>
  );
}

/** A row of films or series; with `withSource`, each says which source it's from, as copies of one film each show. */
export function TitleShelf({ title, items, kind, withSource }: { title: string; items: (Movie | Series)[]; kind: "movie" | "series"; withSource?: boolean }) {
  const watchlistIds = useApp((s) => s.watchlistIds);
  const sourceName = useSourceName();
  const posters = useFilledPosters(items);
  return (
    <Shelf title={title}>
      {items.map((item) => (
        <PosterCard
          key={item.id}
          title={item.title}
          meta={[item.year, withSource ? sourceName(item.sourceId) || firstGenre(item.genre) : firstGenre(item.genre)].filter(Boolean).join(" · ")}
          posterUrl={item.posterUrl ?? posters.get(item.id)}
          saved={watchlistIds.has(item.id)}
          onClick={() => (kind === "movie" ? openMovie(item as Movie) : openSeries(item as Series))}
          onContextMenu={(e) => openMenu(e, kind === "movie" ? movieMenu(item as Movie) : seriesMenu(item as Series))}
        />
      ))}
    </Shelf>
  );
}

/** One listing: when and where, with Watch (or Show in Guide), Record and Remind right there. */
function ProgramResult({ hit, source }: { hit: ProgramHit; source: string }) {
  const { program, channel } = hit;
  const recordings = useApp((s) => s.recordings);
  const watchlistIds = useApp((s) => s.watchlistIds);
  const t = nowSeconds();
  const airing = program.start <= t && t < program.end;
  const recording = recordingFor(recordings, channel.id, program.start, program.id);
  const recordingActive = recording && (recording.status === "recording" || recording.status === "scheduled");
  return (
    <div className="program-result" onContextMenu={(e) => openMenu(e, programMenu(program, channel))}>
      <Art className="program-thumb" src={program.imageUrl} fallback="">
        {!program.imageUrl && (
          <div className="recording-thumb-logo">
            <ChannelLogo src={channel.logoUrl} name={channel.name} size={36} />
          </div>
        )}
      </Art>
      <div className="program-text">
        <div className="program-when">
          {airing ? <span className="live-pill">LIVE</span> : <span>{dayName(program.start)}</span>}
          <span>
            {clockTime(program.start)} – {clockTime(program.end)}
          </span>
          <span className="muted">· {duration(program.end - program.start)}</span>
        </div>
        <div className="program-title">{program.title}</div>
        <div className="program-channel">
          <ChannelLogo src={channel.logoUrl} name={channel.name} size={18} /> {channel.name}
          {source && <span className="source-tag">{source}</span>}
          {program.summary && <span className="program-summary"> — {program.summary}</span>}
        </div>
      </div>
      <div className="program-actions">
        {airing ? (
          <button className="btn-plain" onClick={() => playChannel(channel)}>
            <Play size={13} fill="currentColor" strokeWidth={0} /> Watch
          </button>
        ) : (
          <button className="btn-plain" onClick={() => showInGuide(hit)} title="Open Live TV's guide at this programme">
            <CalendarSearch size={13} /> Show in Guide
          </button>
        )}
        {recordingActive ? (
          <button className="btn-plain rec-active" onClick={() => cancelOrStopRecording(program, channel)}>
            <CircleStop size={13} /> {recording!.status === "recording" ? "Stop" : "Scheduled"}
          </button>
        ) : (
          <button className="btn-plain" onClick={() => recordProgram(program, channel)}>
            <CircleDot size={13} color="var(--recording)" /> Record
          </button>
        )}
        {!airing && (
          <button className={`btn-plain icon-only ${watchlistIds.has(program.id) ? "active" : ""}`} onClick={() => toggleWatchlist(programEntry(program, channel))} aria-label="Remind me" title="Remind me and add to watchlist">
            <Bell size={13} />
          </button>
        )}
      </div>
    </div>
  );
}
