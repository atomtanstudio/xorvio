// Multiview's screen: the tiles over the window, where the player shows, with
// their controls, and the channel picker for adding and swapping. Where tiles
// go and who's heard are multiviewRules; the tiles' streams are multiviewStore.

import { ArrowLeft, ArrowLeftRight, ChevronLeft, ChevronRight, CircleDot, CircleStop, Ellipsis, Grid2x2Plus, Maximize, Maximize2, Minimize, Minimize2, Pause, Play, Plus, RotateCcw, RotateCw, Search, Star, Tv, Volume2, VolumeX, X } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ChannelLogo } from "../components/Artwork";
import { openMenu, type MenuEntry } from "../components/Menu";
import { Sheet } from "../components/Sheet";
import { cancelOrStopRecording, channelItem, recordChannelNow } from "../lib/actions";
import { shortcutLabel } from "../lib/commands";
import { api } from "../lib/api";
import { nowSeconds, playerClock } from "../lib/format";
import { isMac } from "../lib/platform";
import { useApp } from "../lib/store";
import type { Channel, Program, Recording } from "../lib/types";
import { sameChannel, versionIds } from "../lib/versions";
import { attachStream, describeMediaError, type EngineHandle } from "./engine";
import { isWindowFullscreen, togglePlayerFullscreen } from "./fullscreen";
import { useMediaSession } from "./mediaSession";
import {
  choose,
  closeMultiview,
  failTile,
  goLiveTile,
  heardTile,
  listen,
  listenNext,
  moveTile,
  picked,
  registerTileVideo,
  removeTile,
  reportBehind,
  retryTile,
  setTilePaused,
  skipTile,
  tileVideo,
  toggleAllPaused,
  toggleExpand,
  toggleHeardPaused,
  useMultiview,
  watchInPlayer,
  type Picker,
  type Tile,
} from "./multiviewStore";
import { allPaused, fitGrid, layoutFor, MAX_TILES, resumeAt, secondsBehind, tileForKey, type TilePlace } from "./multiviewRules";
import { bufferRange, LIVE_EDGE } from "./playerRules";

/** Between tiles, in points. */
const GAP = 6;

/** Pause All's key, as tooltips show it. */
const PAUSE_ALL_KEY = isMac ? "⇧Space" : "Shift+Space";

export function MultiviewHost() {
  const open = useMultiview((s) => s.open);
  const picker = useMultiview((s) => s.picker);

  // ⇧⌘M is Playback › Multiview in the menu bar (src/lib/commands.ts), as in the Swift app.

  return (
    <>
      {open && <Multiview />}
      {picker && <ChannelPicker picker={picker} />}
    </>
  );
}

function Multiview() {
  const tiles = useMultiview((s) => s.tiles);
  const audio = useMultiview((s) => s.audio);
  const expanded = useMultiview((s) => s.expanded);
  const recordings = useApp((s) => s.recordings);
  const stage = useRef<HTMLDivElement>(null);
  const space = useSize(stage);
  const [active, poke] = useActivity();
  const [fullscreen, setFullscreen] = useState(false);
  const ids = tiles.map((t) => t.id);
  const layout = layoutFor(ids, expanded);
  const grid = fitGrid(space, layout, GAP);
  const places = new Map(layout.places.map((p) => [p.id, p]));
  const programmes = useProgrammesOnNow(tiles.map((t) => t.channel.id));

  useEffect(() => {
    isWindowFullscreen().then(setFullscreen);
  }, []);
  const toggleFullscreen = useCallback(async () => setFullscreen(await togglePlayerFullscreen()), []);

  // Keys, ahead of the rest of the window's: 1–4 to hear a tile, Esc to go
  // back. Play/pause and skipping act on the tile heard, as the player's
  // keys do on its channel; ⇧Space pauses or plays them all.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      // A sheet or menu over Multiview has the keys.
      if (document.querySelector(".sheet-backdrop, .menu")) return;
      const { tiles, expanded } = useMultiview.getState();
      const plain = !e.metaKey && !e.ctrlKey && !e.altKey;
      const tile = plain ? tileForKey(tiles.map((t) => t.id), e.key) : null;
      const heard = heardTile();
      if (tile) listen(tile);
      // ⌘[ is Go › Back in the menu bar, which does the same.
      else if (e.key === "Escape") {
        if (expanded) toggleExpand(expanded);
        else closeMultiview();
      } else if (plain && e.shiftKey && e.key === " ") toggleAllPaused();
      else if (plain && !e.shiftKey && (e.key === " " || e.key === "k")) toggleHeardPaused();
      else if (plain && heard && (e.key === "ArrowLeft" || e.key === "ArrowRight")) skipTile(heard.id, (e.key === "ArrowLeft" ? -1 : 1) * (e.shiftKey ? 60 : 10));
      else if (plain && e.key === "f") toggleFullscreen();
      // Last Channel is the player's; here it would end Multiview, so it does nothing.
      else if (!(plain && e.key.toLowerCase() === "l")) return;
      e.preventDefault();
      e.stopPropagation();
      poke();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [poke, toggleFullscreen]);

  // Now Playing and the media keys follow the tile heard; next and previous move the sound along.
  const heard = tiles.find((t) => t.id === audio) ?? null;
  const heardChannel = heard?.channel;
  const item = useMemo(() => (heardChannel ? channelItem(heardChannel) : null), [heardChannel]);
  const heardVideo = useMemo(
    () => ({
      get current() {
        return tileVideo(audio);
      },
    }),
    [audio],
  );
  const several = tiles.length > 1;
  useMediaSession({
    item,
    video: heardVideo,
    isLive: true,
    paused: !!heard?.paused,
    position: 0,
    duration: 0,
    speed: 1,
    onSeek: () => {},
    onNext: several ? () => listenNext(1) : null,
    onPrevious: several ? () => listenNext(-1) : null,
    onPlay: () => audio && setTilePaused(audio, false),
    onPause: () => audio && setTilePaused(audio, true),
  });

  const everyPaused = allPaused(tiles);
  const shown = active || everyPaused;
  const hint = expanded ? `Esc shows all ${tiles.length} again` : several ? `Click a tile or press 1–${tiles.length} to hear it` : "Add a channel to watch two at once";

  return (
    <div className={`multiview ${shown ? "active" : ""}`} onMouseMove={poke}>
      <div className="mv-top" data-tauri-drag-region>
        <button className="player-round glass" onClick={closeMultiview} aria-label="Back to the player" title="Back to the player (Esc)">
          <ArrowLeft size={18} />
        </button>
        <div className="mv-titles" data-tauri-drag-region>
          <div className="player-title">Multiview</div>
          <div className="player-subtitle">{hint}</div>
        </div>
        <div style={{ flex: 1 }} data-tauri-drag-region />
        <button className="btn btn-glass btn-small" disabled={tiles.length >= MAX_TILES} onClick={() => choose({ purpose: "add" })} title={tiles.length >= MAX_TILES ? "Multiview shows up to four channels" : undefined}>
          <Plus size={15} /> Add Channel
        </button>
        <button className="player-round glass" onClick={toggleAllPaused} aria-label={everyPaused ? "Play all" : "Pause all"} title={`${everyPaused ? "Play all" : "Pause all"} (${PAUSE_ALL_KEY})`}>
          {everyPaused ? <Play size={16} fill="currentColor" strokeWidth={0} /> : <Pause size={16} fill="currentColor" strokeWidth={0} />}
        </button>
        <button className="player-round glass" onClick={toggleFullscreen} aria-label="Full screen">
          {fullscreen ? <Minimize size={16} /> : <Maximize size={16} />}
        </button>
      </div>
      <div className="mv-stage" ref={stage}>
        <div className="mv-grid" style={{ width: grid.width, height: grid.height, gap: GAP, gridTemplateColumns: `repeat(${layout.columns}, 1fr)`, gridTemplateRows: `repeat(${layout.rows}, 1fr)` }}>
          {/* In the order they were added, so a tile moving never moves its video element. */}
          {[...tiles]
            .sort((a, b) => a.seq - b.seq)
            .map((tile) => (
              <TileView
                key={tile.id}
                tile={tile}
                index={ids.indexOf(tile.id)}
                count={tiles.length}
                place={places.get(tile.id)}
                audible={tile.id === audio}
                expanded={tile.id === expanded}
                programme={programmes ? (programmes.get(tile.channel.id) ?? null) : undefined}
                recording={recordingOn(recordings, tile.channel)}
              />
            ))}
          {layout.invite && (
            <button className="mv-invite" style={{ gridColumn: layout.invite.column, gridRow: layout.invite.row }} onClick={() => choose({ purpose: "add" })}>
              <Grid2x2Plus size={30} strokeWidth={1.4} />
              <strong>Add a channel</strong>
              <span>Watch up to four at once, and choose which one you hear.</span>
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

/** The recording running on a channel, if there is one. */
function recordingOn(recordings: Map<string, Recording>, channel: Channel) {
  const ids = versionIds(channel);
  return [...recordings.values()].find((r) => r.status === "recording" && ids.includes(r.channelId)) ?? null;
}

interface TileProps {
  tile: Tile;
  /** Its place in the layout: 0 is the first (and the large one of three). */
  index: number;
  count: number;
  /** Where it sits; none while another tile fills the view. */
  place: TilePlace | undefined;
  audible: boolean;
  expanded: boolean;
  /** What's on now: null when the guide has nothing, undefined until it's been asked. */
  programme: Program | null | undefined;
  recording: Recording | null;
}

function TileView({ tile, index, count, place, audible, expanded, programme, recording }: TileProps) {
  const video = useRef<HTMLVideoElement>(null);
  const [buffering, setBuffering] = useState(true);
  const audibleNow = useRef(audible);
  audibleNow.current = audible;
  const { id, stream, attempt, channel, paused, behind } = tile;
  const pausedNow = useRef(paused);
  pausedNow.current = paused;
  const holdNow = useRef(tile.holdAtStart);
  holdNow.current = tile.holdAtStart;
  const buffered = !!stream?.timeshift;

  useLayoutEffect(() => {
    registerTileVideo(id, video.current);
    return () => registerTileVideo(id, null);
  }, [id]);

  // Attach each stream, as the player does; one that fails is this tile's alone.
  useEffect(() => {
    const el = video.current;
    if (!el || !stream) return;
    let cancelled = false;
    let engine: EngineHandle | null = null;
    let started = false;
    setBuffering(true);
    el.muted = !audibleNow.current;
    attachStream(el, stream, (message) => {
      api.log("error", `multiview engine: ${message}`).catch(() => {});
      if (!cancelled) failTile(id, attempt, message);
    }).then((handle) => {
      if (cancelled) return handle.destroy();
      engine = handle;
      if (!pausedNow.current) el.play().catch((e: Error) => api.log("warn", `multiview play() refused: ${e.name} ${e.message}`).catch(() => {}));
    });
    const onPlaying = () => (started = true);
    el.addEventListener("playing", onPlaying);
    // Paused into a buffer: wait at its start, which is where the pause was
    // (WebKit knows the buffer's range a little after its metadata).
    const rangeEvents = ["loadedmetadata", "durationchange", "progress", "canplay"];
    const toStart = () => {
      const range = bufferRange(el);
      if (!range) return;
      el.currentTime = range.start;
      rangeEvents.forEach((name) => el.removeEventListener(name, toStart));
    };
    if (stream.timeshift && holdNow.current) rangeEvents.forEach((name) => el.addEventListener(name, toStart));
    // A paused tile needn't play, only load.
    const watchdog = setTimeout(() => {
      if (cancelled || started || el.readyState >= (pausedNow.current ? el.HAVE_METADATA : el.HAVE_FUTURE_DATA)) return;
      failTile(id, attempt, "This channel didn't start. It may be offline.");
    }, 45_000);
    return () => {
      cancelled = true;
      clearTimeout(watchdog);
      el.removeEventListener("playing", onPlaying);
      rangeEvents.forEach((name) => el.removeEventListener(name, toStart));
      engine?.destroy();
    };
  }, [id, stream, attempt]);

  useEffect(() => {
    if (video.current) video.current.muted = !audible;
  }, [audible]);

  useEffect(() => {
    const el = video.current;
    if (!el || !el.currentSrc) return;
    if (paused) el.pause();
    else if (el.paused) {
      // Paused for longer than its buffer keeps: carry on from the oldest part still kept.
      const at = stream?.timeshift ? resumeAt(bufferRange(el), el.currentTime) : el.currentTime;
      if (at !== el.currentTime) el.currentTime = at;
      el.play().catch(() => {});
    }
    // The stream is read when it's played or paused, not when it changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paused]);

  // How far behind live it plays from its buffer: growing while it's paused.
  useEffect(() => {
    if (!stream?.timeshift) return;
    const measure = () => {
      const el = video.current;
      if (el) reportBehind(id, secondsBehind(bufferRange(el), el.currentTime));
    };
    measure();
    const timer = setInterval(measure, 1000);
    return () => clearInterval(timer);
  }, [id, stream]);

  const menu = (e: React.MouseEvent, below = false) => openMenu(e, tileMenu(tile, { index, count, audible, expanded, programme, recording }), below);
  const behindLive = buffered && behind >= LIVE_EDGE;
  const style: React.CSSProperties = place ? { gridColumn: `${place.column} / span ${place.span}`, gridRow: `${place.row} / span ${place.span}` } : { display: "none" };
  const stop = (e: React.SyntheticEvent) => e.stopPropagation();

  return (
    <div className={`mv-tile ${audible ? "audible" : ""}`} style={style} onClick={() => listen(id)} onDoubleClick={() => toggleExpand(id)} onContextMenu={menu}>
      <video
        ref={video}
        className="mv-video"
        playsInline
        onWaiting={() => setBuffering(true)}
        onPlaying={() => setBuffering(false)}
        onCanPlay={() => setBuffering(false)}
        onError={(e) => {
          const el = e.currentTarget;
          api.log("error", `multiview video error ${el.error?.code} ${el.error?.message ?? ""} mode=${stream?.mode}`).catch(() => {});
          failTile(id, attempt, describeMediaError(el.error));
        }}
      />
      {(buffering || !stream) && !tile.error && (
        <div className="player-center">
          <div className="spinner" />
        </div>
      )}
      {tile.error && (
        <div className="player-center player-error mv-error">
          <div>
            <strong>Can't play {channel.name}</strong>
            <p>{tile.error}</p>
            <div className="mv-error-actions" onClick={stop} onDoubleClick={stop}>
              <button className="btn btn-glass btn-small" onClick={() => retryTile(id)}>
                Retry
              </button>
              <button className="btn btn-glass btn-small" onClick={() => removeTile(id)}>
                Remove
              </button>
            </div>
          </div>
        </div>
      )}
      <div className="mv-badges">
        {audible && (
          <span className="mv-audio-badge" title="You're hearing this one">
            <Volume2 size={13} />
          </span>
        )}
        {(paused || behindLive) && (
          <span className="mv-behind-badge" title={behindLive ? `${playerClock(behind)} behind live` : "Paused"}>
            {paused && <Pause size={10} fill="currentColor" strokeWidth={0} />}
            {behindLive ? `−${playerClock(behind)}` : "Paused"}
          </span>
        )}
        {recording && (
          <span className="rec-badge active">
            <span className="dot" />
            REC
          </span>
        )}
      </div>
      <div className="mv-tile-chrome">
        <div className="mv-tile-info">
          <span className="mv-key" title={`Press ${index + 1} to listen to this one`}>
            {index + 1}
          </span>
          <ChannelLogo src={channel.logoUrl} name={channel.name} size={30} />
          <div className="mv-tile-text">
            <div className="mv-tile-name">{channel.name}</div>
            <div className="mv-tile-programme">{programme === undefined ? " " : (programme?.title ?? "No guide listings")}</div>
          </div>
        </div>
        <div className="mv-tile-actions" onClick={stop} onDoubleClick={stop}>
          {(paused || buffered) && (
            <button className="live-pill behind" onClick={() => goLiveTile(id)} title={audible ? `Back to live (${shortcutLabel("playback.goLive")})` : "Back to live"}>
              Go Live
            </button>
          )}
          <button className="player-round small glass" onClick={() => setTilePaused(id, !paused)} aria-label={paused ? "Play" : "Pause"} title={paused ? "Play" : "Pause, to carry on from here"}>
            {paused ? <Play size={12} fill="currentColor" strokeWidth={0} /> : <Pause size={12} fill="currentColor" strokeWidth={0} />}
          </button>
          <button className={`player-round small glass ${audible ? "on" : ""}`} onClick={() => listen(id)} aria-label={audible ? "You're hearing this one" : "Listen to this one"} title={audible ? "You're hearing this one" : `Listen to this one (${index + 1})`}>
            {audible ? <Volume2 size={13} /> : <VolumeX size={13} />}
          </button>
          <button className="player-round small glass" onClick={() => toggleExpand(id)} aria-label={expanded ? "Show all" : "Expand"} title={expanded ? "Show all (Esc)" : "Fill the view"}>
            {expanded ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
          </button>
          <button className="player-round small glass" onClick={() => choose({ purpose: "swap", tileId: id })} aria-label="Swap channel" title="Swap channel">
            <ArrowLeftRight size={13} />
          </button>
          <button className="player-round small glass" onClick={(e) => menu(e, true)} aria-label="More">
            <Ellipsis size={13} />
          </button>
          <button className="player-round small glass" onClick={() => removeTile(id)} aria-label="Remove" title="Remove from Multiview">
            <X size={13} />
          </button>
        </div>
      </div>
    </div>
  );
}

const icon = { size: 15, strokeWidth: 1.8 };

/** A tile's "…" and right-click menu. */
function tileMenu(tile: Tile, at: { index: number; count: number; audible: boolean; expanded: boolean; programme: Program | null | undefined; recording: Recording | null }): MenuEntry[] {
  const { id, channel, paused } = tile;
  const { index, count, audible, expanded, programme, recording } = at;
  const buffered = !!tile.stream?.timeshift;
  const items: MenuEntry[] = [{ heading: channel.name }];
  if (!audible) items.push({ label: "Listen to This One", icon: <Volume2 {...icon} />, shortcut: String(index + 1), onSelect: () => listen(id) });
  // The keys act on the tile heard.
  const key = (shortcut: string) => (audible ? shortcut : undefined);
  items.push({ label: paused ? "Play" : "Pause", icon: paused ? <Play {...icon} /> : <Pause {...icon} />, shortcut: key("Space"), onSelect: () => setTilePaused(id, !paused) });
  if (buffered)
    items.push(
      { label: "Skip Back 10 Seconds", icon: <RotateCcw {...icon} />, shortcut: key("←"), onSelect: () => skipTile(id, -10) },
      { label: "Skip Forward 10 Seconds", icon: <RotateCw {...icon} />, shortcut: key("→"), onSelect: () => skipTile(id, 10) },
    );
  if (buffered || paused) items.push({ label: "Go Live", icon: <Tv {...icon} />, onSelect: () => goLiveTile(id) });
  items.push({ separator: true });
  items.push(
    { label: expanded ? "Show All" : "Fill the View", icon: expanded ? <Minimize2 {...icon} /> : <Maximize2 {...icon} />, onSelect: () => toggleExpand(id) },
    { label: "Swap Channel…", icon: <ArrowLeftRight {...icon} />, onSelect: () => choose({ purpose: "swap", tileId: id }) },
    recording
      ? {
          label: "Stop Recording",
          icon: <CircleStop {...icon} />,
          destructive: true,
          // Found by the recording's own programme and start, whatever the guide says now.
          onSelect: () => cancelOrStopRecording({ id: recording.programId ?? "", start: recording.start } as Program, { ...channel, id: recording.channelId }),
        }
      : { label: "Record", icon: <CircleDot {...icon} />, onSelect: () => recordChannelNow(channel, programme ?? null) },
  );
  if (!expanded && count > 1)
    items.push(
      { separator: true },
      { label: "Move Earlier", icon: <ChevronLeft {...icon} />, disabled: index <= 0, onSelect: () => moveTile(id, -1) },
      { label: "Move Later", icon: <ChevronRight {...icon} />, disabled: index >= count - 1, onSelect: () => moveTile(id, 1) },
    );
  items.push(
    { separator: true },
    { label: "Watch in Player", icon: <Tv {...icon} />, onSelect: () => watchInPlayer(id) },
    { label: "Remove from Multiview", icon: <X {...icon} />, destructive: true, onSelect: () => removeTile(id) },
  );
  return items;
}

// MARK: Picker

/** Rows shown at once; typing narrows a long list down. */
const PICKER_ROWS = 150;

/** For matching names whatever their case and accents. */
const fold = (text: string) => text.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();

/** Choosing a channel for another tile, or for a tile's place: favourites first, then every channel, searchable. */
function ChannelPicker({ picker }: { picker: Picker }) {
  const tiles = useMultiview((s) => s.tiles);
  const open = useMultiview((s) => s.open);
  const nowPlaying = useApp((s) => s.nowPlaying);
  const [favorites, setFavorites] = useState<Channel[] | null>(null);
  const [all, setAll] = useState<Channel[] | null>(null);
  const [text, setText] = useState("");

  useEffect(() => {
    api.channels({ favoritesOnly: true }).then(setFavorites).catch(() => setFavorites([]));
    api.channels({}).then(setAll).catch(() => setAll([]));
  }, []);

  const swapping = picker.purpose === "swap" ? tiles.find((t) => t.id === picker.tileId) : undefined;
  const playing = !open && nowPlaying?.kind === "channel" ? nowPlaying.channel : undefined;
  const query = fold(text.trim());

  // Names folded once, so typing stays quick in a long list.
  const list = useMemo(() => {
    if (!favorites || !all) return null;
    const favoriteIds = new Set(favorites.flatMap(versionIds));
    const rest = all.filter((c) => !versionIds(c).some((v) => favoriteIds.has(v)));
    return [...favorites.map((channel) => ({ channel, favorite: true })), ...rest.map((channel) => ({ channel, favorite: false }))].map((row) => ({ ...row, name: fold(row.channel.name) }));
  }, [favorites, all]);
  const rows = useMemo(() => (list && query ? list.filter((row) => row.name.includes(query) || String(row.channel.number ?? "") === query) : list), [list, query]);

  /** Why a channel can't be chosen: it's on already. */
  const note = (channel: Channel) => {
    if (swapping && sameChannel(swapping.channel, channel)) return "In this tile";
    if (tiles.some((t) => sameChannel(t.channel, channel))) return "In Multiview";
    if (playing && sameChannel(playing, channel)) return "Playing now";
    return null;
  };

  const close = () => choose(null);
  const shown = rows?.slice(0, PICKER_ROWS) ?? [];
  const title = swapping ? "Swap Channel" : open ? "Add to Multiview" : "Multiview";
  const subtitle = swapping
    ? `Choose a channel to watch instead of ${swapping.channel.name}.`
    : open
      ? `Choose a channel to watch beside the others. Multiview shows up to ${MAX_TILES}.`
      : playing
        ? `Choose a channel to watch beside ${playing.name}. You can add up to ${MAX_TILES}.`
        : `Choose a channel to start with. You can add up to ${MAX_TILES}.`;

  return (
    <Sheet title={title} subtitle={subtitle} onClose={close} width={520} footer={<button className="btn-plain" onClick={close}>Cancel</button>}>
      {/* The keys stay here, not with the player or Multiview underneath. */}
      <div
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") close();
        }}
      >
        <label className="mv-picker-search">
          <Search size={14} />
          <input
            className="text-input"
            autoFocus
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              const first = e.key === "Enter" ? shown.find(({ channel }) => !note(channel)) : undefined;
              if (first) picked(first.channel);
            }}
            placeholder="Search channels"
            spellCheck={false}
            autoComplete="off"
          />
        </label>
        <div className="mv-picker-list" role="listbox" aria-label="Channels">
          {rows == null ? (
            <div className="spinner" style={{ margin: "14px auto" }} />
          ) : rows.length === 0 ? (
            <p className="muted mv-picker-empty">{query ? "No channel has that name." : "There are no live channels in your sources yet."}</p>
          ) : (
            shown.map(({ channel, favorite }, i) => {
              const why = note(channel);
              const heading = !query && (i === 0 || shown[i - 1].favorite !== favorite) ? (favorite ? "Favorites" : "All Channels") : null;
              return (
                <div key={channel.id} className="mv-picker-item">
                  {heading && <div className="mv-picker-heading">{heading}</div>}
                  <button className="copy-row mv-picker-row" disabled={!!why} onClick={() => picked(channel)} role="option" aria-selected={false}>
                    <ChannelLogo src={channel.logoUrl} name={channel.name} size={30} />
                    <span className="copy-text">
                      <span className="copy-name">
                        {channel.number != null && <span className="mv-picker-number">{channel.number}</span>}
                        {channel.name}
                      </span>
                      {why && <span className="copy-meta">{why}</span>}
                    </span>
                    {favorite && <Star size={13} fill="#f5c84c" color="#f5c84c" className="mv-picker-star" />}
                  </button>
                </div>
              );
            })
          )}
          {rows && rows.length > PICKER_ROWS && <p className="muted mv-picker-more">Showing {PICKER_ROWS} of {rows.length}. Type to find the rest.</p>}
        </div>
      </div>
    </Sheet>
  );
}

// MARK: Helpers

/** An element's size, kept up to date. */
function useSize(ref: React.RefObject<HTMLElement | null>) {
  const [size, setSize] = useState({ width: 0, height: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setSize((old) => (old.width === el.clientWidth && old.height === el.clientHeight ? old : { width: el.clientWidth, height: el.clientHeight }));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return size;
}

/** Whether the pointer or keys moved lately: the controls show, then fade, with the cursor, as the player's do. */
function useActivity() {
  const [active, setActive] = useState(true);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const poke = useCallback(() => {
    setActive(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setActive(false), 3200);
  }, []);
  useEffect(() => {
    poke();
    return () => clearTimeout(timer.current);
  }, [poke]);
  return [active, poke] as const;
}

/** What's on now on each channel, by channel id, checked each minute; null until first asked. */
function useProgrammesOnNow(channelIds: string[]) {
  const key = channelIds.join("\n");
  const [programmes, setProgrammes] = useState<Map<string, Program> | null>(null);
  useEffect(() => {
    const ids = key ? key.split("\n") : [];
    if (!ids.length) return;
    let cancelled = false;
    const load = () =>
      api
        .nowNext(ids)
        .then((rows) => {
          if (cancelled) return;
          const t = nowSeconds();
          const found = new Map<string, Program>();
          for (const row of rows) {
            const on = row.programs.find((p) => p.start <= t && t < p.end);
            if (on) found.set(row.channel.id, on);
          }
          setProgrammes(found);
        })
        .catch(() => {});
    load();
    const timer = setInterval(load, 60_000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [key]);
  return programmes;
}
