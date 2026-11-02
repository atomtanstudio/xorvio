import { Airplay, ArrowDownLeftFromSquare, ArrowLeft, AudioLines, CalendarClock, Captions, Check, ChevronDown, ChevronUp, CircleDot, CircleStop, ExternalLink, Grid2x2Plus, Layers, ListVideo, Maximize, Minimize, PanelsRightBottom, Pause, PictureInPicture2, Play, RotateCcw, RotateCw, Shrink, SlidersHorizontal, SquareArrowOutUpRight, Volume1, Volume2, VolumeX } from "lucide-react";
import { Component, useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { create } from "zustand";
import { ChannelLogo } from "../components/Artwork";
import { openMenu, type MenuEntry } from "../components/Menu";
import { cancelOrStopRecording, playLastChannel, recordChannelNow } from "../lib/actions";
import { api, errorText, isTauri } from "../lib/api";
import { handleCommands, isMenuShortcut, reportPlayback, shortcutLabel } from "../lib/commands";
import { clockTime, nowSeconds, playerClock, remaining } from "../lib/format";
import { languageName } from "../lib/languages";
import { recordingFor, useApp } from "../lib/store";
import { isPopOutWindow } from "../lib/windows";
import type { Channel, ExternalPlayer, MediaTrack, Program, ResolvedStream, StreamOptions } from "../lib/types";
import { switchRequest } from "../screens/Copies";
import { CastButton, CastPanel, isCasting, useCasting, type Casting } from "./cast";
import { changeVolume, registerPlayer, SAVE_PROGRESS_EVENT, toggleMute, togglePictureInPicture } from "./controls";
import { attachStream, type EngineHandle } from "./engine";
import { externalPlayers, externalTitle, openInPlayer, refreshExternalPlayers } from "./external";
import { isWindowFullscreen, leavePlayerFullscreen, madeFullscreen, togglePlayerFullscreen } from "./fullscreen";
import {
  cancelChannelDigits,
  ChannelListOverlay,
  ChannelNumberEntry,
  closeChannelList,
  closeNowNext,
  moveListHighlight,
  NowNextCard,
  stepListGroup,
  toggleChannelList,
  toggleNowNext,
  typeChannelDigit,
  useLiveOverlays,
  watchHighlighted,
} from "./LiveOverlays";
import { useMediaSession } from "./mediaSession";
import { INSET_KEY } from "./multiviewRules";
import { goLiveTile, heardTile, insetCommand, listenNext, skipTile, toggleAllPaused, toggleHeardPaused, toggleMultiview, useMultiview } from "./multiviewStore";
import { playNextEpisode, useEpisodeExtras } from "./nextEpisode";
import { pauseByPerson, takePausedByPerson } from "./pause";
import { CornerButton, NextCountdown, StreamInfoPanel, SyncPanel } from "./PlayerPanels";
import { audioFor, bufferRange, classifyFailure, LIVE_EDGE, watchedEnough, withAudio, withSubtitles, type Problem } from "./playerRules";
import { FailureCard, ReconnectingCard } from "./PlayerStates";
import { retry, streamFailed, streamPlaying, takeResumeSeek, titleHasPlayed, useRecovery } from "./recovery";
import { DockedControls, MiniStrip, useMiniPlayerCorner } from "./SmallPlayers";
import { adBreakAt, SeekTrack, useSeekExtras, useSeekExtrasLoader } from "./seekbar";
import { useSubtitles } from "./subtitles";
import { useTrackMemory, type TrackMemory } from "./trackMemory";
import { useTraktScrobble } from "./trakt";
import { channelDown, channelUp } from "./zap";

type Zoom = "fit" | "fill" | "stretch";
export { SAVE_PROGRESS_EVENT } from "./controls";

const zoomFit: Record<Zoom, React.CSSProperties["objectFit"]> = { fit: "contain", fill: "cover", stretch: "fill" };

/** The zoom stays as it was last set, on this Mac. */
function savedZoom(): Zoom {
  try {
    const z = localStorage.getItem("player.zoom");
    return z === "fill" || z === "stretch" ? z : "fit";
  } catch {
    return "fit";
  }
}

// MARK: Dock target

const useDock = create<{ el: HTMLElement | null; set(el: HTMLElement | null): void }>((set) => ({ el: null, set: (el) => set({ el }) }));

/** Marks where docked video sits (the Live TV panel). The player follows its position. */
export function DockTarget({ className, children }: { className?: string; children?: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    useDock.getState().set(ref.current);
    return () => {
      if (useDock.getState().el === ref.current) useDock.getState().set(null);
    };
  }, []);
  return (
    <div ref={ref} className={className}>
      {children}
    </div>
  );
}

function useDockRect(active: boolean) {
  const el = useDock((s) => s.el);
  const [rect, setRect] = useState<DOMRect | null>(null);
  useEffect(() => {
    if (!active || !el) return setRect(null);
    let frame = 0;
    const update = () => {
      const r = el.getBoundingClientRect();
      setRect((old) => (old && old.x === r.x && old.y === r.y && old.width === r.width && old.height === r.height ? old : r));
      frame = requestAnimationFrame(update);
    };
    update();
    return () => cancelAnimationFrame(frame);
  }, [active, el]);
  return rect;
}

// MARK: AirPlay

/** WebKit's AirPlay additions to the video element. */
type AirPlayVideo = HTMLVideoElement & { webkitShowPlaybackTargetPicker?: () => void; webkitCurrentPlaybackTargetIsWireless?: boolean };

/**
 * Whether there are AirPlay devices about, and whether the video is playing
 * to one. WebKit only looks for devices while something listens.
 */
function useAirPlay(video: React.RefObject<HTMLVideoElement | null>, mounted: boolean) {
  const [state, setState] = useState({ available: false, active: false });
  useEffect(() => {
    const el = video.current as AirPlayVideo | null;
    if (!el || !el.webkitShowPlaybackTargetPicker) return;
    const onAvailability = (e: Event) => setState((s) => ({ ...s, available: (e as Event & { availability?: string }).availability === "available" }));
    const onTarget = () => setState((s) => ({ ...s, active: !!el.webkitCurrentPlaybackTargetIsWireless }));
    el.addEventListener("webkitplaybacktargetavailabilitychanged", onAvailability);
    el.addEventListener("webkitcurrentplaybacktargetiswirelesschanged", onTarget);
    return () => {
      el.removeEventListener("webkitplaybacktargetavailabilitychanged", onAvailability);
      el.removeEventListener("webkitcurrentplaybacktargetiswirelesschanged", onTarget);
      setState({ available: false, active: false });
    };
  }, [video, mounted]);
  return state;
}

// MARK: Player

/**
 * A fault while drawing the player closes the player, with a word about
 * it, rather than taking the whole window with it.
 */
class PlayerBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  componentDidCatch(error: Error) {
    api.log("error", `player: ${error.message}`).catch(() => {});
    const { stop, showToast } = useApp.getState();
    stop();
    showToast("The player ran into a problem and closed.", "error");
    this.setState({ failed: false });
  }
  render() {
    return this.state.failed ? null : this.props.children;
  }
}

export function PlayerHost() {
  return (
    <PlayerBoundary>
      <Player />
    </PlayerBoundary>
  );
}

function Player() {
  const nowPlaying = useApp((s) => s.nowPlaying);
  const stream = useApp((s) => s.stream);
  const presentation = useApp((s) => s.presentation);
  const playError = useApp((s) => s.playError);
  const video = useRef<HTMLVideoElement>(null);
  const root = useRef<HTMLDivElement>(null);
  const engine = useRef<EngineHandle | null>(null);
  // A stream being opened again after it dropped, or one that can't play (recovery.ts).
  const reconnecting = useRecovery((s) => s.reconnecting);
  const failed = useRecovery((s) => s.failure);
  const [buffering, setBuffering] = useState(true);
  const [paused, setPaused] = useState(false);
  const [muted, setMuted] = useState(false);
  const [time, setTime] = useState({ current: 0, duration: 0 });
  // Remuxed streams restart at an offset to seek; the timeline adds it back.
  const [offset, setOffset] = useState(0);
  const dockRect = useDockRect(presentation === "docked");
  // Resuming happens once per title, not again when its stream restarts.
  const resumedFor = useRef<typeof nowPlaying>(null);
  const settings = useApp((s) => s.settings);
  const [zoom, setZoomState] = useState<Zoom>(savedZoom);
  const [speed, setSpeed] = useState(1);
  const [panel, setPanel] = useState<"sync" | "info" | null>(null);
  const [countdown, setCountdown] = useState(false);
  const skipped = useRef(new Set<string>());
  // Audio and subtitles chosen for the series an episode belongs to.
  const tracks = useTrackMemory(nowPlaying);
  const audioApplied = useRef(false);
  const subtitles = useSubtitles(video, stream, nowPlaying, tracks);
  const { next, segments } = useEpisodeExtras(nowPlaying);
  const airplay = useAirPlay(video, !!nowPlaying);
  // Where the title last was, in its own time, kept through a stream going (for carrying on, or another copy).
  const lastPosition = useRef(0);
  const mini = useMiniPlayerCorner(root, presentation === "mini");

  // Look for other players at launch, so menus can offer them straight away.
  useEffect(refreshExternalPlayers, []);

  const isLive = stream?.isLive ?? nowPlaying?.kind === "channel";
  const buffered = isLive && !!stream?.timeshift;
  // The buffer was started by a pause: its stream opens paused, at its start.
  const holdPaused = useRef(false);
  const startingBuffer = useRef(false);
  // The buffer didn't start, and the channel's own stream went with the try: Play tunes in again.
  const retuneOnPlay = useRef(false);

  /** Moves a live channel playing directly onto a buffer, from now. */
  const startBuffer = useCallback(async () => {
    const current = useApp.getState().stream;
    if (!current?.isLive || current.timeshift || startingBuffer.current) return;
    if (useApp.getState().settings.liveBuffer !== "paused" || useApp.getState().nowPlaying?.kind !== "channel") return;
    startingBuffer.current = true;
    try {
      const fresh = await api.startTimeshift(current.url);
      // Zapped, or closed, while it started: that buffer isn't wanted.
      if (useApp.getState().stream?.url !== current.url) {
        api.releaseStream(fresh.url).catch(() => {});
        return;
      }
      holdPaused.current = true;
      useApp.setState({ stream: fresh });
    } catch (e) {
      api.log("warn", `live buffer couldn't start: ${errorText(e)}`).catch(() => {});
      if (useApp.getState().stream?.url === current.url) retuneOnPlay.current = true;
      useApp.getState().showToast("Pause and rewind isn't available on this channel.", "error");
    } finally {
      startingBuffer.current = false;
    }
  }, []);

  const goLive = useCallback(() => {
    const el = video.current;
    const range = el && bufferRange(el);
    if (!el || !range) return;
    el.currentTime = Math.max(range.start, range.end - 3);
    el.play().catch(() => {});
  }, []);

  const setZoom = (z: Zoom) => {
    setZoomState(z);
    try {
      localStorage.setItem("player.zoom", z);
    } catch {
      // Not remembered; the zoom still applies.
    }
  };

  // A new title starts at normal speed, with nothing skipped and no countdown.
  useEffect(() => {
    setSpeed(1);
    setCountdown(false);
    skipped.current = new Set();
    audioApplied.current = false;
    lastPosition.current = nowPlaying?.resumeFrom ?? 0;
  }, [nowPlaying]);

  // Attach each new stream, once what's remembered for its series is known.
  useEffect(() => {
    const el = video.current;
    if (!el || !stream || !tracks.ready) return;
    // The series' audio language, for a remuxed episode: it starts with that
    // track, rather than starting ffmpeg once and then again to switch.
    if (stream.mode === "remux" && !stream.isLive && !audioApplied.current) {
      audioApplied.current = true;
      const wanted = audioFor(tracks.choice, stream.audioTracks.map((t) => ({ id: t.index, language: t.language })), stream.audioIndex);
      if (wanted !== null) {
        const item = useApp.getState().nowPlaying;
        const resume = resumedFor.current !== item ? (item?.resumeFrom ?? 0) : 0;
        api
          .restream(stream.url, stream.startOffsetSeconds || (resume > 5 ? resume : 0), { audioIndex: wanted })
          .then((fresh) => {
            if (useApp.getState().stream === stream) useApp.setState({ stream: fresh });
            else api.releaseStream(fresh.url).catch(() => {});
          })
          // It plays as it is.
          .catch(() => useApp.getState().stream === stream && useApp.setState({ stream: { ...stream } }));
        return;
      }
    }
    let cancelled = false;
    retuneOnPlay.current = false;
    const holding = holdPaused.current;
    holdPaused.current = false;
    setBuffering(true);
    setOffset(stream.startOffsetSeconds);
    setTime({ current: 0, duration: 0 });
    api.log("info", `play mode=${stream.mode} live=${stream.isLive}`).catch(() => {});
    // What can't be got past goes to recovery.ts, which opens the stream again or says why it can't play.
    const report = (problem: Problem) => {
      if (cancelled) return;
      api.log("error", `engine: ${problem.message ?? (problem.ended ? "the stream ended" : "stalled")}`).catch(() => {});
      streamFailed(problem, stream.url, el.currentTime > 0 ? stream.startOffsetSeconds + el.currentTime : lastPosition.current);
    };
    attachStream(el, stream, (message) => report({ message }), report).then((handle) => {
      if (cancelled) handle.destroy();
      else {
        engine.current = handle;
        if (!holding && !isCasting()) el.play().catch((e: Error) => api.log("warn", `play() refused: ${e.name} ${e.message}`).catch(() => {}));
      }
    });
    // A stream that never starts (ffmpeg gave up, the server never sent
    // anything) would otherwise leave the spinner up for good.
    let started = false;
    const onPlaying = () => (started = true);
    el.addEventListener("playing", onPlaying);
    // Paused into a buffer: wait at its start, which is where the pause was.
    // WebKit knows the buffer's range a little after its metadata, and would
    // otherwise start near live when Play is pressed.
    const rangeEvents = ["loadedmetadata", "durationchange", "progress", "canplay"];
    const toStart = () => {
      const range = bufferRange(el);
      if (!range) return;
      el.currentTime = range.start;
      rangeEvents.forEach((name) => el.removeEventListener(name, toStart));
    };
    if (holding) rangeEvents.forEach((name) => el.addEventListener(name, toStart));
    // A live channel gets less time: there's no probing or remuxing to wait for.
    const watchdog = setTimeout(
      () => {
        if (cancelled || started || el.readyState >= el.HAVE_FUTURE_DATA) return;
        api.log("warn", `stream didn't start mode=${stream.mode}`).catch(() => {});
        report({ stalled: true, message: "This video didn't start." });
      },
      stream.isLive ? 25_000 : 45_000,
    );
    // Once it's played, a stream that stops arriving has dropped (a provider
    // can leave the connection open and send nothing): a live channel sooner,
    // since there's nothing to wait for.
    let stall: ReturnType<typeof setTimeout> | undefined;
    const onWaiting = () => {
      clearTimeout(stall);
      if (!started || el.paused) return;
      stall = setTimeout(
        () => {
          if (!cancelled && !el.paused && el.readyState < el.HAVE_FUTURE_DATA) report({ stalled: true, message: "Nothing more arrived from the stream." });
        },
        stream.isLive ? 12_000 : 30_000,
      );
    };
    const onMoving = () => clearTimeout(stall);
    el.addEventListener("waiting", onWaiting);
    el.addEventListener("playing", onMoving);
    el.addEventListener("pause", onMoving);
    return () => {
      cancelled = true;
      clearTimeout(watchdog);
      clearTimeout(stall);
      el.removeEventListener("playing", onPlaying);
      el.removeEventListener("waiting", onWaiting);
      el.removeEventListener("playing", onMoving);
      el.removeEventListener("pause", onMoving);
      rangeEvents.forEach((name) => el.removeEventListener(name, toStart));
      engine.current?.destroy();
      engine.current = null;
    };
    // `tracks.choice` is read once per stream, deliberately: what's chosen while it plays changes the stream itself.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stream, tracks.ready]);

  const duration = stream?.durationSeconds ?? nowPlaying?.durationHint ?? (Number.isFinite(time.duration) ? time.duration : 0);
  const position = offset + time.current;

  const saveProgress = useCallback(
    (finishedHint = false) => {
      const item = useApp.getState().nowPlaying;
      if (!item || isLive || !duration || isCasting() || (item.kind !== "movie" && item.kind !== "episode" && item.kind !== "recording")) return;
      const finished = finishedHint || watchedEnough(position, duration);
      api
        .saveProgress({ itemId: item.itemId, kind: item.kind, sourceId: item.sourceId, seriesId: item.seriesId ?? null, positionSeconds: position, durationSeconds: duration, finished, updatedAt: nowSeconds() })
        .catch(() => {});
    },
    [isLive, duration, position],
  );

  // The interval reads the latest saver through a ref; depending on
  // `saveProgress` itself would restart it on every time update.
  const saveRef = useRef(saveProgress);
  saveRef.current = saveProgress;
  useEffect(() => {
    if (!stream || isLive) return;
    const t = setInterval(() => saveRef.current(), 10000);
    return () => clearInterval(t);
  }, [stream, isLive]);
  useEffect(() => {
    const save = () => saveRef.current();
    window.addEventListener(SAVE_PROGRESS_EVENT, save);
    return () => window.removeEventListener(SAVE_PROGRESS_EVENT, save);
  }, []);

  const seek = useCallback(
    async (target: number) => {
      const el = video.current;
      if (!el || !stream) return;
      if (isLive) {
        // A buffered channel seeks within what's kept; a direct one can't.
        const range = stream.timeshift ? bufferRange(el) : null;
        if (range) el.currentTime = Math.min(Math.max(target, range.start), range.end - 1);
        return;
      }
      const clamped = Math.max(0, duration ? Math.min(duration - 2, target) : target);
      if (stream.mode === "remux") {
        try {
          const next = await api.seekStream(stream.url, clamped);
          useApp.setState({ stream: next });
        } catch (e) {
          // The stream plays on where it was.
          useApp.getState().showToast(errorText(e), "error");
        }
      } else el.currentTime = clamped;
    },
    [stream, isLive, duration],
  );

  /** The same title restarted where it is, with another audio track or the audio moved. */
  const restream = useCallback(
    async (options: StreamOptions) => {
      if (!stream) return;
      try {
        const fresh = await api.restream(stream.url, isLive ? 0 : position, options);
        useApp.setState({ stream: fresh });
      } catch (e) {
        useApp.getState().showToast(errorText(e), "error");
      }
    },
    [stream, isLive, position],
  );

  const close = useCallback(() => {
    saveProgress();
    leavePlayerFullscreen();
    useApp.getState().stop();
  }, [saveProgress]);

  const playNext = useCallback(() => {
    const playing = useApp.getState().nowPlaying;
    setCountdown(false);
    if (!next || !playing) return;
    saveProgress(true);
    playNextEpisode(next, playing);
  }, [next, saveProgress]);

  /** Another copy of the add-on film or episode playing, from where it is; pauses while the list is open. */
  const switchCopy = useCallback(() => {
    const item = useApp.getState().nowPlaying;
    const el = video.current;
    const playingNow = !!el && !el.paused && !!useApp.getState().stream;
    const request = item && switchRequest(item, Math.max(0, lastPosition.current), playingNow);
    if (!request) return;
    if (playingNow) el?.pause();
    useApp.getState().showCopies(request);
  }, []);

  // A Chromecast or Google TV, when one has the title.
  const cast = useCasting({ video, nowPlaying, stream, isLive, position, onFinished: () => (next && settings.autoplayNextEpisode ? playNext() : close()) });

  // The media keys and Now Playing. Next and previous change channel, or go to the next episode.
  const zaps = isLive && !!nowPlaying?.channel;
  useMediaSession({
    item: nowPlaying,
    video,
    isLive,
    paused,
    position,
    duration,
    speed,
    onSeek: seek,
    onNext: zaps ? channelUp : next ? playNext : null,
    onPrevious: zaps ? channelDown : null,
    ...cast.mediaSession,
  });

  // What the menu bar and other windows' keys reach (controls.ts): the latest of each, through a ref.
  const handlers = useRef({ seek, position, switchCopy });
  handlers.current = { seek, position, switchCopy };
  const mounted = !!nowPlaying;
  useEffect(() => {
    if (!mounted) return;
    registerPlayer(video.current, {
      skip: (seconds) => handlers.current.seek(handlers.current.position + seconds),
      switchCopy: () => handlers.current.switchCopy(),
      toggleFullscreen: () => {
        togglePlayerFullscreen();
      },
      position: () => handlers.current.position,
    });
    return () => registerPlayer(null, null);
  }, [mounted]);

  useTraktScrobble(nowPlaying, video, position, duration);

  /**
   * Hands what's playing to another player. A film or episode pauses here
   * and starts there where it was (in players that take a start time); a
   * channel stops here, since most logins allow one connection at a time.
   */
  const openIn = useCallback(
    async (player: ExternalPlayer) => {
      const item = useApp.getState().nowPlaying;
      if (!stream || !item) return;
      if (!(await openInPlayer(player, { streamUrl: stream.url }, externalTitle(item), isLive ? null : position))) return;
      if (isLive) close();
      else video.current?.pause();
    },
    [stream, isLive, position, close],
  );

  // Skipping intros and recaps by themselves, once each, when that's chosen.
  const inSegment = (name: "intro" | "recap") => {
    const part = segments?.[name];
    return part && position >= part.startSeconds && position < part.endSeconds - 1 ? part : null;
  };
  const skippable = !isLive && (inSegment("recap") ? "recap" : inSegment("intro") ? "intro" : null);
  useEffect(() => {
    if (!skippable || !settings.skipIntrosAutomatically || skipped.current.has(skippable)) return;
    skipped.current.add(skippable);
    seek(segments![skippable]!.endSeconds);
  }, [skippable, settings.skipIntrosAutomatically, segments, seek]);

  // Seek-bar thumbnails, and a recording's ad breaks: a Skip Ad button in
  // each, or skipped by themselves once each, as Settings › Recording says.
  useSeekExtrasLoader(nowPlaying, stream);
  const breaks = useSeekExtras((s) => s.breaks);
  const adBreak = isLive ? null : adBreakAt(breaks, position);
  useEffect(() => {
    if (!adBreak || settings.commercialSkip !== "automatic") return;
    const key = `ad ${adBreak.startSeconds}`;
    if (skipped.current.has(key)) return;
    skipped.current.add(key);
    seek(adBreak.endSeconds);
    useApp.getState().showToast("Skipped an ad break");
  }, [adBreak, settings.commercialSkip, seek]);

  // Next Episode shows from the end credits, or the last 40 seconds when they aren't timed.
  const nearEnd = !!duration && (segments?.credits ? position >= segments.credits.startSeconds : duration - position <= 40);
  const offerNext = !!next && !isLive && nearEnd && !countdown;

  /**
   * Into a window of its own, or back into the main window from it: the
   * stream stops here first (one provider connection at a time), and the
   * other window plays on from the same moment.
   */
  const moveWindow = useCallback(async () => {
    const { nowPlaying: item, stream: playing, settings: current } = useApp.getState();
    if (!item) return;
    saveProgress();
    leavePlayerFullscreen();
    const handoff = { item: { ...item, resumeFrom: undefined }, position: isLive ? 0 : position };
    try {
      if (isPopOutWindow) {
        if (playing) await api.releaseStream(playing.url).catch(() => {});
        await api.dock(handoff);
      } else {
        useApp.getState().stop();
        useApp.setState({ poppedOut: true });
        await api.popOut(handoff, current.keepPopOutOnTop);
      }
    } catch (e) {
      useApp.setState({ poppedOut: false });
      useApp.getState().showToast(errorText(e), "error");
    }
  }, [saveProgress, isLive, position]);

  const back = useCallback(() => {
    if (isPopOutWindow) {
      moveWindow();
      return;
    }
    const { tab, nowPlaying: item, setPresentation } = useApp.getState();
    leavePlayerFullscreen();
    if (item?.channel && tab === "live") setPresentation("docked");
    else if (item?.channel) setPresentation("mini");
    else close();
  }, [close, moveWindow]);

  // The menu bar's Playback items, done as the player's own buttons and keys
  // do them. Multiview, while it's open, has the play, skip, Go Live and
  // channel items, for the tile heard, and Pause All.
  const playback = useRef({ position, seekable: false, seek, goLive, close, moveWindow, back });
  playback.current = { position, seekable: !!stream && (!isLive || buffered), seek, goLive, close, moveWindow, back };
  useEffect(() => {
    const multiview = () => useMultiview.getState().open;
    const ifPlaying = (work: () => void) => () => useApp.getState().nowPlaying && work();
    const heard = (work: (id: string) => void) => {
      const tile = heardTile();
      if (tile) work(tile.id);
    };
    const skip = (by: number) => {
      if (multiview()) return heard((id) => skipTile(id, by));
      if (useApp.getState().nowPlaying && playback.current.seekable) playback.current.seek(playback.current.position + by);
    };
    return handleCommands({
      "playback.playPause": () => {
        if (multiview()) return toggleHeardPaused();
        const el = video.current;
        if (!el || !useApp.getState().nowPlaying) return;
        if (el.paused) el.play().catch(() => {});
        else pauseByPerson(el);
      },
      "playback.pauseAll": () => multiview() && toggleAllPaused(),
      "playback.stop": ifPlaying(() => playback.current.close()),
      "playback.skipBack": () => skip(-10),
      "playback.skipForward": () => skip(10),
      "playback.goLive": () => (multiview() ? heard(goLiveTile) : useApp.getState().nowPlaying && playback.current.goLive()),
      // Next is channel up, as ↑ is in the player and on a remote.
      "playback.previousChannel": () => (multiview() ? listenNext(-1) : channelDown()),
      "playback.nextChannel": () => (multiview() ? listenNext(1) : channelUp()),
      "playback.lastChannel": () => !multiview() && playLastChannel(),
      "playback.mute": ifPlaying(() => {
        if (video.current) video.current.muted = !video.current.muted;
      }),
      "playback.popOut": ifPlaying(() => isTauri && playback.current.moveWindow()),
      "player.back": () => playback.current.back(),
    });
  }, []);

  // What the Playback menu can do now. A buffered channel counts as behind live a little way from its edge.
  const range = buffered && video.current ? bufferRange(video.current) : null;
  const behindLive = !!range && range.end - time.current > LIVE_EDGE;
  useEffect(() => {
    reportPlayback({ playing: !!nowPlaying, paused, muted, seekable: !!stream && (!isLive || buffered), behindLive, channel: isLive && !!nowPlaying?.channel });
  }, [nowPlaying, stream, paused, muted, isLive, buffered, behindLive]);

  const listShown = useLiveOverlays((s) => s.list);
  const nowNextShown = useLiveOverlays((s) => s.nowNext);

  // Full screen, the keys are the player's: whatever had focus behind it
  // (Live TV's guide, which moves and plays on the arrows and Return) lets go.
  useEffect(() => {
    const focused = document.activeElement;
    if (presentation === "full" && focused instanceof HTMLElement && focused !== document.body && !root.current?.contains(focused)) focused.blur();
  }, [presentation]);

  if (!nowPlaying) return null;

  const style: React.CSSProperties =
    presentation === "docked" && dockRect ? { left: dockRect.left, top: dockRect.top, width: dockRect.width, height: dockRect.height } : presentation === "mini" ? (mini.style ?? {}) : {};
  const compact = presentation !== "full";
  // A play that failed before a stream was given is put into words like any other failure.
  const failure = failed ?? (playError ? classifyFailure({ message: playError }, { live: isLive, online: navigator.onLine, hasPlayed: titleHasPlayed() }) : null);
  const blocked = !!failure || !!reconnecting;
  const canSwitchCopy = nowPlaying.sourceId === "addon" && (nowPlaying.kind === "movie" || nowPlaying.kind === "episode");
  const openSources =
    isPopOutWindow || nowPlaying.kind === "recording"
      ? null
      : () => {
          leavePlayerFullscreen();
          const { setPresentation, go } = useApp.getState();
          // The video waits in the corner, so Try Again is at hand once the source is put right.
          setPresentation("mini");
          go("sources");
        };
  const liveChannel = nowPlaying.kind === "channel" ? (nowPlaying.channel ?? null) : null;

  return (
    <div
      ref={root}
      className={`player ${presentation} subs-${settings.subtitleSize} ${presentation === "mini" ? `corner-${mini.corner} ${mini.dragging ? "dragging" : ""}` : ""}`}
      style={style}
      onPointerDown={presentation === "mini" ? mini.onPointerDown : undefined}
    >
      <video
        ref={video}
        className="player-video"
        style={{ objectFit: zoomFit[zoom] }}
        playsInline
        onWaiting={() => setBuffering(true)}
        onPlaying={() => {
          setBuffering(false);
          setPaused(false);
          streamPlaying();
        }}
        onCanPlay={() => setBuffering(false)}
        onVolumeChange={(e) => setMuted(e.currentTarget.muted)}
        onPause={() => {
          setPaused(true);
          saveProgress();
          if (takePausedByPerson() && isLive && !stream?.timeshift) startBuffer();
        }}
        onPlay={(e) => {
          setPaused(false);
          const { nowPlaying: item, presentation: shown, play } = useApp.getState();
          if (retuneOnPlay.current) {
            retuneOnPlay.current = false;
            if (item) play(item, shown);
            return;
          }
          // Paused for longer than the buffer keeps: carry on from the oldest part still kept.
          const range = stream?.timeshift ? bufferRange(e.currentTarget) : null;
          if (range && e.currentTarget.currentTime < range.start) e.currentTarget.currentTime = range.start + 1;
        }}
        onEnded={() => {
          // A live stream never ends, and a film that ends minutes early lost its connection.
          if (stream && (isLive || (duration > 0 && titleHasPlayed() && position < duration - 120))) {
            streamFailed({ ended: true, message: "The stream ended early." }, stream.url, position);
            return;
          }
          saveProgress(true);
          if (next && settings.autoplayNextEpisode) setCountdown(true);
          else if (!isLive) close();
        }}
        onTimeUpdate={(e) => {
          const el = e.currentTarget;
          setTime({ current: el.currentTime, duration: el.duration });
          if (stream && el.currentTime > 0) lastPosition.current = stream.startOffsetSeconds + el.currentTime;
        }}
        onLoadedMetadata={(e) => {
          const el = e.currentTarget;
          el.playbackRate = isLive ? 1 : speed;
          // The series' audio language, for a stream whose tracks the web view switches itself.
          if (!audioApplied.current && stream?.mode !== "remux") {
            audioApplied.current = true;
            const wanted = audioFor(tracks.choice, engine.current?.audioTracks() ?? [], null);
            if (wanted !== null) engine.current?.setAudioTrack(wanted);
          }
          // Opened again after a drop: where it dropped.
          const again = takeResumeSeek();
          if (again !== null && stream?.mode !== "remux") el.currentTime = again;
          if (resumedFor.current === nowPlaying) return;
          resumedFor.current = nowPlaying;
          const resumeFrom = nowPlaying.resumeFrom ?? 0;
          if (again !== null) return;
          if (resumeFrom > 5 && stream?.mode !== "remux") el.currentTime = resumeFrom;
          else if (resumeFrom > 5 && stream?.mode === "remux" && stream.startOffsetSeconds === 0) seek(resumeFrom);
        }}
        onError={(e) => {
          const el = e.currentTarget;
          api.log("error", `video error ${el.error?.code} ${el.error?.message ?? ""} mode=${stream?.mode} network=${el.networkState}`).catch(() => {});
          if (stream) streamFailed({ mediaError: el.error?.code ?? null, message: el.error?.message || null }, stream.url, el.currentTime > 0 ? stream.startOffsetSeconds + el.currentTime : lastPosition.current);
        }}
        onDoubleClick={() => presentation === "full" && togglePlayerFullscreen()}
        onClick={() => {
          // The end of dragging the mini player isn't a click.
          if (presentation === "mini" && mini.takeDragged()) return;
          if (presentation !== "full") useApp.getState().setPresentation("full");
        }}
      />
      {(buffering || !stream) && !blocked && (
        <div className="player-center">
          <div className="spinner" />
        </div>
      )}
      {reconnecting && !failure && <ReconnectingCard attempt={reconnecting.attempt} of={reconnecting.of} nextAt={reconnecting.nextAt} compact={compact} />}
      {failure && (
        <FailureCard
          failure={failure}
          title={nowPlaying.title}
          compact={compact}
          onRetry={retry}
          onClose={close}
          onSources={openSources}
          onChooseCopy={canSwitchCopy ? switchCopy : null}
        />
      )}
      {presentation === "full" && !cast.session && (
        <FullControls
          video={video}
          isLive={isLive}
          paused={paused}
          position={position}
          duration={duration}
          onSeek={seek}
          onBack={back}
          engine={engine}
          buffered={buffered}
          onGoLive={goLive}
          stream={stream}
          subtitles={subtitles}
          onAudioTrack={(index) => restream({ audioIndex: index })}
          onBurnSubtitle={(index) => restream({ burnSubtitle: index })}
          onMoveWindow={moveWindow}
          speed={speed}
          onSpeed={(rate) => {
            setSpeed(rate);
            if (video.current) video.current.playbackRate = rate;
          }}
          zoom={zoom}
          onZoom={setZoom}
          onPanel={(p) => setPanel((open) => (open === p ? null : p))}
          // AirPlay hands the receiver an address to play; a Media Source stream (mpegts.js) has none.
          airplay={airplay.active || (airplay.available && (stream?.mode === "native" || stream?.mode === "hls")) ? { active: airplay.active } : null}
          onOpenIn={openIn}
          cast={cast}
          onSwitchCopy={canSwitchCopy ? switchCopy : null}
          remember={tracks.remember}
        />
      )}
      {presentation === "full" && !cast.session && liveChannel && (
        <>
          {listShown && <ChannelListOverlay />}
          {nowNextShown && !listShown && <NowNextCard channel={liveChannel} />}
          <ChannelNumberEntry />
        </>
      )}
      {presentation === "full" && !blocked && adBreak && settings.commercialSkip === "button" && <CornerButton label="Skip Ad" onClick={() => seek(adBreak.endSeconds)} />}
      {presentation === "full" && !blocked && skippable && !settings.skipIntrosAutomatically && (
        <CornerButton label={skippable === "intro" ? "Skip Intro" : "Skip Recap"} onClick={() => segments?.[skippable] && seek(segments[skippable]!.endSeconds)} />
      )}
      {presentation === "full" && !blocked && !skippable && offerNext && <CornerButton label="Next Episode" onClick={playNext} />}
      {countdown && next && <NextCountdown next={next} onPlay={playNext} onCancel={close} />}
      {presentation === "full" && stream && panel === "sync" && (
        <SyncPanel stream={stream} subtitleDelay={subtitles.delay} onSubtitleDelay={subtitles.setDelay} onAudioDelay={(d) => restream({ audioDelaySeconds: d })} onClose={() => setPanel(null)} />
      )}
      {presentation === "full" && stream && panel === "info" && <StreamInfoPanel stream={stream} video={video} onClose={() => setPanel(null)} />}
      {presentation === "mini" && <MiniStrip video={video} paused={paused} onClose={close} onPopOut={moveWindow} />}
      {cast.session && <CastPanel casting={cast} presentation={presentation} onBack={back} onClose={close} />}
      {presentation === "docked" && !cast.session && <DockedControls video={video} paused={paused} onClose={close} onPopOut={moveWindow} />}
    </div>
  );
}

// MARK: Full-screen controls

interface FullControlsProps {
  video: React.RefObject<HTMLVideoElement | null>;
  isLive: boolean;
  paused: boolean;
  position: number;
  duration: number;
  onSeek: (t: number) => void;
  onBack: () => void;
  engine: React.RefObject<EngineHandle | null>;
  /** A live channel playing from its buffer, which can pause and seek. */
  buffered: boolean;
  onGoLive: () => void;
  stream: ResolvedStream | null;
  subtitles: ReturnType<typeof useSubtitles>;
  onAudioTrack: (index: number) => void;
  /** Draws a picture subtitle track into the video (-1 stops), restarting the stream. */
  onBurnSubtitle: (index: number) => void;
  /** Into the pop-out player's window, or back into the main window from it. */
  onMoveWindow: () => void;
  speed: number;
  onSpeed: (rate: number) => void;
  zoom: Zoom;
  onZoom: (zoom: Zoom) => void;
  onPanel: (panel: "sync" | "info") => void;
  /** Shown when there are AirPlay devices and the stream can go to one. */
  airplay: { active: boolean } | null;
  onOpenIn: (player: ExternalPlayer) => void;
  /** Chromecast and Google TV: the button shows once one is found. */
  cast: Casting;
  /** Another copy of an add-on title (C); null for anything else. */
  onSwitchCopy: (() => void) | null;
  /** Audio and subtitles picked here are remembered for the series an episode belongs to. */
  remember: TrackMemory["remember"];
}

const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2];

/** "English · Dolby Digital Plus 5.1 · Commentary" */
function audioLabel(track: MediaTrack) {
  const codec: Record<string, string> = { ac3: "Dolby Digital", eac3: "Dolby Digital Plus", truehd: "Dolby TrueHD", dts: "DTS", aac: "AAC", mp3: "MP3", opus: "Opus", flac: "FLAC" };
  const layout = track.channels === 6 ? "5.1" : track.channels === 8 ? "7.1" : track.channels === 2 ? "Stereo" : track.channels === 1 ? "Mono" : null;
  return [languageName(track.language) ?? "Unknown language", [codec[track.codec] ?? track.codec.toUpperCase(), layout].filter(Boolean).join(" "), track.title].filter(Boolean).join(" · ");
}
const ticked = (on: boolean) => (on ? <Check size={15} strokeWidth={2} /> : <span style={{ width: 15 }} />);

function FullControls({ video, isLive, paused, position, duration, onSeek, onBack, engine, buffered, onGoLive, stream, subtitles, onAudioTrack, onBurnSubtitle, onMoveWindow, speed, onSpeed, zoom, onZoom, onPanel, airplay, onOpenIn, cast, onSwitchCopy, remember }: FullControlsProps) {
  const nowPlaying = useApp((s) => s.nowPlaying)!;
  const keepOnTop = useApp((s) => s.settings.keepPopOutOnTop);
  const listOpen = useLiveOverlays((s) => s.list);
  const nowNextShown = useLiveOverlays((s) => s.nowNext);
  const [visible, setVisible] = useState(true);
  const [volume, setVolume] = useState(() => video.current?.volume ?? 1);
  const [muted, setMuted] = useState(() => video.current?.muted ?? false);
  const [fullscreen, setFullscreen] = useState(false);
  const [scrub, setScrub] = useState<number | null>(null);
  const hideTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const liveChannel = nowPlaying.kind === "channel" ? (nowPlaying.channel ?? null) : null;

  const poke = useCallback(() => {
    setVisible(true);
    clearTimeout(hideTimer.current);
    hideTimer.current = setTimeout(() => setVisible(false), 3200);
  }, []);

  useEffect(() => {
    poke();
    isWindowFullscreen().then(setFullscreen);
    // Full screen and the volume can change from the menu bar or the docked player's controls too.
    const onResize = () => isWindowFullscreen().then(setFullscreen);
    window.addEventListener("resize", onResize);
    const el = video.current;
    const onVolume = () => {
      if (!el) return;
      setVolume(el.volume);
      setMuted(el.muted);
    };
    el?.addEventListener("volumechange", onVolume);
    return () => {
      clearTimeout(hideTimer.current);
      window.removeEventListener("resize", onResize);
      el?.removeEventListener("volumechange", onVolume);
    };
  }, [poke, video]);

  const togglePlay = useCallback(() => {
    const el = video.current;
    if (!el) return;
    if (el.paused) el.play().catch(() => {});
    else pauseByPerson(el);
  }, [video]);

  const toggleFullscreen = useCallback(async () => {
    setFullscreen(await togglePlayerFullscreen());
  }, []);

  // A change made from the menu bar (Mute) shows on the controls too.
  useEffect(() => {
    const el = video.current;
    if (!el) return;
    const onVolume = () => {
      setVolume(el.volume);
      setMuted(el.muted);
    };
    el.addEventListener("volumechange", onVolume);
    return () => el.removeEventListener("volumechange", onVolume);
  }, [video]);

  // Keys work like a TV remote. On a live channel: up and down change
  // channel (⌥↑ and ⌥↓ are the volume), digits jump to a channel, Return
  // opens the channel list and G the Now/Next card. Everywhere: Space or K,
  // ← and → (⇧ for a minute), M, F, I, P, C for another copy, Esc.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target;
      // Typing in a field, or a sheet or menu over the player (Multiview's channel picker, the copies list), has the keys.
      if ((target instanceof HTMLInputElement && target.type !== "range") || target instanceof HTMLTextAreaElement || document.querySelector(".sheet-backdrop, .menu")) return;
      // The menu bar's shortcuts (⌥Space, ⌘←, ⌘↑, ⌥⌘P, ⌘[…) are its own, done through it.
      if (isMenuShortcut(e)) return;
      const plain = !e.metaKey && !e.ctrlKey && !e.altKey;
      const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
      const overlays = useLiveOverlays.getState();
      const seekable = !isLive || buffered;
      poke();
      let handled = true;
      // The channel list has the arrows, Return and Esc while it's open.
      if (overlays.list && plain && ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "PageUp", "PageDown", "Enter", "Escape"].includes(key)) {
        if (key === "ArrowUp") moveListHighlight(-1);
        else if (key === "ArrowDown") moveListHighlight(1);
        else if (key === "PageUp") moveListHighlight(-10);
        else if (key === "PageDown") moveListHighlight(10);
        else if (key === "ArrowLeft") stepListGroup(-1);
        else if (key === "ArrowRight") stepListGroup(1);
        else if (key === "Enter") watchHighlighted();
        else closeChannelList();
      } else if (liveChannel && plain && /^[0-9]$/.test(key)) typeChannelDigit(key);
      else if (liveChannel && plain && key === "Enter") toggleChannelList();
      else if (key === " " || (plain && key === "k")) togglePlay();
      else if ((key === "ArrowLeft" || key === "ArrowRight") && !e.metaKey && !e.altKey && seekable) onSeek(position + (key === "ArrowLeft" ? -1 : 1) * (e.shiftKey ? 60 : 10));
      else if ((key === "ArrowUp" || key === "ArrowDown") && !e.metaKey && (e.altKey || !liveChannel)) changeVolume(key === "ArrowUp" ? 0.1 : -0.1);
      else if (liveChannel && !e.altKey && (key === "ArrowUp" || key === "PageUp")) channelUp();
      else if (liveChannel && !e.altKey && (key === "ArrowDown" || key === "PageDown")) channelDown();
      else if (plain && key === "m") toggleMute();
      else if (plain && key === "f") toggleFullscreen();
      else if (plain && key === "i") onPanel("info");
      else if (plain && key === "p") togglePictureInPicture();
      // Something else in a corner of this, in the main window's Multiview.
      else if (plain && key === INSET_KEY && !isPopOutWindow) insetCommand();
      else if (plain && key === "g" && liveChannel) toggleNowNext();
      else if (plain && key === "c" && onSwitchCopy) onSwitchCopy();
      // In the main window L is the whole window's (App.tsx); the pop-out has only the player.
      else if (plain && key === "l" && isPopOutWindow && liveChannel) playLastChannel();
      else if (e.metaKey && e.altKey && e.code === "KeyP" && isTauri) onMoveWindow();
      else if (key === "Escape") {
        // One step back at a time: what's over the video, then full screen, then out of the player.
        if (overlays.digits) cancelChannelDigits();
        else if (overlays.nowNext) closeNowNext();
        else if (madeFullscreen()) toggleFullscreen();
        // The pop-out window docks only when asked to.
        else if (!isPopOutWindow) onBack();
      } else if (e.metaKey && key === "[") onBack();
      else handled = false;
      if (handled) e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [poke, togglePlay, toggleFullscreen, onSeek, onBack, onPanel, onMoveWindow, onSwitchCopy, position, isLive, buffered, liveChannel]);

  const shown = visible || paused || scrub != null || listOpen;
  const VolumeIcon = muted || volume === 0 ? VolumeX : volume < 0.5 ? Volume1 : Volume2;
  const shownPosition = scrub ?? position;

  return (
    <div className={`player-chrome ${shown ? "shown" : ""}`} onMouseMove={poke} onClick={(e) => e.target === e.currentTarget && togglePlay()}>
      <div className="player-top">
        <button className="player-round glass" onClick={onBack} aria-label="Back">
          <ArrowLeft size={18} />
        </button>
        <div className="player-titles">
          <div className="player-title">{nowPlaying.title}</div>
          {nowPlaying.subtitle && <div className="player-subtitle">{nowPlaying.subtitle}</div>}
        </div>
        <div style={{ flex: 1 }} />
        {liveChannel && (
          <>
            <button className={`player-icon ${listOpen ? "on" : ""}`} onClick={() => toggleChannelList()} aria-label="Channel list" title="Channel list (Return)">
              <ListVideo size={19} />
            </button>
            <button className={`player-icon ${nowNextShown ? "on" : ""}`} onClick={toggleNowNext} aria-label="Now and next" title="Now and next (G)">
              <CalendarClock size={19} />
            </button>
          </>
        )}
        {nowPlaying.channel && <LiveRecordButton channel={nowPlaying.channel} />}
      </div>

      {/* The channel list and the Now/Next card say as much, where it sits. */}
      {nowPlaying.channel && !listOpen && !nowNextShown && <ChannelBar channel={nowPlaying.channel} />}

      <div className="player-bar glass" onMouseDown={(e) => e.stopPropagation()}>
        {buffered ? (
          <BufferTimeline video={video} onSeek={onSeek} onGoLive={onGoLive} />
        ) : isLive ? (
          <div className="player-timeline live">
            <span className="live-pill">LIVE</span>
          </div>
        ) : (
          <div className="player-timeline">
            <span className="clock">{playerClock(shownPosition)}</span>
            <SeekTrack duration={duration} scrub={scrub}>
              <input
                type="range"
                className="scrubber"
                min={0}
                max={duration || 1}
                step={1}
                value={Math.min(shownPosition, duration || 1)}
                style={{ "--fill": `${duration ? (shownPosition / duration) * 100 : 0}%` } as React.CSSProperties}
                onChange={(e) => setScrub(Number(e.target.value))}
                onMouseUp={() => {
                  if (scrub != null) onSeek(scrub);
                  setScrub(null);
                }}
                onKeyUp={() => {
                  if (scrub != null) onSeek(scrub);
                  setScrub(null);
                }}
                aria-label="Position"
              />
            </SeekTrack>
            <span className="clock">−{playerClock(Math.max(0, duration - shownPosition))}</span>
          </div>
        )}
        <div className="player-buttons">
          {(!isLive || buffered) && (
            <button className="player-icon" onClick={() => onSeek(position - 10)} aria-label="Back 10 seconds">
              <RotateCcw size={19} />
            </button>
          )}
          <button className="player-icon big" onClick={togglePlay} aria-label={paused ? "Play" : "Pause"}>
            {paused ? <Play size={22} fill="currentColor" strokeWidth={0} /> : <Pause size={22} fill="currentColor" strokeWidth={0} />}
          </button>
          {(!isLive || buffered) && (
            <button className="player-icon" onClick={() => onSeek(position + 10)} aria-label="Forward 10 seconds">
              <RotateCw size={19} />
            </button>
          )}
          <div style={{ flex: 1 }} />
          <button
            className="player-icon"
            onClick={() => {
              const el = video.current;
              if (!el) return;
              el.muted = !el.muted;
              setMuted(el.muted);
            }}
            aria-label="Mute"
          >
            <VolumeIcon size={19} />
          </button>
          <input
            type="range"
            className="volume"
            min={0}
            max={1}
            step={0.01}
            value={muted ? 0 : volume}
            style={{ "--fill": `${(muted ? 0 : volume) * 100}%` } as React.CSSProperties}
            onChange={(e) => {
              const el = video.current;
              if (!el) return;
              el.volume = Number(e.target.value);
              el.muted = false;
              setVolume(el.volume);
              setMuted(false);
            }}
            aria-label="Volume"
          />
          <button
            className="player-icon"
            aria-label="Audio"
            onClick={(e) => {
              // A remuxed stream's tracks switch by restarting it; others are the web view's own.
              const own = stream?.audioTracks ?? [];
              // The language picked is remembered for the series an episode belongs to.
              const items: MenuEntry[] =
                own.length > 1
                  ? own.map((t) => ({
                      label: audioLabel(t),
                      icon: ticked(t.index === stream?.audioIndex),
                      onSelect: () => {
                        remember((c) => withAudio(c, t.language));
                        if (t.index !== stream?.audioIndex) onAudioTrack(t.index);
                      },
                    }))
                  : (engine.current?.audioTracks() ?? []).map((t) => ({
                      label: t.label,
                      onSelect: () => {
                        remember((c) => withAudio(c, t.language));
                        engine.current?.setAudioTrack(t.id);
                      },
                    }));
              openMenu(e, items.length ? [{ heading: "Audio" }, ...items] : [{ heading: "This stream has one audio track" }], true);
            }}
          >
            <AudioLines size={19} />
          </button>
          <button
            className="player-icon"
            aria-label="Subtitles"
            onClick={(e) => {
              const { options, choice, choose, burnFor, off } = subtitles;
              // Picture subtitles are drawn into the video, which restarts it.
              // What's picked (Off too) is remembered for the series.
              const pick = (key: string) => {
                const burn = burnFor(key);
                if (burn !== undefined) onBurnSubtitle(burn);
                choose(key);
                const option = options.find((o) => o.key === key);
                if (key === off || option) remember((c) => withSubtitles(c, option ?? null));
              };
              openMenu(
                e,
                [
                  { heading: "Subtitles" },
                  { label: "Off", icon: ticked(choice === off), onSelect: () => pick(off) },
                  ...options.map((o) => ({ label: o.detail ? `${o.label} · ${o.detail}` : o.label, icon: ticked(choice === o.key), onSelect: () => pick(o.key) })),
                ],
                true,
              );
            }}
          >
            <Captions size={19} />
          </button>
          <button
            className="player-icon"
            aria-label="Playback options"
            onClick={(e) =>
              openMenu(
                e,
                [
                  ...(isLive ? [] : [{ heading: "Speed" } as MenuEntry, ...SPEEDS.map((rate) => ({ label: rate === 1 ? "Normal" : `${rate}×`, icon: ticked(speed === rate), onSelect: () => onSpeed(rate) }))]),
                  { heading: "Picture" },
                  { label: "Fit", icon: ticked(zoom === "fit"), onSelect: () => onZoom("fit") },
                  { label: "Fill (crop the edges)", icon: ticked(zoom === "fill"), onSelect: () => onZoom("fill") },
                  { label: "Stretch", icon: ticked(zoom === "stretch"), onSelect: () => onZoom("stretch") },
                  { separator: true },
                  { label: "Sync Audio & Subtitles…", onSelect: () => onPanel("sync") },
                  { label: "Stream Information", shortcut: "I", onSelect: () => onPanel("info") },
                  ...(isPopOutWindow
                    ? [
                        { separator: true } as MenuEntry,
                        {
                          label: "Keep on Top",
                          icon: ticked(keepOnTop),
                          onSelect: () => {
                            useApp.getState().updateSettings({ keepPopOutOnTop: !keepOnTop });
                            api.setPopOutOnTop(!keepOnTop).catch(() => {});
                          },
                        },
                      ]
                    : []),
                  ...(stream ? externalPlayers() : []).flatMap((player, i): MenuEntry[] => [
                    ...(i === 0 ? [{ separator: true } as MenuEntry] : []),
                    { label: `Open in ${player.name}`, icon: <ExternalLink size={15} strokeWidth={2} />, onSelect: () => onOpenIn(player) },
                  ]),
                ],
                true,
              )
            }
          >
            <SlidersHorizontal size={19} />
          </button>
          {airplay && (
            <button
              className={`player-icon ${airplay.active ? "on" : ""}`}
              aria-label="AirPlay"
              onClick={() => (video.current as AirPlayVideo | null)?.webkitShowPlaybackTargetPicker?.()}
            >
              <Airplay size={19} />
            </button>
          )}
          <CastButton casting={cast} />
          {onSwitchCopy && (
            <button className="player-icon" aria-label="Switch to another copy" title="Switch to another copy (C)" onClick={onSwitchCopy}>
              <Layers size={19} />
            </button>
          )}
          <button className="player-icon" aria-label="Picture in Picture" title="Picture in Picture (P)" onClick={togglePictureInPicture}>
            <PictureInPicture2 size={19} />
          </button>
          {!isPopOutWindow && (
            <button className="player-icon" aria-label="Add an inset" title={`Watch something else in a corner of this (${INSET_KEY.toUpperCase()})`} onClick={insetCommand}>
              <PanelsRightBottom size={19} />
            </button>
          )}
          {!isPopOutWindow && (
            <button className="player-icon" aria-label="Multiview" title={`Watch something else beside this (${shortcutLabel("playback.multiview")})`} onClick={toggleMultiview}>
              <Grid2x2Plus size={19} />
            </button>
          )}
          {isTauri && (
            <button
              className="player-icon"
              aria-label={isPopOutWindow ? "Return video to the main window" : "Pop out video"}
              title={`${isPopOutWindow ? "Return to the main window" : "Pop out into its own window"} (${shortcutLabel("playback.popOut")})`}
              onClick={onMoveWindow}
            >
              {isPopOutWindow ? <ArrowDownLeftFromSquare size={19} /> : <SquareArrowOutUpRight size={19} />}
            </button>
          )}
          {!isPopOutWindow && (
            <button
              className="player-icon"
              aria-label="Mini player"
              onClick={() => {
                leavePlayerFullscreen();
                useApp.getState().setPresentation(useApp.getState().tab === "live" && nowPlaying.channel ? "docked" : "mini");
              }}
            >
              <Shrink size={19} />
            </button>
          )}
          <button className="player-icon" onClick={toggleFullscreen} aria-label="Full screen">
            {fullscreen ? <Minimize size={19} /> : <Maximize size={19} />}
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * Now and next on the playing channel, with channel up and down (zap.ts,
 * as the keys do: up is the next channel in the list, as on a remote).
 */
function ChannelBar({ channel }: { channel: Channel }) {
  const [programs, setPrograms] = useState<Program[]>([]);
  useEffect(() => {
    let cancelled = false;
    const load = () => api.nowNext([channel.id]).then(([row]) => !cancelled && setPrograms(row?.programs ?? [])).catch(() => {});
    load();
    const t = setInterval(load, 60000);
    return () => {
      cancelled = true;
      clearInterval(t);
    };
  }, [channel.id]);

  const [current, next] = programs;
  const t = nowSeconds();
  return (
    <div className="channel-bar glass">
      <div className="zap">
        <button className="player-icon" onClick={channelUp} aria-label="Channel up" title="Channel up (↑)">
          <ChevronUp size={18} />
        </button>
        <button className="player-icon" onClick={channelDown} aria-label="Channel down" title="Channel down (↓)">
          <ChevronDown size={18} />
        </button>
      </div>
      <ChannelLogo src={channel.logoUrl} name={channel.name} size={44} />
      <div className="channel-bar-text">
        <div className="channel-bar-name">
          {channel.number != null && <span className="num">{channel.number}</span>} {channel.name}
        </div>
        {current ? (
          <>
            <div className="channel-bar-title">{current.title}</div>
            <div className="channel-bar-meta">
              {clockTime(current.start)} – {clockTime(current.end)} · {remaining(current.end - t)}
              {next && (
                <span className="next">
                  Next: {clockTime(next.start)} {next.title}
                </span>
              )}
            </div>
            <div className="live-progress small">
              <div style={{ width: `${((t - current.start) / (current.end - current.start)) * 100}%` }} />
            </div>
          </>
        ) : (
          <div className="channel-bar-meta">No guide listings for this channel</div>
        )}
      </div>
    </div>
  );
}

/** Record what's on, or stop the recording that's running. */
function LiveRecordButton({ channel }: { channel: Channel }) {
  const recordings = useApp((s) => s.recordings);
  const [current, setCurrent] = useState<Program | null>(null);
  useEffect(() => {
    api.nowNext([channel.id]).then(([row]) => setCurrent(row?.programs.find((p) => p.start <= nowSeconds() && nowSeconds() < p.end) ?? null)).catch(() => {});
  }, [channel.id]);
  const active = [...recordings.values()].find((r) => r.channelId === channel.id && r.status === "recording") ?? (current ? recordingFor(recordings, channel.id, current.start, current.id) : undefined);
  const isRecording = active?.status === "recording";
  return (
    <button
      className={`btn btn-glass btn-small player-record ${isRecording ? "on" : ""}`}
      onClick={() => {
        if (isRecording && active) {
          const program = current ?? ({ id: active.programId ?? "", start: active.start } as Program);
          cancelOrStopRecording(program, channel);
        } else recordChannelNow(channel, current);
      }}
    >
      {isRecording ? <CircleStop size={15} /> : <CircleDot size={15} color="var(--recording)" />}
      {isRecording ? "Stop Recording" : "Record"}
    </button>
  );
}

// MARK: Live buffer timeline

/**
 * A buffered live channel's timeline: what's kept so far, where you are in
 * it (by the clock), how far behind live, and a way back to live.
 */
function BufferTimeline({ video, onSeek, onGoLive }: { video: React.RefObject<HTMLVideoElement | null>; onSeek: (t: number) => void; onGoLive: () => void }) {
  const [scrub, setScrub] = useState<number | null>(null);
  // Live moves on while paused, and the buffer grows with it.
  const [, tick] = useState(0);
  useEffect(() => {
    const timer = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, []);
  const el = video.current;
  const range = el ? bufferRange(el) : null;
  if (!range) {
    return (
      <div className="player-timeline live">
        <span className="live-pill">LIVE</span>
      </div>
    );
  }
  // The element's own time: a buffer's clock starts at zero, like any stream's.
  const shown = scrub ?? (el && Number.isFinite(el.currentTime) ? el.currentTime : range.end);
  const behind = Math.max(0, range.end - shown);
  const live = behind < LIVE_EDGE;
  const span = Math.max(1, range.end - range.start);
  return (
    <div className="player-timeline">
      <span className="clock">{clockTime(nowSeconds() - behind)}</span>
      <input
        type="range"
        className="scrubber"
        min={range.start}
        max={range.end}
        step={1}
        value={Math.min(Math.max(shown, range.start), range.end)}
        style={{ "--fill": `${((shown - range.start) / span) * 100}%` } as React.CSSProperties}
        onChange={(e) => setScrub(Number(e.target.value))}
        onMouseUp={() => {
          if (scrub != null) onSeek(scrub);
          setScrub(null);
        }}
        onKeyUp={() => {
          if (scrub != null) onSeek(scrub);
          setScrub(null);
        }}
        aria-label="Position"
      />
      {live ? (
        <span className="live-pill">LIVE</span>
      ) : (
        <>
          <span className="clock">−{playerClock(behind)}</span>
          <button className="live-pill behind" onClick={onGoLive} title="Back to live">
            Go Live
          </button>
        </>
      )}
    </div>
  );
}
