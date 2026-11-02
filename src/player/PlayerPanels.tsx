// The player's smaller pieces over the video: audio and subtitle sync, stream
// information, the skip and next-episode buttons, and the countdown to the
// next episode.

import { ChevronsRight, Minus, Plus, X } from "lucide-react";
import { useEffect, useState, type RefObject } from "react";
import { episodeCode } from "../lib/format";
import type { ResolvedStream } from "../lib/types";
import type { NextEpisode } from "./nextEpisode";

const signed = (seconds: number) => `${seconds > 0 ? "+" : seconds < 0 ? "−" : ""}${Math.abs(seconds).toFixed(1)} s`;

function Nudge({ label, hint, value, onChange }: { label: string; hint: string; value: number; onChange: (v: number) => void }) {
  const step = (by: number) => onChange(Math.round((value + by) * 10) / 10);
  return (
    <div className="sync-row">
      <div>
        <div className="sync-label">{label}</div>
        <div className="sync-hint">{hint}</div>
      </div>
      <div className="sync-controls">
        <button className="player-round small glass" onClick={() => step(-0.1)} aria-label={`${label} earlier`}>
          <Minus size={14} />
        </button>
        <span className="sync-value">{signed(value)}</span>
        <button className="player-round small glass" onClick={() => step(0.1)} aria-label={`${label} later`}>
          <Plus size={14} />
        </button>
      </div>
    </div>
  );
}

/**
 * Moves the audio or the subtitles against the picture. Subtitles move at
 * once; moving the audio restarts the stream through ffmpeg, so it waits for
 * Apply.
 */
export function SyncPanel({ stream, subtitleDelay, onSubtitleDelay, onAudioDelay, onClose }: { stream: ResolvedStream; subtitleDelay: number; onSubtitleDelay: (s: number) => void; onAudioDelay: (s: number) => void; onClose: () => void }) {
  const [audio, setAudio] = useState(stream.audioDelaySeconds);
  useEffect(() => setAudio(stream.audioDelaySeconds), [stream.audioDelaySeconds]);
  return (
    <div className="player-panel sync-panel glass" onMouseDown={(e) => e.stopPropagation()}>
      <div className="player-panel-header">
        <strong>Sync</strong>
        <button className="player-round small" onClick={onClose} aria-label="Close">
          <X size={14} />
        </button>
      </div>
      <Nudge label="Audio" hint="Later if voices come before lips move" value={audio} onChange={setAudio} />
      {audio !== stream.audioDelaySeconds && (
        <div className="sync-apply">
          <button className="btn btn-glass btn-small" onClick={() => setAudio(stream.audioDelaySeconds)}>
            Cancel
          </button>
          <button className="btn btn-primary btn-small" onClick={() => onAudioDelay(audio)}>
            Apply
          </button>
        </div>
      )}
      <Nudge label="Subtitles" hint="Later if they appear too soon" value={subtitleDelay} onChange={onSubtitleDelay} />
    </div>
  );
}

const modeNames: Record<ResolvedStream["mode"], string> = {
  native: "Direct",
  hls: "HLS",
  mpegts: "MPEG-TS",
  remux: "Remuxed by ffmpeg",
};

/** What's playing and how well, updated every second. */
export function StreamInfoPanel({ stream, video, onClose }: { stream: ResolvedStream; video: RefObject<HTMLVideoElement | null>; onClose: () => void }) {
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, []);
  const el = video.current;
  const quality = el?.getVideoPlaybackQuality?.();
  let ahead = 0;
  if (el)
    for (let i = 0; i < el.buffered.length; i++)
      if (el.buffered.start(i) <= el.currentTime && el.currentTime <= el.buffered.end(i)) ahead = el.buffered.end(i) - el.currentTime;
  const rows: [string, string | null | undefined][] = [
    ["Playing", modeNames[stream.mode]],
    ["Container", stream.info?.container],
    ["Video", [stream.info?.video, el?.videoWidth ? `${el.videoWidth}×${el.videoHeight}` : null].filter(Boolean).join(" · ") || null],
    ["Audio", stream.info?.audio],
    ["Buffered", `${ahead.toFixed(1)} s ahead`],
    ["Frames", quality ? `${quality.droppedVideoFrames} dropped of ${quality.totalVideoFrames}` : null],
    ["Speed", el && el.playbackRate !== 1 ? `${el.playbackRate}×` : null],
    ["Audio delay", stream.audioDelaySeconds ? signed(stream.audioDelaySeconds) : null],
  ];
  return (
    <div className="player-panel info-panel glass" onMouseDown={(e) => e.stopPropagation()}>
      <div className="player-panel-header">
        <strong>Stream Information</strong>
        <button className="player-round small" onClick={onClose} aria-label="Close">
          <X size={14} />
        </button>
      </div>
      <dl>
        {rows
          .filter(([, value]) => value)
          .map(([label, value]) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd>{value}</dd>
            </div>
          ))}
      </dl>
      {stream.info?.notes.map((note) => (
        <p key={note} className="info-note">
          {note}
        </p>
      ))}
    </div>
  );
}

/** "Skip Intro", "Skip Recap" or "Next Episode", bottom right above the controls. */
export function CornerButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button className="player-corner-button glass" onClick={onClick}>
      {label}
      <ChevronsRight size={16} />
    </button>
  );
}

/** Counts down to the next episode once one ends. */
export function NextCountdown({ next, onPlay, onCancel }: { next: NextEpisode; onPlay: () => void; onCancel: () => void }) {
  const [left, setLeft] = useState(10);
  useEffect(() => {
    if (left <= 0) {
      onPlay();
      return;
    }
    const t = setTimeout(() => setLeft((n) => n - 1), 1000);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [left]);
  const { episode } = next;
  return (
    <div className="player-center next-countdown">
      <div className="next-card glass">
        <div className="next-eyebrow">Next Episode in {left}</div>
        <div className="next-title">
          {episodeCode(episode.season, episode.number)} · {episode.title}
        </div>
        <div className="next-actions">
          <button className="btn btn-primary btn-small" onClick={onPlay}>
            Play Now
          </button>
          <button className="btn btn-glass btn-small" onClick={onCancel}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
