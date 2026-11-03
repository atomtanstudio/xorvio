import { ArrowLeft, Bookmark, Clapperboard, CircleDot, Compass, House, Monitor, PanelLeft, Plus, Radio, RadioTower, Search, Settings as SettingsIcon } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import "./styles/sources.css";
import { BrandLockup } from "./components/Artwork";
import { ConfirmHost } from "./components/Confirm";
import { EditSheetHost } from "./components/EditSheets";
import { GuideMatchSheet } from "./components/GuideMatchSheet";
import { HomeLayoutSheetHost } from "./components/HomeLayoutSheet";
import { MenuHost } from "./components/Menu";
import { ProgramSheet } from "./components/ProgramSheet";
import { ProfileSwitcher, ProfilesSheetHost } from "./components/ProfilesSheet";
import { SeriesSheetHost } from "./components/SeriesSheet";
import { lastChannel, openLaunchScreen, playChannel, playEpisode, playLastChannel } from "./lib/actions";
import { api } from "./lib/api";
import { useRelock } from "./lib/channelEdits";
import { canPerform, followMenu, followMenuState, handleCommands, perform, resendMenuState, usePlaybackReport } from "./lib/commands";
import { traktPageTitle } from "./lib/homeLayout";
import { useProfiles } from "./lib/profiles";
import { failingSources, followSources, useSources } from "./lib/sources";
import { useApp, type Page, type Tab } from "./lib/store";
import type { AddonInfo, MenuState } from "./lib/types";
import { reportPlaybackSupport } from "./player/engine";
import { leavePlayerFullscreen } from "./player/fullscreen";
import { MultiviewHost } from "./player/Multiview";
import { allPaused, canInset } from "./player/multiviewRules";
import { closeMultiview, heardTile, insetCommand, swapInset, tileChannel, toggleExpand, toggleInset, toggleMultiview, useMultiview } from "./player/multiviewStore";
import { LIVE_EDGE } from "./player/playerRules";
import { PlayerHost } from "./player/Player";
import { OnboardingHost } from "./screens/Onboarding";
import { BrowseScreen } from "./screens/Browse";
import { ContinueWatchingScreen } from "./screens/ContinueWatching";
import { CopiesSheetHost } from "./screens/Copies";
import { DiscoverScreen } from "./screens/Discover";
import { HomeScreen } from "./screens/Home";
import { LiveTVScreen } from "./screens/LiveTV";
import { MovieScreen } from "./screens/MovieDetail";
import { RecordingsScreen } from "./screens/Recordings";
import { PersonScreen } from "./screens/Person";
import { MediaLibraryScreen } from "./screens/MediaLibrary";
import { TraktListScreen } from "./screens/TraktList";
import { SearchScreen } from "./screens/Search";
import { SeriesScreen } from "./screens/SeriesDetail";
import { SettingsScreen } from "./screens/Settings";
import { SourcesScreen } from "./screens/Sources";
import { WatchlistScreen } from "./screens/Watchlist";

const tabTitles: Record<Tab, string> = {
  home: "Home",
  watchlist: "Watchlist",
  live: "Live TV",
  movies: "Movies",
  series: "Series",
  recordings: "Recordings",
  discover: "Discover",
  sources: "Sources",
  settings: "Settings",
};

export function App() {
  const tab = useApp((s) => s.tab);
  const stack = useApp((s) => s.stack);
  const settings = useApp((s) => s.settings);
  const pageTitles = useApp((s) => s.pageTitles);
  // Every screen starts over under a new profile, so none keeps another profile's library.
  const profileRevision = useProfiles((s) => s.revision);
  const [sidebarHidden, setSidebarHidden] = useState(false);
  // First run's welcome waits for the settings, which say whether it's been seen.
  const [settingsLoaded, setSettingsLoaded] = useState(false);

  useTheme(settings.appearance, settings.density);
  useRelock();

  useEffect(() => {
    const { loadSettings, loadLibrary, upsertRecording, guideChanged } = useApp.getState();
    // The launch screen applies once, with the settings first loaded, not as they change.
    loadSettings()
      .then(() => openLaunchScreen(useApp.getState().settings.launchScreen))
      .catch(() => {})
      .finally(() => setSettingsLoaded(true));
    const offSources = followSources();
    const offMenu = followMenu();
    loadLibrary().catch(() => {});
    useProfiles.getState().load().catch(() => {});
    reportPlaybackSupport();
    // A restore happens across a restart; say how it went once the app is back.
    api
      .backupStatus()
      .then(({ restoreNotice }) => restoreNotice && useApp.getState().showToast(restoreNotice.message, restoreNotice.restored ? "info" : "error"))
      .catch(() => {});
    const offRecording = api.onRecordingChanged(upsertRecording);
    // Refreshes can bring new listings, so the guide reloads too.
    const offLibrary = api.onLibraryChanged(() => {
      loadLibrary().catch(() => {});
      guideChanged();
    });
    const offNavigate = api.onNavigate((tab) => useApp.getState().go(tab as Tab));
    const offPlayChannel = api.onPlayChannel(async (channelId) => {
      const channel = await api.channel(channelId).catch(() => null);
      if (channel) playChannel(channel);
    });
    // A new-episode notification: the series (from the Watchlist, as on the
    // Mac), or with its Play button, the first new episode.
    const offOpenSeries = api.onOpenSeries((seriesId) => {
      useApp.getState().go("watchlist");
      useApp.getState().push({ type: "series", id: seriesId });
    });
    const offPlayEpisode = api.onPlayEpisode(async ({ seriesId, episodeId }) => {
      const [series, episodes] = await Promise.all([api.series(seriesId).catch(() => null), api.episodes(seriesId).catch(() => [])]);
      const episode = episodes.find((e) => e.id === episodeId);
      if (episode) playEpisode(episode, series);
      else useApp.getState().showToast("That episode isn't in your sources any more.", "error");
    });
    // The pop-out player hands its video back when it's docked, and plays
    // what's chosen here until then.
    const offDock = api.onDock(({ item, position }) => {
      resendMenuState();
      useApp.setState({ poppedOut: false });
      useApp.getState().play({ ...item, resumeFrom: position > 0 ? position : undefined }, "full");
    });
    const offPopOutClosed = api.onPopOutClosed(() => {
      resendMenuState();
      useApp.setState({ poppedOut: false });
    });
    return () => {
      offSources();
      offMenu();
      offRecording();
      offLibrary();
      offNavigate();
      offPlayChannel();
      offOpenSeries();
      offPlayEpisode();
      offDock();
      offPopOutClosed();
    };
  }, []);

  useKeyboardShortcuts();
  const toggleSidebar = useCallback(() => setSidebarHidden((h) => !h), []);
  useShellCommands(toggleSidebar, sidebarHidden);

  const top = stack[stack.length - 1];
  return (
    <div className={`app ${sidebarHidden ? "sidebar-hidden" : ""}`}>
      <Sidebar onToggle={() => setSidebarHidden(true)} />
      <button className="floating-sidebar-toggle" onClick={() => setSidebarHidden(false)} aria-label="Show sidebar">
        <PanelLeft size={17} strokeWidth={1.6} />
      </button>
      <main className="main">
        <Toolbar title={top ? pageTitle(top) || pageTitles[pageKey(top)] || "" : tabTitles[tab]} canGoBack={stack.length > 0} sidebarHidden={sidebarHidden} />
        <div key={profileRevision} style={{ display: top ? "none" : "contents" }}>
          <TabScreen tab={tab} />
        </div>
        {top && <PageScreen key={`${profileRevision}-${stack.length}${pageKey(top)}`} page={top} />}
      </main>
      <PlayerHost />
      <MultiviewHost />
      <MenuHost />
      <ProgramSheetHost />
      <GuideMatchSheetHost />
      <CopiesSheetHost />
      <EditSheetHost />
      <ProfilesSheetHost />
      <HomeLayoutSheetHost />
      <SeriesSheetHost />
      <OnboardingHost ready={settingsLoaded} />
      <ConfirmHost />
      <Toast />
    </div>
  );
}

function TabScreen({ tab }: { tab: Tab }) {
  switch (tab) {
    case "home":
      return <HomeScreen />;
    case "watchlist":
      return <WatchlistScreen />;
    case "live":
      return <LiveTVScreen />;
    case "movies":
      return <BrowseScreen key="movie" kind="movie" />;
    case "series":
      return <BrowseScreen key="series" kind="series" />;
    case "recordings":
      return <RecordingsScreen />;
    case "discover":
      return <DiscoverScreen />;
    case "sources":
      return <SourcesScreen />;
    case "settings":
      return <SettingsScreen />;
  }
}

function PageScreen({ page }: { page: Page }) {
  switch (page.type) {
    case "movie":
      return <MovieScreen id={page.id} />;
    case "series":
      return <SeriesScreen id={page.id} />;
    case "search":
      return <SearchScreen query={page.query} />;
    case "continue":
      return <ContinueWatchingScreen />;
    case "person":
      return <PersonScreen name={page.name} tmdbId={page.tmdbId} fromId={page.fromId} />;
    case "trakt":
      return <TraktListScreen trakt={page.trakt} />;
    case "library":
      return <MediaLibraryScreen sourceId={page.sourceId} folder={page.folder} />;
  }
}

function pageTitle(page: Page) {
  switch (page.type) {
    case "search":
      return "Search";
    case "continue":
      return "Continue Watching";
    case "person":
      return page.name;
    case "trakt":
      return traktPageTitle(page.trakt);
    default:
      return "";
  }
}

function pageKey(page: Page) {
  if (page.type === "trakt") return `trakt:${JSON.stringify(page.trakt.source)}`;
  if (page.type === "library") return `library:${page.sourceId}:${page.folder ?? ""}`;
  return "id" in page ? page.id : page.type;
}

function Sidebar({ onToggle }: { onToggle: () => void }) {
  const tab = useApp((s) => s.tab);
  const go = useApp((s) => s.go);
  const recordings = useApp((s) => s.recordings);
  const [addons, setAddons] = useState<AddonInfo[]>([]);
  useEffect(() => {
    api.addons().then(setAddons).catch(() => {});
  }, [tab]);

  const recordingNow = [...recordings.values()].some((r) => r.status === "recording");
  const scheduled = [...recordings.values()].filter((r) => r.status === "scheduled").length;
  const addonsLead = useApp((s) => s.settings.addonsLead);
  // When add-ons lead, Home and the grids already show their catalogues.
  const hasCatalogAddons = !addonsLead && addons.some((a) => a.enabled && a.catalogs.length > 0);
  const failing = failingSources(useSources((s) => s.sources)).length;

  const item = (id: Tab, label: string, icon: ReactNode, extra?: ReactNode) => (
    <button className={`sidebar-item ${tab === id ? "selected" : ""}`} onClick={() => go(id)} aria-current={tab === id ? "page" : undefined}>
      {icon}
      <span>{label}</span>
      {extra}
    </button>
  );
  const ic = { size: 16, strokeWidth: 1.6 };

  return (
    <aside className="sidebar">
      <div className="sidebar-panel">
        <div className="sidebar-top" data-tauri-drag-region>
          <button className="sidebar-toggle" onClick={onToggle} aria-label="Hide sidebar">
            <PanelLeft size={17} strokeWidth={1.6} />
          </button>
        </div>
        <div className="brand" data-tauri-drag-region>
          <BrandLockup height={20} />
        </div>
        <nav className="sidebar-section">
          <div className="sidebar-heading">Watch</div>
          {item("home", "Home", <House {...ic} />)}
          {item("watchlist", "Watchlist", <Bookmark {...ic} />)}
          {item("live", "Live TV", <Radio {...ic} />)}
          {item("movies", "Movies", <Clapperboard {...ic} />)}
          {item("series", "Series", <Monitor {...ic} />)}
          {item(
            "recordings",
            "Recordings",
            <CircleDot {...ic} />,
            recordingNow ? <span className="rec-dot" aria-label="Recording now" /> : scheduled ? <span className="count">{scheduled}</span> : null,
          )}
          {hasCatalogAddons && item("discover", "Discover", <Compass {...ic} />)}
        </nav>
        <nav className="sidebar-section">
          <div className="sidebar-heading">Manage</div>
          {item(
            "sources",
            "Sources",
            <RadioTower {...ic} />,
            failing ? (
              <span className="attention" title={`${failing === 1 ? "A source" : `${failing} sources`} couldn't refresh`} aria-label={`${failing} need${failing === 1 ? "s" : ""} attention`}>
                {failing}
              </span>
            ) : null,
          )}
          {item("settings", "Settings", <SettingsIcon {...ic} />)}
        </nav>
        <div className="sidebar-spacer" data-tauri-drag-region />
        <SidebarRefreshes />
        <ProfileSwitcher />
      </div>
    </aside>
  );
}

/** Each refresh running, by source, with the step it's on. */
function SidebarRefreshes() {
  const sources = useSources((s) => s.sources);
  const progress = useSources((s) => s.progress);
  const stopping = useSources((s) => s.stopping);
  const running = (sources ?? []).filter((s) => progress.has(s.config.id));
  if (!running.length) return null;
  return (
    <div className="sidebar-refreshes" role="status">
      {running.map((s) => (
        <div key={s.config.id} className="sidebar-refresh">
          <span className="spinner small" />
          <div className="sidebar-refresh-text">
            <div className="sidebar-refresh-name">{s.config.name}</div>
            <div className="sidebar-refresh-phase">{stopping.has(s.config.id) ? "Stopping" : progress.get(s.config.id)!.phase}…</div>
          </div>
        </div>
      ))}
    </div>
  );
}

function Toolbar({ title, canGoBack, sidebarHidden }: { title: string; canGoBack: boolean; sidebarHidden: boolean }) {
  const back = useApp((s) => s.back);
  const go = useApp((s) => s.go);
  const searchText = useApp((s) => s.searchText);
  const setSearchText = useApp((s) => s.setSearchText);
  const refreshing = useSources((s) => s.progress.size > 0);
  return (
    <header className="toolbar" style={sidebarHidden ? { paddingLeft: 130 } : undefined}>
      <div className="toolbar-drag" data-tauri-drag-region />
      {canGoBack && (
        <button className="back-button glass" onClick={back} aria-label="Back">
          <ArrowLeft size={16} strokeWidth={2} />
        </button>
      )}
      <div className="toolbar-title" data-tauri-drag-region>
        {title}
      </div>
      <div className="toolbar-spacer" />
      {refreshing && (
        <button className="tool-button glass refreshing" onClick={() => go("sources")} aria-label="Refreshing sources" title="Refreshing sources">
          <span className="spinner small" />
        </button>
      )}
      <button className="tool-button glass" onClick={() => useSources.getState().requestAdd()} aria-label="Add a source" title="Add a playlist, provider login, media server or folder">
        <Plus size={16} strokeWidth={1.8} />
      </button>
      <label className="search-field glass">
        <Search size={15} strokeWidth={2} />
        <input
          id="global-search"
          value={searchText}
          placeholder="Channels, movies, series, programs"
          onChange={(e) => setSearchText(e.target.value)}
          onKeyDown={(e) => e.key === "Escape" && setSearchText("")}
          spellCheck={false}
          autoComplete="off"
        />
      </label>
    </header>
  );
}

function ProgramSheetHost() {
  const hit = useApp((s) => s.programSheet);
  const showProgram = useApp((s) => s.showProgram);
  if (!hit) return null;
  return <ProgramSheet program={hit.program} channel={hit.channel} onClose={() => showProgram(null)} />;
}

function GuideMatchSheetHost() {
  const channel = useApp((s) => s.guideMatch);
  const showGuideMatch = useApp((s) => s.showGuideMatch);
  if (!channel) return null;
  return <GuideMatchSheet key={channel.id} channel={channel} onClose={() => showGuideMatch(null)} />;
}

export function Toast() {
  const toast = useApp((s) => s.toast);
  if (!toast) return null;
  return (
    <div className={`toast glass ${toast.tone === "error" ? "error" : ""}`} role="status">
      {toast.text}
    </div>
  );
}

/** Applies the chosen appearance, following the system when asked to. */
export function useTheme(appearance: string, density: string) {
  useEffect(() => {
    const root = document.documentElement;
    root.dataset.density = density;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      root.dataset.theme = appearance === "system" ? (media.matches ? "dark" : "light") : appearance;
    };
    apply();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [appearance, density]);
}

/**
 * The window's own keys, those the menu bar doesn't have: Esc goes back, L
 * is Last Channel. The menu's shortcuts (⌘1, ⌘F, ⌘[…) arrive as commands
 * (useShellCommands).
 */
function useKeyboardShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLSelectElement;
      const { back, stack, presentation, tab, nowPlaying, guideMatch, programSheet } = useApp.getState();
      if (!typing && presentation !== "full" && e.key === "Escape" && stack.length) {
        back();
      } else if (!typing && !e.metaKey && !e.ctrlKey && !e.altKey && (e.key === "l" || e.key === "L") && !guideMatch && !programSheet) {
        // Last Channel, while a channel plays or in Live TV (not over a film).
        const watchingChannel = !!nowPlaying?.channel && nowPlaying.kind === "channel";
        if (watchingChannel || (tab === "live" && presentation !== "full")) {
          e.preventDefault();
          playLastChannel();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

/** Going somewhere from the menu: video filling the window steps aside, as in the Swift app. */
function goTo(tab: Tab) {
  const { presentation, nowPlaying, setPresentation, setSearchText, go } = useApp.getState();
  if (presentation === "full" && nowPlaying) {
    leavePlayerFullscreen();
    setPresentation(nowPlaying.channel && tab === "live" ? "docked" : "mini");
  }
  setSearchText("");
  go(tab);
}

/** The menu bar's commands that belong to the window rather than the player, and what it says can act now. */
function useShellCommands(toggleSidebar: () => void, sidebarHidden: boolean) {
  useEffect(
    () =>
      handleCommands({
        "app.settings": () => goTo("settings"),
        "go.settings": () => goTo("settings"),
        "file.addSource": () => {
          goTo("sources");
          useSources.getState().requestAdd();
        },
        "file.refreshAll": () => useSources.getState().refreshAll(),
        "view.sidebar": toggleSidebar,
        "go.home": () => goTo("home"),
        "go.watchlist": () => goTo("watchlist"),
        "go.live": () => goTo("live"),
        "go.movies": () => goTo("movies"),
        "go.series": () => goTo("series"),
        "go.recordings": () => goTo("recordings"),
        "go.sources": () => goTo("sources"),
        "go.search": () => {
          const { presentation, setPresentation } = useApp.getState();
          if (presentation === "full") setPresentation("mini");
          document.getElementById("global-search")?.focus();
        },
        // Back from whatever fills the window: Multiview, the player, then a page.
        "go.back": () => {
          const multiview = useMultiview.getState();
          const { presentation, nowPlaying, stack, back } = useApp.getState();
          if (multiview.open) {
            if (multiview.expanded) toggleExpand(multiview.expanded);
            else closeMultiview();
          } else if (presentation === "full" && nowPlaying && canPerform("player.back")) perform("player.back");
          else if (stack.length) back();
        },
        "playback.multiview": () => {
          // A sheet over the window has the keys.
          if (!document.querySelector(".sheet-backdrop")) toggleMultiview();
        },
        "playback.inset": () => {
          if (document.querySelector(".sheet-backdrop")) return;
          if (useMultiview.getState().inset) toggleInset();
          else insetCommand();
        },
        "playback.swapInset": swapInset,
      }),
    [toggleSidebar],
  );

  const hidden = useRef(sidebarHidden);
  hidden.current = sidebarHidden;
  const send = useRef<() => void>(undefined);
  useEffect(() => send.current?.(), [sidebarHidden]);
  useEffect(() => {
    const compute = (): MenuState => {
      const { stack, presentation, nowPlaying } = useApp.getState();
      const multiview = useMultiview.getState();
      const report = usePlaybackReport.getState();
      const playing = report.playing && !!nowPlaying;
      // In Multiview, Play/Pause, the skips and Go Live are the heard tile's.
      const tile = multiview.open ? heardTile() : null;
      const tileState = tile
        ? { paused: tile.paused, seekable: !!tile.stream?.timeshift || !tileChannel(tile), behindLive: !!tile.stream?.timeshift && tile.behind >= LIVE_EDGE }
        : {};
      return {
        app: {
          hasSources: !!useSources.getState().sources?.length,
          canGoBack: stack.length > 0 || (presentation === "full" && !!nowPlaying) || multiview.open,
          sidebarHidden: hidden.current,
          multiview: multiview.open,
          canAddInset: !multiview.open && !!nowPlaying && !useApp.getState().poppedOut,
          canInset: multiview.open && canInset(multiview.tiles.length),
          inset: multiview.open && multiview.inset,
        },
        playback: {
          ...report,
          ...tileState,
          playing,
          channel: playing && report.channel,
          lastChannel: !!lastChannel(),
          multiview: multiview.open,
          allPaused: multiview.open && allPaused(multiview.tiles),
          poppedOut: false,
        },
      };
    };
    return followMenuState(compute, (changed) => {
      send.current = changed;
      const offs = [useApp.subscribe(changed), useSources.subscribe(changed), useMultiview.subscribe(changed), usePlaybackReport.subscribe(changed)];
      return () => offs.forEach((off) => off());
    });
  }, []);
}

