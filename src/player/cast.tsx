// Casting to a Chromecast or Google TV, offered beside AirPlay. Looking for
// TVs starts with the player (as WebKit looks for AirPlay devices while the
// player listens), so the Cast button shows once one is on the network.
// Choosing one moves what's playing there: Xorvio's own player pauses (a
// channel's stream goes, since most logins allow one connection), the TV
// plays the title from where it was, and the player shows where it's playing
// with calm controls of its own. Stopping carries on here from where the TV
// was. While casting, choosing another channel or the next episode sends
// that to the TV too, and the media keys work the TV.

import { ArrowLeft, Cast, Check, Expand, Pause, Play, RotateCcw, RotateCw, Tv, Volume1, Volume2, VolumeX, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { create } from "zustand";
import { openMenu, type MenuEntry } from "../components/Menu";
import { api, errorText } from "../lib/api";
import { nowSeconds, playerClock } from "../lib/format";
import { imageSrc } from "../lib/images";
import { useApp, type NowPlaying, type Presentation } from "../lib/store";
import type { CastDetails, CastDevice, CastStatus, ResolvedStream } from "../lib/types";

export interface CastSession {
  device: CastDevice;
  /** Connecting to the TV, starting the title there, playing, or stopped by the TV itself. */
  phase: "connecting" | "starting" | "casting" | "ended";
  /** What's on the TV, or going there. */
  item: NowPlaying;
  /** What the TV was told about it (a channel's programme). */
  details: CastDetails | null;
  status: CastStatus | null;
  /** When `status` came, by this computer's clock. */
  at: number;
  /** Why it ended, when the TV stopped by itself. */
  message: string | null;
}

export const useCast = create<{ devices: CastDevice[]; session: CastSession | null }>(() => ({ devices: [], session: null }));

/** Whether a TV has (or is getting) what's playing, so the player here stays paused. */
export function isCasting() {
  return !!useCast.getState().session;
}

/** Where the title is on the TV now: the last report, moved on while it plays. */
export function castPosition(session: CastSession) {
  const status = session.status;
  if (!status) return 0;
  const moved = status.state === "playing" ? (Date.now() - session.at) / 1000 : 0;
  const position = status.positionSeconds + moved;
  return status.durationSeconds ? Math.min(position, status.durationSeconds) : position;
}

function updateSession(patch: Partial<CastSession>) {
  useCast.setState((s) => ({ session: s.session && { ...s.session, ...patch } }));
}

function onStatus(status: CastStatus) {
  const session = useCast.getState().session;
  if (!session || status.deviceId !== session.device.id) return;
  const ended = (status.state === "stopped" || status.state === "failed") && session.phase === "casting";
  updateSession({ status, at: Date.now(), ...(ended ? { phase: "ended", message: status.message } : {}) });
}

let looking = false;

/** Starts looking for TVs, once; the list stays up to date from then on. */
function lookForTVs() {
  if (looking) return;
  looking = true;
  api.onCastDevices((devices) => useCast.setState({ devices }));
  api.onCastChanged(onStatus);
  api
    .castDevices()
    .then((devices) => useCast.setState({ devices }))
    .catch((e) => api.log("warn", `looking for TVs: ${errorText(e)}`).catch(() => {}));
}

/** The title, its episode or year, or the programme on a channel, and its artwork. */
async function castDetails(item: NowPlaying): Promise<CastDetails> {
  let subtitle = item.subtitle ?? null;
  if (item.kind === "channel" && item.channel) {
    // The programme on now says more on a TV than the channel's number.
    const [row] = await api.nowNext([item.channel.id]).catch(() => []);
    const t = nowSeconds();
    subtitle = row?.programs.find((p) => p.start <= t && t < p.end)?.title ?? null;
  }
  return { title: item.title, subtitle, artworkUrl: item.artworkUrl ?? item.channel?.logoUrl ?? null };
}

/** Lets a channel's stream here go: the TV's opens the provider's one connection. */
function releaseLocalStream() {
  const { stream } = useApp.getState();
  if (!stream) return;
  api.releaseStream(stream.url).catch(() => {});
  useApp.setState({ stream: null });
}

function saveCastProgress(item: NowPlaying, position: number, duration: number | null, finished = false) {
  if (!duration || (item.kind !== "movie" && item.kind !== "episode" && item.kind !== "recording")) return;
  api
    .saveProgress({ itemId: item.itemId, kind: item.kind, sourceId: item.sourceId, seriesId: item.seriesId ?? null, positionSeconds: position, durationSeconds: duration, finished: finished || position / duration > 0.93, updatedAt: nowSeconds() })
    .catch(() => {});
}

interface CastingProps {
  video: RefObject<HTMLVideoElement | null>;
  nowPlaying: NowPlaying | null;
  stream: ResolvedStream | null;
  isLive: boolean;
  /** Where the title is here, in its own time. */
  position: number;
  /** The title played to its end on the TV: the next episode, or closing. */
  onFinished: () => void;
}

/** Casting for the player: the session, what to do with it, and what the media keys should do meanwhile. */
export function useCasting({ video, nowPlaying, stream, isLive, position, onFinished }: CastingProps) {
  const session = useCast((s) => s.session);
  useEffect(lookForTVs, []);
  const latest = useRef({ position, isLive, onFinished });
  latest.current = { position, isLive, onFinished };

  /** Plays `item` on the connected TV, from `from` for a film or an episode. */
  const loadOnTv = useCallback(async (item: NowPlaying, from: number | null, presentation: Presentation) => {
    updateSession({ item, phase: "starting", details: null, status: null, message: null });
    const live = item.kind === "channel";
    if (live) releaseLocalStream();
    try {
      const details = await castDetails(item);
      if (useCast.getState().session?.item === item) updateSession({ details });
      const status = await api.castLoad(item.request, details, live ? null : from);
      if (useCast.getState().session?.item !== item) return;
      updateSession({ phase: "casting", status, at: Date.now() });
    } catch (e) {
      if (useCast.getState().session?.item !== item) return;
      api.castStop().catch(() => {});
      useCast.setState({ session: null });
      useApp.getState().showToast(errorText(e), "error");
      // It plays here instead.
      useApp.getState().play({ ...item, resumeFrom: from ?? item.resumeFrom }, presentation === "hidden" ? "full" : presentation);
    }
  }, []);

  const castTo = useCallback(
    async (device: CastDevice) => {
      const { nowPlaying: item, presentation } = useApp.getState();
      const before = useCast.getState().session;
      if (!item || before?.device.id === device.id) return;
      const el = video.current;
      const wasPlaying = !!el && !el.paused;
      // A plain pause: one asked for by a person would start a channel's buffer.
      el?.pause();
      // From another TV, this one picks up where that was.
      const from = before?.status && !before.status.live ? castPosition(before) : latest.current.isLive ? null : latest.current.position;
      useCast.setState({ session: { device, phase: "connecting", item, details: null, status: null, at: Date.now(), message: null } });
      const stillThisTv = () => useCast.getState().session?.device.id === device.id;
      try {
        await api.castConnect(device.id);
      } catch (e) {
        if (!stillThisTv()) return;
        useCast.setState({ session: null });
        useApp.getState().showToast(errorText(e), "error");
        if (before) useApp.getState().play({ ...item, resumeFrom: from ?? item.resumeFrom }, presentation);
        else if (wasPlaying) el?.play().catch(() => {});
        return;
      }
      if (!stillThisTv()) return;
      // Whatever is playing by now (it may have changed while the TV woke up).
      const current = useApp.getState().nowPlaying;
      if (current) loadOnTv(current, current === item ? from : (current.resumeFrom ?? null), presentation);
    },
    [video, loadOnTv],
  );

  /** Stops the TV and, unless `carryOn` is false, plays on here from where it was. */
  const stopCasting = useCallback(async (carryOn = true) => {
    const current = useCast.getState().session;
    if (!current) return;
    const guess = current.status ? castPosition(current) : null;
    useCast.setState({ session: null });
    const at = await api.castStop().catch(() => guess);
    const live = current.status?.live ?? current.item.kind === "channel";
    if (!live && at != null) saveCastProgress(current.item, at, current.status?.durationSeconds ?? null);
    if (!carryOn) return;
    const { nowPlaying: item, presentation } = useApp.getState();
    if (!item) return;
    useApp.getState().play({ ...item, resumeFrom: live ? undefined : (at ?? guess ?? item.resumeFrom) }, presentation === "hidden" ? "full" : presentation);
  }, []);

  // Something else chosen while casting (another channel, the next episode)
  // goes to the TV too; the player closing stops it.
  useEffect(() => {
    const current = useCast.getState().session;
    if (!current || current.item === nowPlaying) return;
    if (!nowPlaying) {
      stopCasting(false);
      return;
    }
    if (current.phase === "connecting") return;
    if (current.phase === "ended") {
      // The TV stopped by itself earlier; this plays here.
      api.castStop().catch(() => {});
      useCast.setState({ session: null });
      return;
    }
    loadOnTv(nowPlaying, nowPlaying.resumeFrom ?? null, useApp.getState().presentation);
  }, [nowPlaying, loadOnTv, stopCasting]);

  // A channel plays on the TV only: a stream of it here would be a second connection.
  useEffect(() => {
    const current = useCast.getState().session;
    if (current && current.phase !== "connecting" && stream?.isLive) releaseLocalStream();
  }, [stream]);

  // The title ended on the TV: the next episode goes there too, or the
  // player closes (which stops the TV).
  const finished = session?.status?.state === "finished";
  useEffect(() => {
    const current = useCast.getState().session;
    if (!finished || !current) return;
    const duration = current.status?.durationSeconds ?? null;
    if (duration) saveCastProgress(current.item, duration, duration, true);
    latest.current.onFinished();
  }, [finished]);

  // Progress is saved as the TV plays, as it is here.
  const casting = session?.phase === "casting";
  useEffect(() => {
    if (!casting) return;
    const timer = setInterval(() => {
      const current = useCast.getState().session;
      if (current?.status && !current.status.live && current.status.state !== "finished") saveCastProgress(current.item, castPosition(current), current.status.durationSeconds);
    }, 10_000);
    return () => clearInterval(timer);
  }, [casting]);

  const status = session?.status ?? null;
  const report = (e: unknown) => useApp.getState().showToast(errorText(e), "error");
  const play = useCallback(() => api.castPlay().catch(report), []);
  const pause = useCallback(() => api.castPause().catch(report), []);
  const seek = useCallback((to: number) => {
    const current = useCast.getState().session;
    if (!current?.status?.canSeek) return;
    const duration = current.status.durationSeconds;
    const target = Math.max(0, duration ? Math.min(duration - 2, to) : to);
    // Shown at once; the TV says where it really is shortly.
    updateSession({ status: { ...current.status, positionSeconds: target }, at: Date.now() });
    api.castSeek(target).catch(report);
  }, []);

  /** The Cast menu: the TVs found, or why what's playing can't go to one. */
  const openCastMenu = useCallback(
    async (e: React.MouseEvent<HTMLElement>) => {
      const anchor = { clientX: e.clientX, clientY: e.clientY, currentTarget: e.currentTarget, preventDefault() {}, stopPropagation() {} };
      e.preventDefault();
      e.stopPropagation();
      const item = useApp.getState().nowPlaying;
      if (!item) return;
      const reason = await api.castCheck(item.request).catch((error) => errorText(error));
      const { devices, session: current } = useCast.getState();
      const entries: MenuEntry[] = [{ heading: "Play on a TV" }];
      if (reason) entries.push({ heading: reason });
      for (const device of devices) {
        const chosen = current?.device.id === device.id;
        entries.push({
          label: device.name,
          icon: chosen ? <Check size={15} strokeWidth={2} /> : <Tv size={15} strokeWidth={1.8} />,
          disabled: !!reason && !chosen,
          onSelect: () => !chosen && castTo(device),
        });
      }
      if (!devices.length) entries.push({ heading: "No TVs found on this network" });
      if (current) entries.push({ separator: true }, { label: "Stop casting", icon: <X size={15} strokeWidth={1.8} />, onSelect: () => stopCasting() });
      openMenu(anchor, entries, true);
    },
    [castTo, stopCasting],
  );

  // While casting, the media keys and Now Playing are the TV's.
  const mediaSession =
    session && status
      ? {
          paused: status.state !== "playing",
          position: castPosition(session),
          duration: status.durationSeconds ?? 0,
          isLive: status.live,
          onSeek: seek,
          onPlay: play,
          onPause: pause,
        }
      : {};

  return { session, castTo, stopCasting, play, pause, seek, openCastMenu, mediaSession };
}

export type Casting = ReturnType<typeof useCasting>;

/** The Cast button for the player's control bar, shown once a TV is found. */
export function CastButton({ casting }: { casting: Casting }) {
  const found = useCast((s) => s.devices.length > 0 || !!s.session);
  if (!found) return null;
  return (
    <button className={`player-icon ${casting.session ? "on" : ""}`} aria-label="Cast to a TV" onClick={casting.openCastMenu}>
      <Cast size={19} />
    </button>
  );
}

/** Where the title is on the TV, ticking along while it plays. */
function useCastClock(session: CastSession) {
  const [, tick] = useState(0);
  const playing = session.status?.state === "playing";
  useEffect(() => {
    if (!playing) return;
    const timer = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, [playing]);
  return castPosition(session);
}

/** The TV's volume: the slider moves at once, the TV a moment later. */
function CastVolume({ status }: { status: CastStatus }) {
  const [level, setLevel] = useState(status.volume);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const dragging = useRef(false);
  useEffect(() => {
    if (!dragging.current) setLevel(status.volume);
  }, [status.volume]);
  useEffect(() => () => clearTimeout(timer.current), []);
  const shown = status.muted ? 0 : level;
  const Icon = shown === 0 ? VolumeX : shown < 0.5 ? Volume1 : Volume2;
  return (
    <>
      <button className="player-icon" aria-label={status.muted ? "Unmute the TV" : "Mute the TV"} onClick={() => api.castMute(!status.muted).catch(() => {})}>
        <Icon size={19} />
      </button>
      <input
        type="range"
        className="volume"
        min={0}
        max={1}
        step={0.01}
        value={shown}
        style={{ "--fill": `${shown * 100}%` } as React.CSSProperties}
        onPointerDown={() => (dragging.current = true)}
        onPointerUp={() => (dragging.current = false)}
        onChange={(e) => {
          const next = Number(e.target.value);
          setLevel(next);
          clearTimeout(timer.current);
          timer.current = setTimeout(() => api.castVolume(next).catch(() => {}), 150);
        }}
        aria-label="TV volume"
      />
    </>
  );
}

interface CastPanelProps {
  casting: Casting;
  presentation: Presentation;
  onBack: () => void;
  onClose: () => void;
}

/** "Playing on Living Room TV", in place of the video while casting. */
export function CastPanel({ casting, presentation, onBack, onClose }: CastPanelProps) {
  const session = casting.session!;
  const { device, phase, item, status } = session;
  const position = useCastClock(session);
  const [scrub, setScrub] = useState<number | null>(null);
  const full = presentation === "full";
  const playing = status?.state === "playing";
  const duration = status?.durationSeconds ?? 0;
  const canSeek = phase === "casting" && !!status?.canSeek && duration > 0;
  const shown = scrub ?? position;
  const artwork = item.artworkUrl ?? item.channel?.logoUrl ?? null;
  // A channel's own subtitle is its number; the programme on now says more.
  const subtitle = session.details?.subtitle ?? (item.kind === "channel" ? null : item.subtitle);
  const where =
    phase === "connecting" ? `Connecting to ${device.name}…` : phase === "starting" ? `Starting on ${device.name}…` : phase === "ended" ? (session.message ?? `${device.name} stopped playing.`) : `Playing on ${device.name}`;
  const toggle = useCallback(() => (playing ? casting.pause() : casting.play()), [playing, casting]);

  // Keys work the TV as they work the player: space, arrows, Esc.
  useEffect(() => {
    if (!full) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement) return;
      if (e.key === " " || e.key === "k") {
        e.preventDefault();
        if (phase === "casting") toggle();
      } else if (e.key === "ArrowLeft" && canSeek) casting.seek(position - 10);
      else if (e.key === "ArrowRight" && canSeek) casting.seek(position + 10);
      else if (e.key === "Escape") {
        e.preventDefault();
        onBack();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [full, phase, toggle, canSeek, casting, position, onBack]);

  const playButton = (big: boolean) => (
    <button className={big ? "player-icon big" : "mini-play"} onClick={toggle} disabled={phase !== "casting"} aria-label={playing ? "Pause" : "Play"}>
      {playing ? <Pause size={big ? 22 : 26} fill="currentColor" strokeWidth={0} /> : <Play size={big ? 22 : 26} fill="currentColor" strokeWidth={0} />}
    </button>
  );

  return (
    <div
      className={`cast-panel ${presentation}`}
      // The small player opens full size from anywhere but its buttons, as the video does.
      onClick={(e) => !full && !(e.target as HTMLElement).closest("button") && useApp.getState().setPresentation("full")}
    >
      {artwork && <img className="cast-backdrop" src={imageSrc(artwork)} alt="" />}
      {full && (
        <div className="cast-top">
          <button className="player-round glass" onClick={onBack} aria-label="Back">
            <ArrowLeft size={18} />
          </button>
          <div style={{ flex: 1 }} />
          <button className="player-round glass" onClick={casting.openCastMenu} aria-label="Choose another TV">
            <Cast size={16} />
          </button>
        </div>
      )}
      {presentation === "mini" && (
        <div className="mini-top">
          <button className="player-round small glass" onClick={onClose} aria-label="Close">
            <X size={14} />
          </button>
          <button className="player-round small glass" onClick={() => useApp.getState().setPresentation("full")} aria-label="Full screen">
            <Expand size={13} />
          </button>
        </div>
      )}
      <div className="cast-card">
        <div className={`cast-where ${phase === "ended" ? "ended" : ""}`}>
          <Cast size={full ? 16 : 13} />
          <span>{where}</span>
        </div>
        <div className="cast-title">{item.title}</div>
        {full && subtitle && <div className="cast-subtitle">{subtitle}</div>}
        {phase === "ended" ? (
          <div className="cast-actions">
            <button className="btn btn-glass btn-small" onClick={() => casting.stopCasting()}>
              Play here
            </button>
            <button className="btn btn-glass btn-small" onClick={onClose}>
              Close
            </button>
          </div>
        ) : full ? (
          <div className="cast-controls glass">
            {canSeek && (
              <div className="player-timeline">
                <span className="clock">{playerClock(shown)}</span>
                <input
                  type="range"
                  className="scrubber"
                  min={0}
                  max={duration}
                  step={1}
                  value={Math.min(shown, duration)}
                  style={{ "--fill": `${(shown / duration) * 100}%` } as React.CSSProperties}
                  onChange={(e) => setScrub(Number(e.target.value))}
                  onMouseUp={() => {
                    if (scrub != null) casting.seek(scrub);
                    setScrub(null);
                  }}
                  onKeyUp={() => {
                    if (scrub != null) casting.seek(scrub);
                    setScrub(null);
                  }}
                  aria-label="Position"
                />
                <span className="clock">−{playerClock(Math.max(0, duration - shown))}</span>
              </div>
            )}
            {status?.live && (
              <div className="player-timeline live">
                <span className="live-pill">LIVE</span>
              </div>
            )}
            <div className="player-buttons">
              {canSeek && (
                <button className="player-icon" onClick={() => casting.seek(position - 10)} aria-label="Back 10 seconds">
                  <RotateCcw size={19} />
                </button>
              )}
              {playButton(true)}
              {canSeek && (
                <button className="player-icon" onClick={() => casting.seek(position + 10)} aria-label="Forward 10 seconds">
                  <RotateCw size={19} />
                </button>
              )}
              <div style={{ flex: 1 }} />
              {status && <CastVolume status={status} />}
              <button className="btn btn-glass btn-small cast-stop" onClick={() => casting.stopCasting()}>
                Stop casting
              </button>
            </div>
          </div>
        ) : (
          phase === "casting" && playButton(false)
        )}
      </div>
    </div>
  );
}
