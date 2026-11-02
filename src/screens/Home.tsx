import { ArrowDown, ArrowUp, Bell, BellOff, ChevronLeft, ChevronRight, CircleDot, EyeOff, Info, Play, Plus, Check, SlidersHorizontal, Sparkles, Star } from "lucide-react";
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { Art, ChannelLogo, placeholderGradient } from "../components/Artwork";
import { ContinueChip, LandscapeCard, NewEpisodeChip, PosterCard, Top10Card } from "../components/Cards";
import { HeroPreview } from "../components/HeroPreview";
import { HomeBanners } from "../components/HomeBanners";
import { openMenu, type MenuEntry } from "../components/Menu";
import { ScrollRow } from "../components/ScrollRow";
import { channelMenu, continueMenu, movieMenu, programMenu, seriesMenu } from "../components/menus";
import { Shelf } from "../components/Shelf";
import { movieEntry, openMovie, openSeries, playChannel, playMovie, recordProgram, resumeContinue, seriesEntry, toggleFavoriteChannel, toggleWatchlist } from "../lib/actions";
import { api } from "../lib/api";
import { clockTime, duration, firstGenre, nowSeconds, remaining, resolutionBadge, timeRange, whenLabel } from "../lib/format";
import { catalogKey, moved, rowNote, rowTitle, useHomeLayout, useHomeLayouts, type HomeLayout, type HomeMode, type HomeRow, type HomeRowKind } from "../lib/homeLayout";
import { imageSrc } from "../lib/images";
import { newEpisodeLabel, useNewEpisodeCounts } from "../lib/newEpisodes";
import { useFilledPosters } from "../lib/posterFill";
import { filtersAnything, useActiveProfile, useProfiles } from "../lib/profiles";
import { useSources } from "../lib/sources";
import { recordingFor, useApp } from "../lib/store";
import type { AddonCatalogRow, AddonInfo, Channel, ContinueItem, MediaDetails, Movie, Program, ProgramHit, Recording, Series, SourceSummary, TraktRowItem, WatchlistEntry } from "../lib/types";
import { isFavoriteChannel } from "../lib/versions";
import { useHomeKeyboard, type HeroControls } from "./homeKeyboard";
import { openMediaFolder } from "./MediaLibrary";
import "../styles/profiles.css";
import "../styles/home.css";

/**
 * A title featured in the hero: a film, a series or a programme from the
 * watchlist; or, when the sources carry only channels, what's on one now.
 */
type Featured =
  | { kind: "movie"; movie: Movie; progress?: ContinueItem }
  | { kind: "series"; series: Series; progress?: ContinueItem }
  | { kind: "program"; hit: ProgramHit }
  | { kind: "live"; channel: Channel; program: Program | null };

interface ProviderRows {
  top10: Movie[];
  recentMovies: Movie[];
  recentSeries: Series[];
  topSeries: Series[];
}

/**
 * Home's sections, each loaded on its own so the page fills in as they
 * arrive (as Xorvio for Mac's does). The last results are kept for the
 * session, so coming back to Home shows them at once while they refresh.
 */
const remembered: {
  addons?: AddonInfo[];
  continueItems?: ContinueItem[];
  watchlist?: WatchlistEntry[];
  addonRows?: AddonCatalogRow[];
  tonight?: ProgramHit[];
  provider?: ProviderRows;
  featured?: Featured[];
  sources?: SourceSummary[];
  hasChannels?: boolean;
} = {};

/** Rows that load their own titles (Home's newer rows), remembered the same way, by row id. */
const rowCache = new Map<string, unknown>();

/** The profile the remembered sections belong to: another profile's rows never show, even for a moment. */
let rememberedFor = -1;

function forgetOtherProfiles(revision: number) {
  if (rememberedFor === revision) return;
  for (const key of Object.keys(remembered)) delete remembered[key as keyof typeof remembered];
  rowCache.clear();
  rememberedFor = revision;
}

/** Rows made from the provider's films and series (the `provider` section). */
const PROVIDER_ROWS: HomeRowKind[] = ["top10Movies", "recentMovies", "topRatedSeries", "newSeries"];

/** Loads one section, starting from what's remembered. */
function useSection<K extends keyof typeof remembered>(key: K, load: () => Promise<NonNullable<(typeof remembered)[K]>>, deps: unknown[], enabled = true) {
  const [value, setValue] = useState<(typeof remembered)[K] | null>(remembered[key] ?? null);
  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    load()
      .then((v) => {
        remembered[key] = v;
        if (!cancelled) setValue(v);
      })
      .catch(() => !cancelled && setValue((remembered[key] ?? null) as (typeof remembered)[K] | null));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, enabled]);
  return value as NonNullable<(typeof remembered)[K]> | null;
}

function tonightWindow() {
  const now = nowSeconds();
  const midnight = new Date();
  midnight.setHours(24, 0, 0, 0);
  // After 6 PM "tonight" runs into the small hours.
  return [now + 5 * 60, Math.max(Math.floor(midnight.getTime() / 1000) + (new Date().getHours() >= 18 ? 3 * 3600 : 0), now + 6 * 3600)] as const;
}

export function HomeScreen() {
  const libraryRevision = useApp((s) => s.libraryRevision);
  const addonsLead = useApp((s) => s.settings.addonsLead);
  const profile = useActiveProfile();
  forgetOtherProfiles(useProfiles((s) => s.revision));
  const [genre, setGenre] = useState<string | null>(null);
  const [genreRows, setGenreRows] = useState<AddonCatalogRow[] | null>(null);

  // Local and quick: the add-ons list decides the layout before anything slow arrives.
  const addons = useSection("addons", () => api.addons(), []);
  const hasCatalogs = !!addons?.some((a) => a.enabled && a.catalogs.length > 0);
  const lead = addonsLead && hasCatalogs;
  const mode: HomeMode | null = addons === null ? null : lead ? "addons" : "sources";
  // The rows the profile's layout shows, in its order.
  const layout = useHomeLayout(mode);
  const shown = useMemo(() => layout?.rows.filter((row) => row.visible) ?? [], [layout]);
  const wants = (...kinds: HomeRowKind[]) => shown.some((row) => kinds.includes(row.kind));

  // The hero draws on Continue Watching, the watchlist and the add-on rows, so those always load.
  const watchlist = useSection("watchlist", () => api.watchlist(), [libraryRevision]);
  const tonight = useSection("tonight", () => api.upcomingPrograms(...tonightWindow(), 24), [libraryRevision], wants("tonight"));
  const continueItems = useSection("continueItems", () => api.continueWatching(20), [libraryRevision]);
  const addonRows = useSection("addonRows", () => api.discoverRows().then(orderAddonRows), []);

  // The provider's rows feed the hero unless add-ons lead.
  const provider = useSection(
    "provider",
    async () => {
      const [top, recentMovies, recentSeries, topSeries] = await Promise.all([
        api.topTitles("movie", 10).catch(() => ({ movies: [] as Movie[], series: [] as Series[] })),
        api.movies({ sort: "recentlyAdded", limit: 24, offset: 0 }).catch(() => []),
        api.seriesList({ sort: "recentlyAdded", limit: 24, offset: 0 }).catch(() => []),
        api.seriesList({ sort: "rating", limit: 24, offset: 0 }).catch(() => []),
      ]);
      return { top10: top.movies, recentMovies, recentSeries, topSeries };
    },
    [libraryRevision],
    addons !== null && (!lead || wants(...PROVIDER_ROWS)),
  );

  // For the banners, and to tell a library of channels alone.
  const sources = useSection("sources", () => api.listSources(), [libraryRevision]);
  const hasChannels = useSection("hasChannels", () => api.channels({ limit: 1 }).then((list) => list.length > 0), [libraryRevision]);
  // Channels but no films or series (or a profile showing none): the live rows lead, as on the Mac.
  const liveOnly =
    mode === "sources" && !!provider && hasChannels === true && !provider.top10.length && !provider.recentMovies.length && !provider.recentSeries.length && !provider.topSeries.length;
  useEffect(() => {
    if (mode === "sources" && provider && hasChannels !== null) useHomeLayouts.getState().setLiveOnly(liveOnly);
  }, [mode, provider, hasChannels, liveOnly]);

  const featured = useFeatured(lead, addonRows, provider, continueItems, watchlist);
  const root = useRef<HTMLDivElement>(null);
  const heroControls = useRef<HeroControls | null>(null);
  useHomeKeyboard(root, heroControls);

  // The popular film catalogue's genres become Home's chips.
  const { genres, genreCatalogs } = useMemo(() => {
    const genreCatalogs = new Set<string>();
    let genres: string[] = [];
    for (const addon of addons ?? [])
      for (const c of addon.catalogs)
        if (c.genres.length) {
          genreCatalogs.add(catalogKey({ addonId: addon.id, mediaType: c.mediaType, catalogId: c.id }));
          if (!genres.length && c.id === "top" && c.mediaType === "movie") genres = c.genres;
        }
    return { genres, genreCatalogs };
  }, [addons]);

  // A chosen genre narrows the add-on rows that accept one; the others step aside.
  useEffect(() => {
    if (!genre || !addonRows) return setGenreRows(null);
    let cancelled = false;
    setGenreRows(null);
    Promise.all(
      addonRows
        .filter((row) => genreCatalogs.has(catalogKey(row)))
        .map((row) => api.addonCatalog(row.addonId, row.mediaType, row.catalogId, genre, 0).then((r) => ({ ...r, title: `${row.title} · ${genre}` })).catch(() => null)),
    ).then((rows) => !cancelled && setGenreRows(rows.filter((r): r is AddonCatalogRow => !!r && r.movies.length + r.series.length > 0)));
    return () => {
      cancelled = true;
    };
  }, [genre, addonRows, genreCatalogs]);

  const settled = addons !== null && layout !== null && watchlist !== null && continueItems !== null && addonRows !== null && (lead || provider !== null);
  const empty =
    settled &&
    hasChannels === false &&
    !featured?.length &&
    !continueItems?.length &&
    !watchlist?.length &&
    !addonRows?.length &&
    !provider?.recentMovies.length &&
    !provider?.recentSeries.length;
  if (empty) return <div className="content home">{profile && filtersAnything(profile) ? <ProfileEmpty name={profile.name} /> : <HomeEmpty />}</div>;

  // Add-on rows given a row of their own leave the block of add-on rows.
  const placed = new Set(shown.flatMap((row) => (row.kind === "addonCatalog" && row.catalog ? [catalogKey(row.catalog)] : [])));
  const shownAddonRows = (genre ? genreRows ?? [] : addonRows ?? []).filter((row) => !placed.has(catalogKey(row)));
  const save = (change: (layout: HomeLayout) => HomeLayout) => {
    if (layout && mode) useHomeLayouts.getState().save(mode, change(layout)).catch(() => {});
  };
  const more = (row: HomeRow) => (e: React.MouseEvent) => openMenu(e, rowMenu(row, shown, save), true);

  const render = (row: HomeRow) => {
    const onMore = more(row);
    switch (row.kind) {
      case "continueWatching":
        return !!continueItems?.length && <ContinueRow items={continueItems} onMore={onMore} />;
      case "watchlist":
        return !!watchlist?.length && <WatchlistRow entries={watchlist} onMore={onMore} />;
      case "recordings":
        return <RecordingsRow onMore={onMore} />;
      case "tonight":
        return !!tonight?.length && <TonightRow hits={tonight} onMore={onMore} />;
      case "top10Movies":
        return (
          !!provider?.top10.length && (
            <Shelf title="Top 10 New Releases" subtitle="This year's films your sources carry the most copies of" onMore={onMore}>
              {provider.top10.map((m, i) => (
                <Top10Card key={m.id} rank={i + 1} title={m.title} posterUrl={m.posterUrl} onClick={() => openMovie(m)} onContextMenu={(e) => openMenu(e, movieMenu(m))} />
              ))}
            </Shelf>
          )
        );
      case "recentMovies":
        return !!provider?.recentMovies.length && <MovieRow title="Recently Added Movies" movies={provider.recentMovies} onShowAll={() => useApp.getState().go("movies")} onMore={onMore} />;
      case "topRatedSeries":
        return !!provider?.topSeries.length && <SeriesRow title="Top Rated Series" series={provider.topSeries} onMore={onMore} />;
      case "newSeries":
        return !!provider?.recentSeries.length && <SeriesRow title="New Series" series={provider.recentSeries} onShowAll={() => useApp.getState().go("series")} onMore={onMore} />;
      case "addonRows":
        return (
          <>
            {lead && genres.length > 0 && <GenreBar genres={genres} selected={genre} onSelect={setGenre} />}
            {shownAddonRows.map((addonRow) => (
              <AddonRow key={catalogKey(addonRow) + (genre ?? "")} row={addonRow} onMore={onMore} />
            ))}
          </>
        );
      case "addonCatalog": {
        const addonRow = addonRows?.find((r) => row.catalog && catalogKey(r) === catalogKey(row.catalog));
        return addonRow ? <AddonRow row={addonRow} onMore={onMore} /> : null;
      }
      case "onNow":
      case "favoriteChannels":
      case "recentChannels":
        return <LiveRow row={row} onMore={onMore} />;
      case "category":
        return row.categoryKind === "live" ? <LiveRow row={row} onMore={onMore} /> : <TitlesRow row={row} onMore={onMore} />;
      case "trakt":
        return <TraktRow row={row} onMore={onMore} />;
      case "library":
        return <LibraryRow row={row} onMore={onMore} />;
      case "top10Series":
      case "topRatedMovies":
      case "genreMovies":
      case "genreSeries":
      case "newReleases":
      case "newEpisodes":
        return <TitlesRow row={row} onMore={onMore} />;
    }
  };

  return (
    <div className="content home" ref={root}>
      {/* The hero's space is kept while it loads, so rows don't jump when it appears. */}
      {layout?.showHero === false ? null : featured?.length ? <Hero items={featured} controls={heroControls} /> : featured === null ? <section className="hero hero-loading" /> : null}
      <div className="home-rows" style={layout?.showHero === false ? { marginTop: 24 } : undefined}>
        <HomeBanners sources={sources} />
        {shown.map((row) => (
          <Fragment key={row.id}>{render(row)}</Fragment>
        ))}
        {layout && (
          <div className="home-customize-footer">
            <button className="btn-plain" onClick={() => useProfiles.getState().customizeHome(true)}>
              <SlidersHorizontal size={14} /> Customize Home
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

/** A row's "…" menu: hide it, move it past the rows around it, or customize all of Home. */
function rowMenu(row: HomeRow, shown: HomeRow[], save: (change: (layout: HomeLayout) => HomeLayout) => void): MenuEntry[] {
  const at = shown.findIndex((r) => r.id === row.id);
  const icon = { size: 15, strokeWidth: 1.8 };
  return [
    { heading: rowTitle(row) },
    { label: "Hide this row", icon: <EyeOff {...icon} />, onSelect: () => save((l) => ({ ...l, rows: l.rows.map((r) => (r.id === row.id ? { ...r, visible: false } : r)) })) },
    { label: "Move up", icon: <ArrowUp {...icon} />, disabled: at <= 0, onSelect: () => save((l) => moved(l, row.id, -1, true)) },
    { label: "Move down", icon: <ArrowDown {...icon} />, disabled: at < 0 || at >= shown.length - 1, onSelect: () => save((l) => moved(l, row.id, 1, true)) },
    { separator: true },
    { label: "Customize Home…", icon: <SlidersHorizontal {...icon} />, onSelect: () => useProfiles.getState().customizeHome(true) },
  ];
}

/** Loads a row's own titles, starting from what's remembered for it. */
function useRowData<T>(id: string, load: () => Promise<T>, deps: unknown[]): T | null {
  const [value, setValue] = useState<T | null>((rowCache.get(id) as T | undefined) ?? null);
  useEffect(() => {
    let cancelled = false;
    load()
      .then((v) => {
        rowCache.set(id, v);
        if (!cancelled) setValue(v);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, ...deps]);
  return value;
}

/** Films or series for a row: a genre, a category pinned to Home, Top 10 Series, Top Rated Movies, New Releases or New Episodes. */
function TitlesRow({ row, onMore }: { row: HomeRow; onMore: (e: React.MouseEvent) => void }) {
  const libraryRevision = useApp((s) => s.libraryRevision);
  const titles = useRowData(
    row.id,
    async (): Promise<{ movies: Movie[]; series: Series[] }> => {
      const page = { sort: "recentlyAdded" as const, limit: 30, offset: 0 };
      switch (row.kind) {
        case "top10Series":
          return { movies: [], series: (await api.topTitles("series", 10)).series };
        case "topRatedMovies":
          return { movies: await api.movies({ ...page, sort: "rating" }), series: [] };
        case "genreMovies":
          return { movies: await api.movies({ ...page, contentGenre: row.genre }), series: [] };
        case "genreSeries":
          return { movies: [], series: await api.seriesList({ ...page, contentGenre: row.genre }) };
        case "newReleases":
          return { movies: await api.newReleases(30), series: [] };
        case "newEpisodes":
          return { movies: [], series: await api.newEpisodeSeries(30) };
        default:
          return row.categoryKind === "series"
            ? { movies: [], series: await api.seriesList({ ...page, categoryId: row.categoryId }) }
            : { movies: await api.movies({ ...page, categoryId: row.categoryId }), series: [] };
      }
    },
    [libraryRevision],
  );
  if (!titles) return null;
  if (row.kind === "top10Series")
    return titles.series.length ? (
      <Shelf title={rowTitle(row)} subtitle="This year's series your sources carry the most copies of" onMore={onMore}>
        {titles.series.map((s, i) => (
          <Top10Card key={s.id} rank={i + 1} title={s.title} posterUrl={s.posterUrl} onClick={() => openSeries(s)} onContextMenu={(e) => openMenu(e, seriesMenu(s))} />
        ))}
      </Shelf>
    ) : null;
  // The newer rows say where their titles come from, as on the Mac.
  const subtitle = row.kind === "newReleases" || row.kind === "newEpisodes" ? (rowNote(row) ?? undefined) : undefined;
  if (titles.movies.length) return <MovieRow title={rowTitle(row)} subtitle={subtitle} movies={titles.movies} onMore={onMore} />;
  if (titles.series.length) return <SeriesRow title={rowTitle(row)} subtitle={subtitle} series={titles.series} onMore={onMore} />;
  return null;
}

/**
 * A media folder's or media server's films and series together, newest
 * first. Show All opens a media folder's own library page, and Movies (or
 * Series) as a media server's.
 */
function LibraryRow({ row, onMore }: { row: HomeRow; onMore: (e: React.MouseEvent) => void }) {
  const libraryRevision = useApp((s) => s.libraryRevision);
  const watchlistIds = useApp((s) => s.watchlistIds);
  const sourceId = row.sourceId;
  const isFolder = useSources((s) => s.sources?.find((summary) => summary.config.id === sourceId)?.config.kind === "folder");
  const titles = useRowData(
    row.id,
    async () => {
      if (!sourceId) return { movies: [] as Movie[], series: [] as Series[] };
      const page = { sort: "recentlyAdded" as const, limit: 30, offset: 0, sourceId };
      const [movies, series] = await Promise.all([api.movies(page).catch(() => [] as Movie[]), api.seriesList(page).catch(() => [] as Series[])]);
      return { movies, series };
    },
    [libraryRevision, sourceId],
  );
  const items = useMemo(
    () =>
      [...(titles?.movies ?? []).map((movie) => ({ movie, at: movie.addedAt ?? 0 })), ...(titles?.series ?? []).map((series) => ({ series, at: series.addedAt ?? 0 }))]
        .sort((a, b) => b.at - a.at)
        .slice(0, 30),
    [titles],
  );
  // A folder's videos without artwork get a frame of the video.
  const shownTitles = useMemo(() => items.map((item) => ("movie" in item ? item.movie : item.series)), [items]);
  const pictures = useFilledPosters(shownTitles);
  if (!titles || !sourceId || !items.length) return null;
  const showAll = () => (isFolder ? openMediaFolder(sourceId, null) : useApp.getState().browseSource(titles.movies.length ? "movie" : "series", sourceId));
  return (
    <Shelf title={rowTitle(row)} subtitle={rowNote(row) ?? undefined} onShowAll={showAll} onMore={onMore}>
      {items.map((item) => {
        const title = "movie" in item ? item.movie : item.series;
        return (
          <PosterCard
            key={title.id}
            title={title.title}
            meta={[title.year, firstGenre(title.genre)].filter(Boolean).join(" · ")}
            posterUrl={title.posterUrl ?? pictures.get(title.id) ?? null}
            saved={watchlistIds.has(title.id)}
            onClick={() => ("movie" in item ? openMovie(item.movie) : openSeries(item.series))}
            onContextMenu={(e) => openMenu(e, "movie" in item ? movieMenu(item.movie) : seriesMenu(item.series))}
          />
        );
      })}
    </Shelf>
  );
}

/** A Trakt chart as a Top 10, or a list or recommendations as posters, in Trakt's order, of what can be played. */
function TraktRow({ row, onMore }: { row: HomeRow; onMore: (e: React.MouseEvent) => void }) {
  const libraryRevision = useApp((s) => s.libraryRevision);
  // When add-ons lead, the row shows their titles (whose pages list every copy).
  const addonsLead = useApp((s) => s.settings.addonsLead);
  const watchlistIds = useApp((s) => s.watchlistIds);
  // Without an account the row asks Trakt nothing unless Settings › Trakt says to.
  const withoutAccount = useApp((s) => s.settings.traktRowsWithoutAccount);
  const source = row.trakt?.source;
  const chart = source?.type === "chart";
  const items = useRowData(row.id, async () => (source ? (await api.traktRow(source, chart ? 10 : 40)).items : []), [libraryRevision, addonsLead, withoutAccount]);
  if (!items?.length) return null;
  const trakt = row.trakt;
  const card = (item: TraktRowItem, i: number) => {
    const [title, open, menu] =
      "movie" in item
        ? ([item.movie, () => openMovie(item.movie), (e: React.MouseEvent) => openMenu(e, movieMenu(item.movie))] as const)
        : ([item.series, () => openSeries(item.series), (e: React.MouseEvent) => openMenu(e, seriesMenu(item.series))] as const);
    return chart ? (
      <Top10Card key={title.id} rank={i + 1} title={title.title} posterUrl={title.posterUrl} onClick={open} onContextMenu={menu} />
    ) : (
      <PosterCard
        key={title.id}
        title={title.title}
        meta={[title.year, firstGenre(title.genre)].filter(Boolean).join(" · ")}
        posterUrl={title.posterUrl}
        saved={watchlistIds.has(title.id)}
        onClick={open}
        onContextMenu={menu}
      />
    );
  };
  return (
    <Shelf title={rowTitle(row)} subtitle={rowNote(row) ?? undefined} onShowAll={trakt ? () => useApp.getState().push({ type: "trakt", trakt }) : undefined} onMore={onMore}>
      {items.map(card)}
    </Shelf>
  );
}

/** Channels with what's on each: On Now, favourites, recently watched, or a channel category pinned to Home. */
function LiveRow({ row, onMore }: { row: HomeRow; onMore: (e: React.MouseEvent) => void }) {
  const libraryRevision = useApp((s) => s.libraryRevision);
  const items = useRowData(row.id, () => liveItems(row), [libraryRevision]);
  if (!items?.length) return null;
  const t = nowSeconds();
  return (
    <Shelf title={rowTitle(row)} subtitle={row.kind === "onNow" ? "Live programmes airing right now" : undefined} onMore={onMore}>
      {items.map(({ channel, program }) => (
        <LandscapeCard
          key={channel.id}
          title={program?.title ?? channel.name}
          subtitle={program ? channel.name : null}
          imageUrl={program?.imageUrl}
          progress={program ? (t - program.start) / Math.max(1, program.end - program.start) : null}
          chip={
            <span className="chip" style={{ color: "#ff8a8d" }}>
              ● Live
            </span>
          }
          corner={<ChannelLogo src={channel.logoUrl} name={channel.name} size={30} />}
          onClick={() => playChannel(channel)}
          onContextMenu={(e) => openMenu(e, program ? programMenu(program, channel) : channelMenu(channel))}
        />
      ))}
    </Shelf>
  );
}

async function liveItems(row: HomeRow): Promise<{ channel: Channel; program: Program | null }[]> {
  if (row.kind === "onNow") return (await api.onNow(24)).map((hit) => ({ channel: hit.channel, program: hit.program }));
  const channels =
    row.kind === "favoriteChannels"
      ? await api.channels({ favoritesOnly: true, limit: 24 })
      : row.kind === "recentChannels"
        ? await api.recentChannels(24)
        : await api.channels({ categoryId: row.categoryId, limit: 30 });
  if (!channels.length) return [];
  const listings = await api.nowNext(channels.map((c) => c.id)).catch(() => []);
  const t = nowSeconds();
  return channels.map((channel) => ({
    channel,
    program: listings.find((r) => r.channel.id === channel.id)?.programs.find((p) => p.start <= t && t < p.end) ?? null,
  }));
}

/**
 * The hero's titles, worked out once their main source has arrived (the
 * add-on rows when add-ons lead, the provider's otherwise) so it doesn't
 * reshuffle while the page fills in. Partly watched films lead when they're
 * already known; then (unless add-ons lead, where live events stay off Home
 * as in Personal) programmes on soon, then popular titles.
 */
function useFeatured(lead: boolean, addonRows: AddonCatalogRow[] | null, provider: ProviderRows | null, continueItems: ContinueItem[] | null, watchlist: WatchlistEntry[] | null) {
  const [featured, setFeatured] = useState<Featured[] | null>(remembered.featured ?? null);
  const base = lead ? addonRows : provider;
  useEffect(() => {
    if (!base) return;
    let cancelled = false;
    (async () => {
      const now = nowSeconds();
      const items: Featured[] = [];
      for (const item of (continueItems ?? []).slice(0, 3)) {
        if (item.kind !== "movie") continue;
        const movie = await api.movie(item.itemId).catch(() => null);
        if (movie?.backdropUrl) items.push({ kind: "movie", movie, progress: item });
      }
      if (!lead)
        for (const entry of watchlist ?? []) {
          if (entry.kind !== "program" || !entry.start || !entry.end || entry.start > now + 3600 || entry.end <= now) continue;
          const hit = await api.program(entry.id).catch(() => null);
          if (hit) items.unshift({ kind: "program", hit });
        }
      if (lead) {
        for (const row of addonRows ?? [])
          for (const item of [...row.movies.map((movie) => ({ kind: "movie" as const, movie })), ...row.series.map((series) => ({ kind: "series" as const, series }))]) {
            const backdrop = item.kind === "movie" ? item.movie.backdropUrl : item.series.backdropUrl;
            if (backdrop && items.length < 6) items.push(item);
          }
      } else if (provider) {
        for (const movie of [...provider.top10.slice(0, 2), ...provider.recentMovies.slice(0, 2)])
          if (movie.backdropUrl && !items.some((f) => f.kind === "movie" && f.movie.id === movie.id)) items.push({ kind: "movie", movie });
        for (const series of provider.topSeries.slice(0, 1)) if (series.backdropUrl) items.push({ kind: "series", series });
      }
      // No films or series to feature: what's on the channels now, as on the Mac.
      if (!lead && !items.some((f) => f.kind === "movie" || f.kind === "series")) items.push(...(await liveFeatured()));
      if (cancelled) return;
      remembered.featured = items.slice(0, 6);
      setFeatured(remembered.featured);
    })();
    return () => {
      cancelled = true;
    };
    // Continue Watching joins the hero only if it arrived first, so the hero doesn't reshuffle.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [base, lead]);
  return featured;
}

/**
 * The hero for a library of channels alone: favourite channels with what's
 * on each, then other programmes airing, and with no guide at all, the
 * first channels themselves.
 */
async function liveFeatured(): Promise<Featured[]> {
  const t = nowSeconds();
  const favorites = await api.channels({ favoritesOnly: true, limit: 12 }).catch(() => [] as Channel[]);
  const listings = favorites.length ? await api.nowNext(favorites.map((c) => c.id)).catch(() => []) : [];
  const items: Featured[] = favorites.map((channel) => ({
    kind: "live",
    channel,
    program: listings.find((r) => r.channel.id === channel.id)?.programs.find((p) => p.start <= t && t < p.end) ?? null,
  }));
  if (items.length < 4)
    for (const hit of await api.onNow(12).catch(() => [] as ProgramHit[]))
      if (!items.some((f) => f.kind === "live" && f.channel.id === hit.channel.id)) items.push({ kind: "live", channel: hit.channel, program: hit.program });
  if (!items.length) for (const channel of await api.channels({ limit: 5 }).catch(() => [] as Channel[])) items.push({ kind: "live", channel, program: null });
  return items.slice(0, 5);
}

/** Genres as capsules, as in Xorvio Personal, narrowing Home's add-on rows. */
function GenreBar({ genres, selected, onSelect }: { genres: string[]; selected: string | null; onSelect: (g: string | null) => void }) {
  return (
    <ScrollRow className="genre-bar no-scrollbar" wrapClassName="genre-wrap">
      <button className={`genre-chip ${selected === null ? "selected" : ""}`} onClick={() => onSelect(null)}>
        All
      </button>
      {genres.map((g) => (
        <button key={g} className={`genre-chip ${selected === g ? "selected" : ""}`} onClick={() => onSelect(g)}>
          {g}
        </button>
      ))}
    </ScrollRow>
  );
}

/**
 * Personal's order for add-on rows: the popular catalogue first, then the
 * other add-ons' own rows, then the rest of the main catalogue add-on's.
 */
function orderAddonRows(rows: AddonCatalogRow[]): AddonCatalogRow[] {
  const main = rows.find((r) => r.catalogId === "top")?.addonId;
  const rank = (r: AddonCatalogRow) => (r.addonId === main && r.catalogId === "top" ? 0 : r.addonId !== main ? 1 : 2);
  return [...rows].sort((a, b) => rank(a) - rank(b));
}

function featuredId(item: Featured) {
  switch (item.kind) {
    case "movie":
      return item.movie.id;
    case "series":
      return item.series.id;
    case "program":
      return item.hit.program.id;
    case "live":
      return `live-${item.channel.id}`;
  }
}

function Hero({ items, controls }: { items: Featured[]; controls?: React.RefObject<HeroControls | null> }) {
  const [index, setIndex] = useState(0);
  const [details, setDetails] = useState<Record<string, MediaDetails>>({});
  // A preview clip playing holds the featured title until it ends.
  const [previewing, setPreviewing] = useState(false);
  const previewClips = useApp((s) => s.settings.previewClips);
  const item = items[Math.min(index, items.length - 1)];
  const id = featuredId(item);

  const step = (by: 1 | -1) => setIndex((i) => (i + by + items.length) % items.length);
  useEffect(() => {
    if (!controls) return;
    controls.current = { step: (by) => setIndex((i) => (i + by + items.length) % items.length) };
    return () => {
      controls.current = null;
    };
  }, [controls, items.length]);

  // Featured titles change every 12 seconds (counting again after a manual
  // change); the backdrop itself never moves.
  useEffect(() => {
    if (items.length < 2 || previewing) return;
    const timer = setInterval(() => setIndex((i) => (i + 1) % items.length), 12000);
    return () => clearInterval(timer);
  }, [items.length, index, previewing]);

  useEffect(() => {
    if (item.kind === "program" || item.kind === "live" || details[id]) return;
    api.details(id).then((d) => setDetails((all) => ({ ...all, [id]: d }))).catch(() => {});
  }, [id, item.kind, details]);

  const image = heroImage(item);
  const channel = item.kind === "live" ? item.channel : item.kind === "program" ? item.hit.channel : null;
  return (
    <section className="hero" aria-roledescription="carousel">
      <div className="hero-backdrop" key={id}>
        {channel && !image ? <ChannelAmbience channel={channel} /> : <Art className="hero-fade" src={image} eager />}
        {previewClips && item.kind === "movie" && <HeroPreview movie={item.movie} onPlaying={setPreviewing} />}
      </div>
      <div className="hero-content hero-fade" key={`text-${id}`}>
        {item.kind === "program" ? (
          <ProgramHero hit={item.hit} />
        ) : item.kind === "live" ? (
          <LiveHero channel={item.channel} program={item.program} />
        ) : (
          <TitleHero item={item} details={details[id]} />
        )}
      </div>
      {items.length > 1 && (
        <>
          <button className="hero-arrow left glass" onClick={() => step(-1)} aria-label="Previous featured title">
            <ChevronLeft size={20} />
          </button>
          <button className="hero-arrow right glass" onClick={() => step(1)} aria-label="Next featured title">
            <ChevronRight size={20} />
          </button>
        </>
      )}
      {items.length > 1 && (
        <div className="hero-dots" role="tablist">
          {items.map((_, i) => (
            <button key={i} className={i === index ? "current" : ""} onClick={() => setIndex(i)} aria-label={`Featured ${i + 1}`} role="tab" aria-selected={i === index} />
          ))}
        </div>
      )}
    </section>
  );
}

function heroImage(item: Featured) {
  if (item.kind === "movie") return item.movie.backdropUrl ?? item.movie.posterUrl;
  if (item.kind === "series") return item.series.backdropUrl ?? item.series.posterUrl;
  if (item.kind === "live") return item.program?.imageUrl ?? null;
  return item.hit.program.imageUrl;
}

/** A programme without artwork: the channel's colours, with its logo large at the right, as on the Mac. */
function ChannelAmbience({ channel }: { channel: Channel }) {
  return (
    <div className="hero-ambience hero-fade" style={{ background: placeholderGradient(channel.name) }}>
      <ChannelLogo src={channel.logoUrl} name={channel.name} size={200} />
    </div>
  );
}

function TitleHero({ item, details }: { item: Extract<Featured, { kind: "movie" | "series" }>; details?: MediaDetails }) {
  const watchlistIds = useApp((s) => s.watchlistIds);
  const title = item.kind === "movie" ? item.movie : item.series;
  const saved = watchlistIds.has(title.id);
  const progress = item.progress;
  const kindLabel = item.kind === "movie" ? "Film" : "Series";
  const eyebrow = [kindLabel, firstGenre(title.genre), title.year].filter(Boolean).join(" · ");
  const runtime = item.kind === "movie" ? item.movie.durationSeconds ?? (details?.runtimeMinutes ? details.runtimeMinutes * 60 : null) : null;
  const resolution = resolutionBadge(details?.video?.height);
  // TMDB's (or the provider's detail record's) when it has come; the list's until then.
  const overview = details?.overview ?? title.plot;

  return (
    <>
      <div className="hero-eyebrow">{eyebrow}</div>
      {details?.logoUrl ? <img className="hero-logo" src={imageSrc(details.logoUrl)} alt={title.title} /> : <h1 className={`hero-title ${title.title.length > 24 ? "long" : ""}`}>{title.title}</h1>}
      <div className="hero-facts">
        {runtime ? <span>{duration(runtime)}</span> : null}
        {title.rating ? (
          <span>
            ★ {title.rating.toFixed(1)}
          </span>
        ) : null}
        <span className="badges">
          {details?.ageRating && <span className="badge">{details.ageRating}</span>}
          {resolution && <span className="badge">{resolution}</span>}
          {details?.video?.isHdr && <span className="badge">HDR</span>}
          {details?.audioChannels && details.audioChannels > 2 && <span className="badge">{details.audioChannels === 6 ? "5.1" : details.audioChannels === 8 ? "7.1" : `${details.audioChannels}ch`}</span>}
        </span>
      </div>
      {overview && <p className="hero-overview">{overview}</p>}
      <div className="hero-actions">
        <button
          className="btn btn-primary"
          onClick={() => (item.kind === "movie" ? playMovie(item.movie) : progress ? resumeContinue(progress) : openSeries(item.series))}
        >
          <Play size={17} fill="currentColor" strokeWidth={0} /> {progress ? "Resume" : "Play"}
        </button>
        <button className="btn btn-glass" onClick={() => (item.kind === "movie" ? openMovie(item.movie) : openSeries(item.series))}>
          <Info size={17} strokeWidth={2} /> More Info
        </button>
        <button
          className="btn btn-glass btn-icon"
          aria-label={saved ? "Remove from Watchlist" : "Add to Watchlist"}
          title={saved ? "Remove from Watchlist" : "Add to Watchlist"}
          onClick={() => toggleWatchlist(item.kind === "movie" ? movieEntry(item.movie) : seriesEntry(item.series))}
        >
          {saved ? <Check size={18} strokeWidth={2.2} /> : <Plus size={19} strokeWidth={2} />}
        </button>
        {progress && progress.progress > 0 && (
          <div className="hero-progress" style={{ marginTop: 0, marginLeft: 8 }}>
            <div className="progress-track">
              <div style={{ width: `${progress.progress * 100}%` }} />
            </div>
          </div>
        )}
      </div>
    </>
  );
}

function ProgramHero({ hit }: { hit: ProgramHit }) {
  const { program, channel } = hit;
  const recordings = useApp((s) => s.recordings);
  const airing = program.start <= nowSeconds();
  const recording = recordingFor(recordings, channel.id, program.start, program.id);
  return (
    <>
      <div className="hero-eyebrow">
        {airing ? <span className="live-pill">LIVE</span> : <span>{whenLabel(program.start)}</span>}
        <span>{channel.name}</span>
      </div>
      <h1 className="hero-title">{program.title}</h1>
      <div className="hero-facts">
        <span>{duration(program.end - program.start)}</span>
        {program.category && <span className="dot-sep">{program.category}</span>}
        {recording && (
          <span className={`rec-badge ${recording.status === "recording" ? "active" : ""}`}>
            <span className="dot" /> {recording.status === "recording" ? "REC" : "SCHEDULED"}
          </span>
        )}
      </div>
      {program.summary && <p className="hero-overview">{program.summary}</p>}
      <div className="hero-actions">
        {airing ? (
          <button className="btn btn-primary" onClick={() => playChannel(channel)}>
            <Play size={17} fill="currentColor" strokeWidth={0} /> Watch Live
          </button>
        ) : (
          <button className="btn btn-primary" onClick={() => playChannel(channel)}>
            <Play size={17} fill="currentColor" strokeWidth={0} /> Watch {channel.name}
          </button>
        )}
        {!recording && (
          <button className="btn btn-glass" onClick={() => recordProgram(program, channel)}>
            <CircleDot size={17} strokeWidth={2.2} color="var(--recording)" /> {airing ? "Record Now" : "Record"}
          </button>
        )}
        <button className="btn btn-glass btn-icon" aria-label="Options" onClick={(e) => openMenu(e, programMenu(program, channel), true)}>
          <Bell size={17} />
        </button>
      </div>
    </>
  );
}

/** What's on a channel now, featured when the sources carry only channels. */
function LiveHero({ channel, program }: { channel: Channel; program: Program | null }) {
  const favorites = useApp((s) => s.favorites);
  const recordings = useApp((s) => s.recordings);
  const favorite = isFavoriteChannel(favorites, channel);
  const t = nowSeconds();
  const recording = program ? recordingFor(recordings, channel.id, program.start, program.id) : undefined;
  const title = program?.title ?? channel.name;
  return (
    <>
      <div className="hero-eyebrow">
        <span className="live-pill">LIVE</span>
        <span>{[channel.name, program ? timeRange(program.start, program.end) : null].filter(Boolean).join(" · ")}</span>
      </div>
      <h1 className={`hero-title ${title.length > 24 ? "long" : ""}`}>{title}</h1>
      {program && (
        <div className="hero-facts">
          {program.episode && <span>{program.episode}</span>}
          <span>{Math.max(1, Math.round((program.end - t) / 60))} min left</span>
          {program.category && <span className="dot-sep">{program.category}</span>}
          <span className="badges">
            {program.flags.isNew && <span className="badge">NEW</span>}
            {program.ageRating && <span className="badge">{program.ageRating}</span>}
          </span>
          {recording && (
            <span className={`rec-badge ${recording.status === "recording" ? "active" : ""}`}>
              <span className="dot" /> {recording.status === "recording" ? "REC" : "SCHEDULED"}
            </span>
          )}
        </div>
      )}
      {program?.summary && <p className="hero-overview">{program.summary}</p>}
      <div className="hero-actions">
        <button className="btn btn-primary" onClick={() => playChannel(channel)}>
          <Play size={17} fill="currentColor" strokeWidth={0} /> Watch Live
        </button>
        {program && !recording && (
          <button className="btn btn-glass" onClick={() => recordProgram(program, channel)}>
            <CircleDot size={17} strokeWidth={2.2} color="var(--recording)" /> Record Now
          </button>
        )}
        <button
          className="btn btn-glass btn-icon"
          aria-label={favorite ? "Remove from Favorites" : "Add to Favorites"}
          title={favorite ? "Remove from Favorites" : "Add to Favorites"}
          onClick={() => toggleFavoriteChannel(channel)}
        >
          <Star size={18} strokeWidth={2} fill={favorite ? "currentColor" : "none"} />
        </button>
        {program && (
          <div className="hero-progress" style={{ marginTop: 0, marginLeft: 8 }}>
            <div className="progress-track">
              <div style={{ width: `${Math.min(100, Math.max(0, ((t - program.start) / Math.max(1, program.end - program.start)) * 100))}%` }} />
            </div>
          </div>
        )}
      </div>
    </>
  );
}

/** A row's "…" menu on Home. */
type RowMenu = { onMore?: (e: React.MouseEvent) => void };

function ContinueRow({ items, onMore }: { items: ContinueItem[] } & RowMenu) {
  const push = useApp((s) => s.push);
  return (
    <Shelf title="Continue Watching" onShowAll={() => push({ type: "continue" })} onMore={onMore}>
      {items.map((item) => (
        <LandscapeCard
          key={item.itemId}
          title={item.title}
          subtitle={item.subtitle}
          imageUrl={item.artworkUrl}
          progress={item.upNext ? null : item.progress}
          progressLabel={item.remainingSeconds ? remaining(item.remainingSeconds) : null}
          chip={<ContinueChip item={item} />}
          onClick={() => resumeContinue(item)}
          onContextMenu={(e) => openMenu(e, continueMenu(item))}
        />
      ))}
    </Shelf>
  );
}

/** Programmes as wide cards (their artwork is landscape), then films and series as posters. */
function WatchlistRow({ entries, onMore }: { entries: WatchlistEntry[] } & RowMenu) {
  const go = useApp((s) => s.go);
  const recordings = useApp((s) => s.recordings);
  const reminderIds = useApp((s) => s.reminderIds);
  const newEpisodes = useNewEpisodeCounts();
  const programs = entries.filter((e) => e.kind === "program");
  const titles = entries.filter((e) => e.kind !== "program");
  return (
    <Shelf title="Watchlist" onShowAll={() => go("watchlist")} onMore={onMore}>
      {programs.map((entry) => {
        const recording = entry.channelId && entry.start ? recordingFor(recordings, entry.channelId, entry.start, entry.id) : undefined;
        const airing = entry.start && entry.start <= nowSeconds();
        return (
          <LandscapeCard
            key={entry.id}
            title={entry.title}
            subtitle={entry.subtitle}
            imageUrl={entry.artworkUrl}
            chip={
              entry.start ? (
                airing ? (
                  <span className="chip" style={{ color: "#ff8a8d" }}>
                    ● Live now
                  </span>
                ) : (
                  <span className="chip">
                    {reminderIds.has(entry.id) ? <Bell size={10} strokeWidth={2.4} /> : <BellOff size={10} strokeWidth={2.4} />} {whenLabel(entry.start)}
                  </span>
                )
              ) : null
            }
            corner={recording ? <span className={`rec-badge ${recording.status === "recording" ? "active" : ""}`} style={{ background: "rgba(10,12,16,0.62)" }}><span className="dot" />REC</span> : null}
            onClick={() => openWatchlistEntry(entry)}
            onContextMenu={(e) => watchlistEntryMenu(e, entry)}
          />
        );
      })}
      {titles.map((entry) => (
        <PosterCard
          key={entry.id}
          title={entry.title}
          meta={entry.subtitle ?? (entry.kind === "series" ? "Series" : "Film")}
          posterUrl={entry.artworkUrl}
          eager
          badge={entry.kind === "series" && newEpisodes[entry.id] ? <NewEpisodeChip label={newEpisodeLabel(newEpisodes[entry.id])} /> : null}
          onClick={() => openWatchlistEntry(entry)}
          onContextMenu={(e) => watchlistEntryMenu(e, entry)}
        />
      ))}
    </Shelf>
  );
}

export async function openWatchlistEntry(entry: WatchlistEntry) {
  if (entry.kind === "movie") {
    const movie = await api.movie(entry.id);
    if (movie) openMovie(movie);
  } else if (entry.kind === "series") {
    const series = await api.series(entry.id);
    if (series) openSeries(series);
  } else if (entry.kind === "program" && entry.channelId) {
    // On now: watch it. Later: show it, with Record and Remind.
    const hit = await api.program(entry.id).catch(() => null);
    if (hit && hit.program.start > nowSeconds()) useApp.getState().showProgram(hit);
    else {
      const channel = hit?.channel ?? (await api.channel(entry.channelId));
      if (channel) playChannel(channel);
    }
  }
}

export async function watchlistEntryMenu(e: React.MouseEvent, entry: WatchlistEntry) {
  e.preventDefault();
  const point = { clientX: e.clientX, clientY: e.clientY, preventDefault() {}, stopPropagation() {} };
  if (entry.kind === "movie") {
    const movie = await api.movie(entry.id);
    if (movie) openMenu(point, movieMenu(movie));
  } else if (entry.kind === "series") {
    const series = await api.series(entry.id);
    if (series) openMenu(point, seriesMenu(series));
  } else if (entry.kind === "program") {
    const hit = await api.program(entry.id);
    if (hit) openMenu(point, programMenu(hit.program, hit.channel, entry));
    else openMenu(point, [{ label: "Remove from Watchlist", onSelect: () => useApp.getState().setWatchlisted(entry, false) }]);
  }
}

/** What's coming up later today, ready to record from the tile. */
function TonightRow({ hits, onMore }: { hits: ProgramHit[] } & RowMenu) {
  const recordings = useApp((s) => s.recordings);
  const showProgram = useApp((s) => s.showProgram);
  return (
    <Shelf title="Tonight on TV" subtitle="Films, sport and events coming up. Record them so they're waiting for you." onMore={onMore}>
      {hits.map((hit) => {
        const { program, channel } = hit;
        const recording = recordingFor(recordings, channel.id, program.start, program.id);
        const active = recording && (recording.status === "scheduled" || recording.status === "recording");
        return (
          <LandscapeCard
            key={program.id}
            title={program.title}
            subtitle={channel.name}
            imageUrl={program.imageUrl}
            chip={<span className="chip">{clockTime(program.start)}</span>}
            corner={
              active ? (
                <span className="rec-badge" style={{ background: "rgba(10,12,16,0.62)" }}>
                  <span className="dot" />
                  {recording!.status === "recording" ? "REC" : "WILL RECORD"}
                </span>
              ) : (
                <span
                  role="button"
                  className="card-record glass"
                  title="Record"
                  onClick={(e) => {
                    e.stopPropagation();
                    recordProgram(program, channel);
                  }}
                >
                  <CircleDot size={14} color="var(--recording)" strokeWidth={2.4} /> Record
                </span>
              )
            }
            onClick={() => showProgram(hit)}
            onContextMenu={(e) => openMenu(e, programMenu(program, channel))}
          />
        );
      })}
    </Shelf>
  );
}

function RecordingsRow({ onMore }: RowMenu) {
  const recordings = useApp((s) => s.recordings);
  const go = useApp((s) => s.go);
  const list = useMemo(
    () =>
      [...recordings.values()]
        .filter((r) => r.status === "recording" || r.status === "completed" || r.status === "partial")
        .sort((a, b) => (a.status === "recording" ? -1 : b.status === "recording" ? 1 : b.start - a.start))
        .slice(0, 12),
    [recordings],
  );
  if (!list.length) return null;
  return (
    <Shelf title="Recordings" onShowAll={() => go("recordings")} onMore={onMore}>
      {list.map((r: Recording) => (
        <LandscapeCard
          key={r.id}
          title={r.title}
          subtitle={`${r.channelName} · ${whenLabel(r.start)}`}
          imageUrl={r.artworkUrl}
          chip={r.status === "recording" ? <span className="rec-badge active" style={{ background: "rgba(10,12,16,0.62)" }}><span className="dot" />REC</span> : null}
          onClick={() => go("recordings")}
        />
      ))}
    </Shelf>
  );
}

function MovieRow({ title, subtitle, movies, onShowAll, onMore }: { title: string; subtitle?: string; movies: Movie[]; onShowAll?: () => void } & RowMenu) {
  const watchlistIds = useApp((s) => s.watchlistIds);
  return (
    <Shelf title={title} subtitle={subtitle} onShowAll={onShowAll} onMore={onMore}>
      {movies.map((m) => (
        <PosterCard key={m.id} title={m.title} meta={[m.year, firstGenre(m.genre)].filter(Boolean).join(" · ")} posterUrl={m.posterUrl} saved={watchlistIds.has(m.id)} onClick={() => openMovie(m)} onContextMenu={(e) => openMenu(e, movieMenu(m))} />
      ))}
    </Shelf>
  );
}

function SeriesRow({ title, subtitle, series, onShowAll, onMore }: { title: string; subtitle?: string; series: Series[]; onShowAll?: () => void } & RowMenu) {
  const watchlistIds = useApp((s) => s.watchlistIds);
  const newEpisodes = useNewEpisodeCounts();
  return (
    <Shelf title={title} subtitle={subtitle} onShowAll={onShowAll} onMore={onMore}>
      {series.map((s) => (
        <PosterCard
          key={s.id}
          title={s.title}
          meta={[s.year, firstGenre(s.genre)].filter(Boolean).join(" · ")}
          posterUrl={s.posterUrl}
          saved={watchlistIds.has(s.id)}
          badge={newEpisodes[s.id] ? <NewEpisodeChip label={newEpisodeLabel(newEpisodes[s.id])} /> : null}
          onClick={() => openSeries(s)}
          onContextMenu={(e) => openMenu(e, seriesMenu(s))}
        />
      ))}
    </Shelf>
  );
}

function AddonRow({ row, onMore }: { row: AddonCatalogRow } & RowMenu) {
  if (row.movies.length) return <MovieRow title={row.title} movies={row.movies} onMore={onMore} />;
  if (row.series.length) return <SeriesRow title={row.title} series={row.series} onMore={onMore} />;
  return null;
}

/** The profile leaves Home nothing to show. */
function ProfileEmpty({ name }: { name: string }) {
  const { editProfiles, customizeHome } = useProfiles.getState();
  return (
    <div className="empty-state" style={{ minHeight: "70vh" }}>
      <div>
        <h3>Nothing to show in “{name}”</h3>
        <p>This profile leaves out everything Home's rows would show. Change what it shows, turn on other rows, or switch profile at the foot of the sidebar.</p>
        <div style={{ display: "flex", gap: 10, justifyContent: "center" }}>
          <button className="btn btn-primary" onClick={() => editProfiles()}>
            Edit profile
          </button>
          <button className="btn btn-glass" onClick={() => customizeHome(true)}>
            <SlidersHorizontal size={16} /> Customize Home
          </button>
        </div>
      </div>
    </div>
  );
}

function HomeEmpty() {
  const go = useApp((s) => s.go);
  return (
    <div className="empty-state" style={{ minHeight: "70vh" }}>
      <div>
        <h3>Welcome to Xorvio</h3>
        <p>Add an M3U playlist or an Xtream Codes login to bring your channels, films and series together, or try the demo library first.</p>
        <div style={{ display: "flex", gap: 10, justifyContent: "center" }}>
          <button className="btn btn-primary" onClick={() => go("sources")}>
            <Plus size={17} /> Add a Source
          </button>
          <button
            className="btn btn-glass"
            onClick={async () => {
              await api.addDemoSource();
              await api.refreshSource("demo").catch(() => {});
              useApp.getState().loadLibrary();
            }}
          >
            <Sparkles size={16} /> Try the Demo
          </button>
        </div>
      </div>
    </div>
  );
}
