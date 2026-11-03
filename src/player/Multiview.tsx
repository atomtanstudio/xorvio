// Multiview's screen: the tiles over the window, where the player shows, with
// their controls, and the picker for adding and swapping channels, films and
// episodes. Where tiles go and who's heard are multiviewRules; the tiles'
// streams are multiviewStore. With the inset on, the second tile sits small
// in a corner over the first, and can be dragged to another corner.

import { ArrowLeft, ArrowLeftRight, ChevronLeft, ChevronRight, CircleDot, CircleStop, Columns2, Ellipsis, Eye, EyeOff, Folder, Grid2x2Plus, Maximize, Maximize2, Minimize, Minimize2, PanelsRightBottom, Pause, Play, Plus, Replace, RotateCcw, RotateCw, Search, Star, Tv, Volume2, VolumeX, X } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ChannelLogo } from "../components/Artwork";
import { openMenu, type MenuEntry } from "../components/Menu";
import { Sheet } from "../components/Sheet";
import { cancelOrStopRecording, channelItem, continueItem, episodeItem, movieItem, recordChannelNow } from "../lib/actions";
import { shortcutLabel } from "../lib/commands";
import { api } from "../lib/api";
import { episodeCode, nowSeconds, playerClock, remaining } from "../lib/format";
import { isMac } from "../lib/platform";
import { useApp, type NowPlaying } from "../lib/store";
import { useSources } from "../lib/sources";
import type { Channel, ContinueItem, Episode, FolderListing, Movie, Program, Recording, Series } from "../lib/types";
import { versionIds } from "../lib/versions";
import { attachStream, describeMediaError, type EngineHandle } from "./engine";
import { isWindowFullscreen, togglePlayerFullscreen } from "./fullscreen";
import { useMediaSession } from "./mediaSession";
import {
  choose,
  closeMultiview,
  endTile,
  failTile,
  goLiveTile,
  heardTile,
  insetCommand,
  isLiveItem,
  listen,
  listenNext,
  moveTile,
  picked,
  registerTileVideo,
  removeTile,
  reportBehind,
  reportPosition,
  retryTile,
  sameItem,
  seekTile,
  setTilePaused,
  skipTile,
  swapInset,
  tileChannel,
  tileVideo,
  toggleAllPaused,
  toggleExpand,
  toggleHeardPaused,
  toggleInset,
  toggleInsetHidden,
  useMultiview,
  watchInPlayer,
  type Picker,
  type Tile,
} from "./multiviewStore";
import { allPaused, canInset, fitGrid, INSET_KEY, layoutFor, MAX_TILES, resumeAt, secondsBehind, tileForKey, type TilePlace } from "./multiviewRules";
import { bufferRange, LIVE_EDGE, type Corner } from "./playerRules";
import { useMiniPlayerCorner } from "./SmallPlayers";

/** Between tiles, in points. */
const GAP = 6;

/** Pause All's key, as tooltips show it. */
const PAUSE_ALL_KEY = isMac ? "⇧Space" : "Shift+Space";

/** Where the inset's corner is remembered. */
const INSET_CORNER_KEY = "multiview.insetCorner";

export function MultiviewHost() {
  const open = useMultiview((s) => s.open);
  const picker = useMultiview((s) => s.picker);

  // ⇧⌘M is Playback › Multiview in the menu bar (src/lib/commands.ts), as in the Swift app.

  return (
    <>
      {open && <Multiview />}
      {picker && <ItemPicker picker={picker} />}
    </>
  );
}

function Multiview() {
  const tiles = useMultiview((s) => s.tiles);
  const audio = useMultiview((s) => s.audio);
  const expanded = useMultiview((s) => s.expanded);
  const inset = useMultiview((s) => s.inset);
  const insetHidden = useMultiview((s) => s.insetHidden);
  const recordings = useApp((s) => s.recordings);
  const stage = useRef<HTMLDivElement>(null);
  const space = useSize(stage);
  const [active, poke] = useActivity();
  const [fullscreen, setFullscreen] = useState(false);
  const ids = tiles.map((t) => t.id);
  const layout = layoutFor(ids, expanded, inset);
  const grid = fitGrid(space, layout, GAP);
  const places = new Map(layout.places.map((p) => [p.id, p]));
  const programmes = useProgrammesOnNow(tiles.flatMap((t) => tileChannel(t)?.id ?? []));
  // The inset's element, whichever tile it is, for dragging it to another corner.
  const insetElement = useRef<HTMLDivElement>(null);
  const corner = useMiniPlayerCorner(insetElement, !!layout.inset && !insetHidden, INSET_CORNER_KEY);

  useEffect(() => {
    isWindowFullscreen().then(setFullscreen);
  }, []);
  const toggleFullscreen = useCallback(async () => setFullscreen(await togglePlayerFullscreen()), []);

  // Keys, ahead of the rest of the window's: 1–4 to hear a tile, Esc to go
  // back. Play/pause and skipping act on the tile heard, as the player's
  // keys do on what it plays; ⇧Space pauses or plays them all, and X swaps
  // the inset and the big picture (or, with two side by side, makes one the inset).
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
      else if (plain && e.key.toLowerCase() === INSET_KEY) insetCommand();
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
  const heardItem = heard?.item ?? null;
  const item = useMemo(() => (heardItem && isLiveItem(heardItem) && heardItem.channel ? channelItem(heardItem.channel) : heardItem), [heardItem]);
  const heardVideo = useMemo(
    () => ({
      get current() {
        return tileVideo(audio);
      },
    }),
    [audio],
  );
  const several = tiles.length > 1;
  const heardLive = !heard || !!tileChannel(heard);
  useMediaSession({
    item,
    video: heardVideo,
    isLive: heardLive,
    paused: !!heard?.paused,
    position: heardLive ? 0 : (heard?.position ?? 0),
    duration: heardLive ? 0 : (heard?.duration ?? 0),
    speed: 1,
    onSeek: (to) => heard && !heardLive && seekTile(heard.id, to),
    onNext: several ? () => listenNext(1) : null,
    onPrevious: several ? () => listenNext(-1) : null,
    onPlay: () => audio && setTilePaused(audio, false),
    onPause: () => audio && setTilePaused(audio, true),
  });

  const everyPaused = allPaused(tiles);
  const shown = active || everyPaused;
  const insetTitle = layout.inset ? (tiles.find((t) => t.id === layout.inset)?.item.title ?? "") : "";
  const hint = layout.inset
    ? insetHidden
      ? `${insetTitle} is hidden. Press X to swap it back in`
      : "Press X or double-click the small one to swap them"
    : expanded
      ? `Esc shows all ${tiles.length} again`
      : several
        ? `Click a tile or press 1–${tiles.length} to hear it`
        : "Add a channel, film or episode to watch two at once";
  const insetCorner: Corner = corner.corner;

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
        <button className="btn btn-glass btn-small" disabled={tiles.length >= MAX_TILES} onClick={() => choose({ purpose: "add" })} title={tiles.length >= MAX_TILES ? "Multiview shows up to four at once" : "Add a channel, film or episode"}>
          <Plus size={15} /> Add
        </button>
        {canInset(tiles.length) && (
          <button className="player-round glass" onClick={toggleInset} aria-label={inset ? "Side by side" : "Inset"} title={inset ? "Side by side" : `One small in a corner of the other (${INSET_KEY.toUpperCase()})`}>
            {inset ? <Columns2 size={16} /> : <PanelsRightBottom size={16} />}
          </button>
        )}
        <button className="player-round glass" onClick={toggleAllPaused} aria-label={everyPaused ? "Play all" : "Pause all"} title={`${everyPaused ? "Play all" : "Pause all"} (${PAUSE_ALL_KEY})`}>
          {everyPaused ? <Play size={16} fill="currentColor" strokeWidth={0} /> : <Pause size={16} fill="currentColor" strokeWidth={0} />}
        </button>
        <button className="player-round glass" onClick={toggleFullscreen} aria-label="Full screen">
          {fullscreen ? <Minimize size={16} /> : <Maximize size={16} />}
        </button>
      </div>
      <div className="mv-stage" ref={stage}>
        <div
          className={`mv-grid ${layout.inset ? "with-inset" : ""}`}
          style={{ width: grid.width, height: grid.height, gap: GAP, gridTemplateColumns: `repeat(${layout.columns}, 1fr)`, gridTemplateRows: `repeat(${layout.rows}, 1fr)` }}
        >
          {/* In the order they were added, so a tile moving (or swapping with the inset) never moves its video element. */}
          {[...tiles]
            .sort((a, b) => a.seq - b.seq)
            .map((tile) => {
              const small = tile.id === layout.inset;
              const channel = tileChannel(tile);
              return (
                <TileView
                  key={tile.id}
                  tile={tile}
                  index={ids.indexOf(tile.id)}
                  count={tiles.length}
                  place={places.get(tile.id)}
                  audible={tile.id === audio}
                  expanded={tile.id === expanded}
                  inset={layout.inset ? (small ? (insetHidden ? "hidden" : "small") : "big") : null}
                  insetCorner={small ? { corner: insetCorner, drag: corner, element: insetElement } : null}
                  programme={channel && programmes ? (programmes.get(channel.id) ?? null) : channel ? undefined : null}
                  recording={channel ? recordingOn(recordings, channel) : null}
                />
              );
            })}
          {layout.invite && (
            <button className="mv-invite" style={{ gridColumn: layout.invite.column, gridRow: layout.invite.row }} onClick={() => choose({ purpose: "add" })}>
              <Grid2x2Plus size={30} strokeWidth={1.4} />
              <strong>Add a channel, film or episode</strong>
              <span>Watch up to four at once, and choose which one you hear.</span>
            </button>
          )}
          {layout.inset && insetHidden && (
            <button className={`mv-inset-show glass corner-${insetCorner}`} onClick={toggleInsetHidden} title="Show the inset again">
              <Eye size={14} /> {insetTitle}
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

type CornerDrag = ReturnType<typeof useMiniPlayerCorner>;

interface TileProps {
  tile: Tile;
  /** Its place in the layout: 0 is the first (and the large one of three, or the big picture with an inset). */
  index: number;
  count: number;
  /** Where it sits; none while another tile fills the view, or for the inset. */
  place: TilePlace | undefined;
  audible: boolean;
  expanded: boolean;
  /** With the inset on: the big picture, the inset, or the inset out of sight. */
  inset: "big" | "small" | "hidden" | null;
  /** The inset's corner and its dragging, for the inset. */
  insetCorner: { corner: Corner; drag: CornerDrag; element: React.RefObject<HTMLDivElement | null> } | null;
  /** A channel's programme on now: null when the guide has nothing (or it's a film), undefined until it's been asked. */
  programme: Program | null | undefined;
  recording: Recording | null;
}

function TileView({ tile, index, count, place, audible, expanded, inset, insetCorner, programme, recording }: TileProps) {
  const video = useRef<HTMLVideoElement>(null);
  const [buffering, setBuffering] = useState(true);
  const audibleNow = useRef(audible);
  audibleNow.current = audible;
  const { id, stream, attempt, item, paused, behind } = tile;
  const channel = tileChannel(tile);
  const live = !!channel;
  const pausedNow = useRef(paused);
  pausedNow.current = paused;
  const holdNow = useRef(tile.holdAtStart);
  holdNow.current = tile.holdAtStart;
  const itemNow = useRef(item);
  itemNow.current = item;
  // The try whose film or episode was taken to where it was left: once per
  // try, since seeking one ffmpeg remuxes gives it another stream.
  const resumed = useRef(-1);
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
    // A film or episode carries on from where it was left, as in the player;
    // one ffmpeg remuxes is asked for again from there.
    const resumeFrom = stream.isLive || resumed.current === attempt ? 0 : (itemNow.current.resumeFrom ?? 0);
    const toResume = () => {
      resumed.current = attempt;
      if (stream.mode !== "remux") el.currentTime = resumeFrom - stream.startOffsetSeconds;
      else seekTile(id, resumeFrom);
    };
    if (resumeFrom > 5 && Math.abs(stream.startOffsetSeconds - resumeFrom) > 5) el.addEventListener("loadedmetadata", toResume, { once: true });
    else resumed.current = attempt;
    // A paused tile needn't play, only load.
    const watchdog = setTimeout(() => {
      if (cancelled || started || el.readyState >= (pausedNow.current ? el.HAVE_METADATA : el.HAVE_FUTURE_DATA)) return;
      failTile(id, attempt, stream.isLive ? "This channel didn't start. It may be offline." : "This video didn't start.");
    }, 45_000);
    return () => {
      cancelled = true;
      clearTimeout(watchdog);
      el.removeEventListener("playing", onPlaying);
      el.removeEventListener("loadedmetadata", toResume);
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

  // How far behind live it plays from its buffer (growing while it's
  // paused), or how far into a film or episode it is.
  useEffect(() => {
    if (!stream || (stream.isLive && !stream.timeshift)) return;
    const measure = () => {
      const el = video.current;
      if (!el) return;
      if (stream.isLive) return reportBehind(id, secondsBehind(bufferRange(el), el.currentTime));
      const duration = stream.durationSeconds ?? itemNow.current.durationHint ?? (Number.isFinite(el.duration) ? el.duration : 0);
      if (el.currentTime > 0) reportPosition(id, stream.startOffsetSeconds + el.currentTime, duration);
    };
    measure();
    const timer = setInterval(measure, 1000);
    return () => clearInterval(timer);
  }, [id, stream]);

  const menu = (e: React.MouseEvent, below = false) => openMenu(e, tileMenu(tile, { index, count, audible, expanded, inset, programme, recording }), below);
  const behindLive = buffered && behind >= LIVE_EDGE;
  const small = inset === "small" || inset === "hidden";
  const style: React.CSSProperties = small
    ? { ...insetCorner?.drag.style, ...(inset === "hidden" ? { display: "none" } : {}) }
    : place
      ? { gridColumn: `${place.column} / span ${place.span}`, gridRow: `${place.row} / span ${place.span}` }
      : { display: "none" };
  const stop = (e: React.SyntheticEvent) => e.stopPropagation();
  const progress = !live && tile.duration > 0 ? Math.min(1, tile.position / tile.duration) : null;
  const badge = tile.ended ? "Finished" : behindLive ? `−${playerClock(behind)}` : "Paused";
  const subtitle = live ? (programme === undefined ? " " : (programme?.title ?? "No guide listings")) : [item.subtitle, tile.duration ? `${playerClock(tile.position)} of ${playerClock(tile.duration)}` : null].filter(Boolean).join(" · ");
  const classes = ["mv-tile", audible ? "audible" : "", small ? `inset corner-${insetCorner?.corner ?? "bottomRight"}` : "", insetCorner?.drag.dragging ? "dragging" : ""].filter(Boolean).join(" ");

  return (
    <div
      ref={small ? insetCorner?.element : undefined}
      className={classes}
      style={style}
      onPointerDown={small ? insetCorner?.drag.onPointerDown : undefined}
      onClick={() => {
        // The end of dragging the inset isn't a click.
        if (small && insetCorner?.drag.takeDragged()) return;
        listen(id);
      }}
      onDoubleClick={() => (inset === "small" ? swapInset() : inset ? undefined : toggleExpand(id))}
      onContextMenu={menu}
    >
      <video
        ref={video}
        className="mv-video"
        playsInline
        onWaiting={() => setBuffering(true)}
        onPlaying={() => setBuffering(false)}
        onCanPlay={() => setBuffering(false)}
        onEnded={(e) => {
          if (live) return;
          // A film that ends minutes early lost its connection.
          const at = (stream?.startOffsetSeconds ?? 0) + e.currentTarget.currentTime;
          if (tile.duration > 0 && at < tile.duration - 120) failTile(id, attempt, "The stream ended early.");
          else endTile(id);
        }}
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
            <strong>Can't play {item.title}</strong>
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
          <span className="mv-behind-badge" title={tile.ended ? "Played to the end" : behindLive ? `${playerClock(behind)} behind live` : "Paused"}>
            {paused && !tile.ended && <Pause size={10} fill="currentColor" strokeWidth={0} />}
            {badge}
          </span>
        )}
        {recording && (
          <span className="rec-badge active">
            <span className="dot" />
            REC
          </span>
        )}
      </div>
      {progress !== null && (
        <div className="mv-progress" aria-hidden="true">
          <span style={{ width: `${progress * 100}%` }} />
        </div>
      )}
      <div className="mv-tile-chrome">
        <div className="mv-tile-info">
          <span className="mv-key" title={`Press ${index + 1} to listen to this one`}>
            {index + 1}
          </span>
          {!small && <ChannelLogo src={channel ? channel.logoUrl : item.artworkUrl} name={item.title} size={30} />}
          <div className="mv-tile-text">
            <div className="mv-tile-name">{item.title}</div>
            <div className="mv-tile-programme">{subtitle || " "}</div>
          </div>
        </div>
        <div className="mv-tile-actions" onClick={stop} onDoubleClick={stop} onPointerDown={stop}>
          {live && (paused || buffered) && !small && (
            <button className="live-pill behind" onClick={() => goLiveTile(id)} title={audible ? `Back to live (${shortcutLabel("playback.goLive")})` : "Back to live"}>
              Go Live
            </button>
          )}
          <button className="player-round small glass" onClick={() => setTilePaused(id, !paused)} aria-label={paused ? "Play" : "Pause"} title={paused ? "Play" : live ? "Pause, to carry on from here" : "Pause"}>
            {paused ? <Play size={12} fill="currentColor" strokeWidth={0} /> : <Pause size={12} fill="currentColor" strokeWidth={0} />}
          </button>
          <button className={`player-round small glass ${audible ? "on" : ""}`} onClick={() => listen(id)} aria-label={audible ? "You're hearing this one" : "Listen to this one"} title={audible ? "You're hearing this one" : `Listen to this one (${index + 1})`}>
            {audible ? <Volume2 size={13} /> : <VolumeX size={13} />}
          </button>
          {inset ? (
            <button className="player-round small glass" onClick={swapInset} aria-label="Swap the two" title={`Swap the big picture and the small one (${INSET_KEY.toUpperCase()})`}>
              <Replace size={13} />
            </button>
          ) : (
            <button className="player-round small glass" onClick={() => toggleExpand(id)} aria-label={expanded ? "Show all" : "Expand"} title={expanded ? "Show all (Esc)" : "Fill the view"}>
              {expanded ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
            </button>
          )}
          {small && (
            <button className="player-round small glass" onClick={toggleInsetHidden} aria-label="Hide" title="Hide it for now">
              <EyeOff size={13} />
            </button>
          )}
          {!small && (
            <button className="player-round small glass" onClick={() => choose({ purpose: "swap", tileId: id })} aria-label="Watch something else here" title="Watch something else here">
              <ArrowLeftRight size={13} />
            </button>
          )}
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
function tileMenu(
  tile: Tile,
  at: { index: number; count: number; audible: boolean; expanded: boolean; inset: TileProps["inset"]; programme: Program | null | undefined; recording: Recording | null },
): MenuEntry[] {
  const { id, item, paused } = tile;
  const { index, count, audible, expanded, inset, programme, recording } = at;
  const channel = tileChannel(tile);
  const buffered = !!tile.stream?.timeshift;
  const items: MenuEntry[] = [{ heading: item.title }];
  if (!audible) items.push({ label: "Listen to This One", icon: <Volume2 {...icon} />, shortcut: String(index + 1), onSelect: () => listen(id) });
  // The keys act on the tile heard.
  const key = (shortcut: string) => (audible ? shortcut : undefined);
  items.push({ label: paused ? "Play" : "Pause", icon: paused ? <Play {...icon} /> : <Pause {...icon} />, shortcut: key("Space"), onSelect: () => setTilePaused(id, !paused) });
  if (buffered || !channel)
    items.push(
      { label: "Skip Back 10 Seconds", icon: <RotateCcw {...icon} />, shortcut: key("←"), onSelect: () => skipTile(id, -10) },
      { label: "Skip Forward 10 Seconds", icon: <RotateCw {...icon} />, shortcut: key("→"), onSelect: () => skipTile(id, 10) },
    );
  if (channel && (buffered || paused)) items.push({ label: "Go Live", icon: <Tv {...icon} />, onSelect: () => goLiveTile(id) });
  items.push({ separator: true });
  if (inset) {
    items.push({ label: "Swap the Two", icon: <Replace {...icon} />, shortcut: INSET_KEY.toUpperCase(), onSelect: swapInset });
    if (inset === "small") items.push({ label: "Hide", icon: <EyeOff {...icon} />, onSelect: toggleInsetHidden });
    items.push({ label: "Side by Side", icon: <Columns2 {...icon} />, onSelect: toggleInset });
  } else {
    items.push({ label: expanded ? "Show All" : "Fill the View", icon: expanded ? <Minimize2 {...icon} /> : <Maximize2 {...icon} />, onSelect: () => toggleExpand(id) });
    if (canInset(count))
      items.push({
        label: "Inset the Other",
        icon: <PanelsRightBottom {...icon} />,
        onSelect: () => {
          // This one stays big, as the one heard does.
          listen(id);
          toggleInset();
        },
      });
  }
  items.push({ label: channel ? "Swap Channel…" : "Watch Something Else Here…", icon: <ArrowLeftRight {...icon} />, onSelect: () => choose({ purpose: "swap", tileId: id }) });
  if (channel)
    items.push(
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
  if (!expanded && !inset && count > 1)
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

/** Why something can't be chosen (it's on already), given how to tell it's the same. */
type Note = (matches: (item: NowPlaying) => boolean) => string | null;

/** What Return chooses: the first row that can be chosen, as each list says. */
type Enter = React.RefObject<(() => void) | null>;

/**
 * Choosing what a tile plays, for another tile, for a tile's place, or for
 * an inset over what's playing: a channel (favourites first, then every
 * channel), or a film or episode (Continue Watching first, then any found).
 */
function ItemPicker({ picker }: { picker: Picker }) {
  const tiles = useMultiview((s) => s.tiles);
  const open = useMultiview((s) => s.open);
  const nowPlaying = useApp((s) => s.nowPlaying);
  const swapping = picker.purpose === "swap" ? tiles.find((t) => t.id === picker.tileId) : undefined;
  const playing = !open ? nowPlaying : null;
  // An inset over a channel is most likely a film, and over a film a channel.
  const [tab, setTab] = useState<"channels" | "titles">(() => {
    const beside = swapping?.item ?? (picker.purpose === "inset" ? playing : null);
    if (!beside) return "channels";
    return isLiveItem(beside) === (picker.purpose === "inset") ? "titles" : "channels";
  });
  const [text, setText] = useState("");
  const enter = useRef<(() => void) | null>(null);

  const note: Note = (matches) => {
    if (swapping && matches(swapping.item)) return "In this tile";
    if (tiles.some((t) => matches(t.item))) return "In Multiview";
    if (playing && matches(playing)) return "Playing now";
    return null;
  };

  const close = () => choose(null);
  const title = swapping ? "Watch Something Else Here" : picker.purpose === "inset" ? "Add an Inset" : open ? "Add to Multiview" : "Multiview";
  const subtitle = swapping
    ? `Choose something to watch instead of ${swapping.item.title}.`
    : picker.purpose === "inset"
      ? `Choose something to watch in a corner of ${playing?.title ?? "what's playing"}. Press ${INSET_KEY.toUpperCase()} to swap the two.`
      : open
        ? `Choose a channel, film or episode to watch beside the others. Multiview shows up to ${MAX_TILES}.`
        : playing
          ? `Choose a channel, film or episode to watch beside ${playing.title}. You can add up to ${MAX_TILES}.`
          : `Choose a channel, film or episode to start with. You can add up to ${MAX_TILES}.`;

  return (
    <Sheet title={title} subtitle={subtitle} onClose={close} width={520} footer={<button className="btn-plain" onClick={close}>Cancel</button>}>
      {/* The keys stay here, not with the player or Multiview underneath. */}
      <div
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") close();
        }}
      >
        <div className="segmented mv-picker-tabs">
          <button className={tab === "channels" ? "selected" : ""} onClick={() => setTab("channels")}>
            Channels
          </button>
          <button className={tab === "titles" ? "selected" : ""} onClick={() => setTab("titles")}>
            Films & Series
          </button>
        </div>
        <label className="mv-picker-search">
          <Search size={14} />
          <input
            key={tab}
            className="text-input"
            autoFocus
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && enter.current?.()}
            placeholder={tab === "channels" ? "Search channels" : "Search films and series"}
            spellCheck={false}
            autoComplete="off"
          />
        </label>
        {tab === "channels" ? <ChannelRows text={text} note={note} enter={enter} /> : <TitleRows text={text} note={note} enter={enter} />}
      </div>
    </Sheet>
  );
}

/** Every channel, favourites first, narrowed by what's typed. */
function ChannelRows({ text, note, enter }: { text: string; note: Note; enter: Enter }) {
  const [favorites, setFavorites] = useState<Channel[] | null>(null);
  const [all, setAll] = useState<Channel[] | null>(null);
  const query = fold(text.trim());

  useEffect(() => {
    api.channels({ favoritesOnly: true }).then(setFavorites).catch(() => setFavorites([]));
    api.channels({}).then(setAll).catch(() => setAll([]));
  }, []);

  // Names folded once, so typing stays quick in a long list.
  const list = useMemo(() => {
    if (!favorites || !all) return null;
    const favoriteIds = new Set(favorites.flatMap(versionIds));
    const rest = all.filter((c) => !versionIds(c).some((v) => favoriteIds.has(v)));
    return [...favorites.map((channel) => ({ channel, favorite: true })), ...rest.map((channel) => ({ channel, favorite: false }))].map((row) => ({ ...row, name: fold(row.channel.name) }));
  }, [favorites, all]);
  const rows = useMemo(() => (list && query ? list.filter((row) => row.name.includes(query) || String(row.channel.number ?? "") === query) : list), [list, query]);

  const why = (channel: Channel) => note((item) => isLiveItem(item) && sameItem(item, channelItem(channel)));
  const shown = rows?.slice(0, PICKER_ROWS) ?? [];
  const first = shown.find(({ channel }) => !why(channel));
  enter.current = first ? () => picked(channelItem(first.channel)) : null;

  return (
    <div className="mv-picker-list" role="listbox" aria-label="Channels">
      {rows == null ? (
        <div className="spinner" style={{ margin: "14px auto" }} />
      ) : rows.length === 0 ? (
        <p className="muted mv-picker-empty">{query ? "No channel has that name." : "There are no live channels in your sources yet."}</p>
      ) : (
        shown.map(({ channel, favorite }, i) => {
          const taken = why(channel);
          const heading = !query && (i === 0 || shown[i - 1].favorite !== favorite) ? (favorite ? "Favorites" : "All Channels") : null;
          return (
            <div key={channel.id} className="mv-picker-item">
              {heading && <div className="mv-picker-heading">{heading}</div>}
              <button className="copy-row mv-picker-row" disabled={!!taken} onClick={() => picked(channelItem(channel))} role="option" aria-selected={false}>
                <ChannelLogo src={channel.logoUrl} name={channel.name} size={30} />
                <span className="copy-text">
                  <span className="copy-name">
                    {channel.number != null && <span className="mv-picker-number">{channel.number}</span>}
                    {channel.name}
                  </span>
                  {taken && <span className="copy-meta">{taken}</span>}
                </span>
                {favorite && <Star size={13} fill="#f5c84c" color="#f5c84c" className="mv-picker-star" />}
              </button>
            </div>
          );
        })
      )}
      {rows && rows.length > PICKER_ROWS && <p className="muted mv-picker-more">Showing {PICKER_ROWS} of {rows.length}. Type to find the rest.</p>}
    </div>
  );
}

/** One row of the films and episodes list. */
interface TitleRow {
  key: string;
  itemId: string;
  name: string;
  meta: string | null;
  artwork: string | null;
  /** A folder's, in place of artwork. */
  folder?: boolean;
  heading: string;
  /** What choosing it does: the item to play, or (for a series or a folder) what's in it. */
  choose: () => Promise<NowPlaying | null> | void;
  /** It opens to more rows. */
  opens?: boolean;
}

/** A media folder being looked through: the source, and the folder in it (null for the source's own). */
interface Browsing {
  sourceId: string;
  name: string;
  folder: string | null;
  listing: FolderListing | null;
  /** The source's series, at its own folder: their episodes aren't among a folder's videos. */
  series: Series[];
}

/** Videos listed from a folder at once; the library page has the rest. */
const FOLDER_ROWS = 200;

/**
 * Films and episodes: what's in Continue Watching (and up next) and the
 * media folders to look through, or the films and series found for what's
 * typed; a series opens to its episodes and a folder to its folders and
 * videos. Add-on titles aren't here, since one needs a copy chosen first.
 */
function TitleRows({ text, note, enter }: { text: string; note: Note; enter: Enter }) {
  const [continuing, setContinuing] = useState<ContinueItem[] | null>(null);
  const [found, setFound] = useState<{ text: string; movies: Movie[]; series: Series[] } | null>(null);
  const [opened, setOpened] = useState<{ series: Series; episodes: Episode[] | null } | null>(null);
  const [browsing, setBrowsing] = useState<Browsing | null>(null);
  const [busy, setBusy] = useState(false);
  const sources = useSources((s) => s.sources);
  const folders = useMemo(() => (sources ?? []).filter((s) => s.config.kind === "folder" && !s.config.isPaused).map((s) => s.config), [sources]);
  const query = text.trim();

  useEffect(() => {
    api
      .continueWatching(40)
      .then((items) => setContinuing(items.filter((i) => i.sourceId !== "addon" && (i.kind === "movie" || i.kind === "episode"))))
      .catch(() => setContinuing([]));
  }, []);

  // What's typed, looked up once typing pauses.
  useEffect(() => {
    setOpened(null);
    if (query) setBrowsing(null);
    if (!query) return setFound(null);
    const timer = setTimeout(() => {
      api
        .search(query)
        .then((r) => setFound({ text: query, movies: r.movies.filter((m) => m.stream.type !== "addon"), series: r.series.filter((s) => s.sourceId !== "addon") }))
        .catch(() => setFound({ text: query, movies: [], series: [] }));
    }, 250);
    return () => clearTimeout(timer);
  }, [query]);

  const openSeries = (series: Series) => {
    setOpened({ series, episodes: null });
    api
      .episodes(series.id)
      .then((episodes) => setOpened((o) => (o?.series.id === series.id ? { series, episodes: episodes.filter((e) => e.stream.type !== "addon") } : o)))
      .catch(() => setOpened((o) => (o?.series.id === series.id ? { series, episodes: [] } : o)));
  };

  const openFolder = (sourceId: string, name: string, folder: string | null) => {
    setBrowsing({ sourceId, name, folder, listing: null, series: [] });
    const here = (b: Browsing | null): b is Browsing => b?.sourceId === sourceId && b.folder === folder;
    const empty: FolderListing = { folder, trail: [], folders: [], videos: [], total: 0, unread: 0 };
    Promise.all([
      api.folderListing({ sourceId, folder, below: false, kind: null, length: null, sort: "name", limit: FOLDER_ROWS, offset: 0 }).catch(() => empty),
      folder === null ? api.seriesList({ sourceId, sort: "title", limit: FOLDER_ROWS, offset: 0 }).catch((): Series[] => []) : Promise.resolve([] as Series[]),
    ]).then(([listing, series]) => setBrowsing((b) => (here(b) ? { ...b, listing, series } : b)));
  };

  /** Up a folder, or out of the media folder. */
  const back = () => {
    if (opened) return setOpened(null);
    // The trail runs from the source's own folder to the one shown, as the library page's does.
    const trail = browsing?.listing?.trail ?? [];
    if (!browsing || browsing.folder === null || trail.length < 2) return setBrowsing(null);
    const parent = trail[trail.length - 2];
    openFolder(browsing.sourceId, browsing.name, parent.path === trail[0].path ? null : parent.path);
  };

  // A series' episodes, a media folder, what was found, or what's in Continue Watching and the media folders.
  const rows = ((): TitleRow[] | null => {
    if (opened)
      return (
        opened.episodes?.map((episode) => ({
          key: episode.id,
          itemId: episode.id,
          name: `${episodeCode(episode.season, episode.number)} · ${episode.title}`,
          meta: null,
          artwork: episode.stillUrl ?? opened.series.posterUrl,
          heading: opened.series.title,
          choose: () => episodeItem(episode, opened.series),
        })) ?? null
      );
    if (browsing) {
      const { listing, series } = browsing;
      if (!listing) return null;
      const heading = listing.trail.at(-1)?.name ?? browsing.name;
      return [
        // A folder of a series' episodes alone counts no videos: the series is listed instead.
        ...listing.folders
          .filter((entry) => entry.videos > 0)
          .map((entry) => ({
            key: `folder:${entry.path}`,
            itemId: "",
            name: entry.name,
            meta: `${entry.videos.toLocaleString()} ${entry.videos === 1 ? "video" : "videos"}`,
            artwork: null,
            folder: true,
            opens: true,
            heading,
            choose: () => openFolder(browsing.sourceId, browsing.name, entry.path),
          })),
        ...series.map((one) => ({
          key: one.id,
          itemId: one.id,
          name: one.title,
          meta: one.year ? `${one.year} · Series` : "Series",
          artwork: one.posterUrl ?? one.backdropUrl,
          opens: true,
          heading: "Series",
          // Its episodes, with Back to the folder.
          choose: () => openSeries(one),
        })),
        ...listing.videos.map(({ movie, facts }) => ({
          key: movie.id,
          itemId: movie.id,
          name: movie.title,
          meta: facts?.durationSeconds ? playerClock(facts.durationSeconds) : movie.year ? String(movie.year) : null,
          artwork: movie.posterUrl ?? movie.backdropUrl,
          heading: series.length ? "Films" : heading,
          choose: () => movieItem(movie),
        })),
      ];
    }
    if (query) {
      if (!found || found.text !== query) return null;
      return [
        ...found.movies.map((movie) => ({
          key: movie.id,
          itemId: movie.id,
          name: movie.title,
          meta: movie.year ? String(movie.year) : null,
          artwork: movie.posterUrl ?? movie.backdropUrl,
          heading: "Films",
          choose: () => movieItem(movie),
        })),
        ...found.series.map((series) => ({
          key: series.id,
          itemId: series.id,
          name: series.title,
          meta: series.year ? `${series.year} · Series` : "Series",
          artwork: series.posterUrl ?? series.backdropUrl,
          heading: "Series",
          opens: true,
          choose: () => openSeries(series),
        })),
      ];
    }
    if (!continuing) return null;
    return [
      ...continuing.map((item) => ({
        key: item.itemId,
        itemId: item.itemId,
        name: item.title,
        meta: [item.subtitle, item.upNext ? "Up next" : item.remainingSeconds ? remaining(item.remainingSeconds) : null].filter(Boolean).join(" · ") || null,
        artwork: item.artworkUrl,
        heading: "Continue Watching",
        choose: () => continueItem(item),
      })),
      ...folders.map((config) => ({
        key: `source:${config.id}`,
        itemId: "",
        name: config.name,
        meta: "Media folder",
        artwork: null,
        folder: true,
        opens: true,
        heading: "Media Folders",
        choose: () => openFolder(config.id, config.name, null),
      })),
    ];
  })();

  const why = (row: TitleRow) => (row.itemId ? note((item) => !isLiveItem(item) && item.itemId === row.itemId) : null);
  const pick = async (row: TitleRow) => {
    if (busy) return;
    const made = row.choose();
    if (!made) return;
    setBusy(true);
    const item = await made.catch(() => null);
    setBusy(false);
    if (item) picked(item);
    else useApp.getState().showToast(`${row.name} can't play here.`, "error");
  };
  const shown = rows?.slice(0, PICKER_ROWS) ?? [];
  const first = shown.find((row) => !why(row));
  enter.current = first ? () => pick(first) : null;

  return (
    <div className="mv-picker-list" role="listbox" aria-label="Films and series">
      {(opened || browsing) && (
        <button className="copy-row mv-picker-row mv-picker-back" onClick={back}>
          <ChevronLeft size={15} />
          <span className="copy-text">
            <span className="copy-name">{opened && query ? "Back to what was found" : "Back"}</span>
          </span>
        </button>
      )}
      {rows == null ? (
        <div className="spinner" style={{ margin: "14px auto" }} />
      ) : rows.length === 0 ? (
        <p className="muted mv-picker-empty">
          {browsing ? "There are no videos in this folder." : opened ? "This series has no episodes to play here." : query ? "No film or series has that name." : "Search for a film or series in your sources."}
        </p>
      ) : (
        shown.map((row, i) => {
          const taken = why(row);
          const heading = i === 0 || shown[i - 1].heading !== row.heading ? row.heading : null;
          return (
            <div key={row.key} className="mv-picker-item">
              {heading && <div className="mv-picker-heading">{heading}</div>}
              <button className="copy-row mv-picker-row" disabled={!!taken || busy} onClick={() => pick(row)} role="option" aria-selected={false}>
                {row.folder ? (
                  <span className="channel-logo mv-picker-folder" style={{ width: 30, height: 30 }}>
                    <Folder size={15} />
                  </span>
                ) : (
                  <ChannelLogo src={row.artwork} name={row.name} size={30} />
                )}
                <span className="copy-text">
                  <span className="copy-name">{row.name}</span>
                  {(taken ?? row.meta) && <span className="copy-meta">{taken ?? row.meta}</span>}
                </span>
                {row.opens && <ChevronRight size={14} className="mv-picker-star" />}
              </button>
            </div>
          );
        })
      )}
      {rows && rows.length > PICKER_ROWS && <p className="muted mv-picker-more">Showing {PICKER_ROWS} of {rows.length}. Type to find the rest.</p>}
      {browsing?.listing && browsing.listing.total > browsing.listing.videos.length && (
        <p className="muted mv-picker-more">
          Showing {browsing.listing.videos.length} of {browsing.listing.total.toLocaleString()} videos. Search finds the rest.
        </p>
      )}
    </div>
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
