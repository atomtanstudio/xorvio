// A guide programme's details with what can be done with it: watch, play the
// replay or save it, record it or the series, or be reminded.

import { Bell, BellOff, BookmarkMinus, CircleDot, CircleStop, Download, Play, Repeat, RotateCcw } from "lucide-react";
import { cancelOrStopRecording, playChannel, playReplay, programEntry, recordProgram, setReminder, toggleWatchlist } from "../lib/actions";
import { clockTime, dayName, duration, nowSeconds, remaining } from "../lib/format";
import { catchupSaveFor, recordSeries, saveFromCatchup, seriesFor, useSeries } from "../lib/series";
import { recordingFor, useApp } from "../lib/store";
import type { Channel, Program } from "../lib/types";
import { Art, ChannelLogo } from "./Artwork";
import { Sheet } from "./Sheet";

export function ProgramSheet({ program, channel, onClose }: { program: Program; channel: Channel; onClose: () => void }) {
  const recordings = useApp((s) => s.recordings);
  const watchlistIds = useApp((s) => s.watchlistIds);
  const reminderIds = useApp((s) => s.reminderIds);
  const t = nowSeconds();
  const airing = program.start <= t && t < program.end;
  const past = program.end <= t;
  const canReplay = past && channel.catchupDays > 0 && t - program.start < channel.catchupDays * 86400;
  const recording = recordingFor(recordings, channel.id, program.start, program.id);
  const active = recording && (recording.status === "scheduled" || recording.status === "recording");
  const series = seriesFor(useSeries((s) => s.rules), program, channel);
  const saved = canReplay ? catchupSaveFor(program, channel) : undefined;
  const act = (fn: () => void) => () => {
    fn();
    onClose();
  };

  return (
    <Sheet onClose={onClose} width={600}>
      <div className="program-sheet">
        <Art className="program-sheet-art" src={program.imageUrl} fallback={program.title} />
        <div className="program-sheet-channel">
          <ChannelLogo src={channel.logoUrl} name={channel.name} size={24} />
          <span>{channel.name}</span>
          {airing && <span className="live-pill">LIVE</span>}
          {recording?.status === "recording" && (
            <span className="rec-badge active">
              <span className="dot" />
              REC
            </span>
          )}
          {recording?.status === "scheduled" && (
            <span className="rec-badge">
              <span className="dot" />
              SCHEDULED
            </span>
          )}
        </div>
        <h2 className="program-sheet-title">{program.title}</h2>
        <div className="program-sheet-meta">
          {dayName(program.start)} {clockTime(program.start)} – {clockTime(program.end)} · {duration(program.end - program.start)}
          {program.category ? ` · ${program.category}` : ""}
          {program.episode ? ` · ${program.episode}` : ""}
          {airing ? ` · ${remaining(program.end - t)}` : ""}
        </div>
        {program.subtitle && <p className="program-sheet-subtitle">{program.subtitle}</p>}
        {program.summary && <p className="program-sheet-summary">{program.summary}</p>}
        <div className="program-sheet-actions">
          {airing && (
            <button className="btn btn-primary btn-small" onClick={act(() => playChannel(channel))}>
              <Play size={14} fill="currentColor" strokeWidth={0} /> Watch Live
            </button>
          )}
          {canReplay && (
            <button className="btn btn-primary btn-small" onClick={act(() => playReplay(channel, program))}>
              <RotateCcw size={14} /> Play Replay
            </button>
          )}
          {canReplay && (
            <button className="btn-plain" disabled={!!saved} onClick={act(() => saveFromCatchup(program, channel))}>
              <Download size={14} /> {saved ? (saved.status === "recording" ? "Saving from Catch-up…" : "Saved from Catch-up") : "Save from Catch-up"}
            </button>
          )}
          {!past &&
            (active ? (
              <button className="btn-plain rec-active" onClick={act(() => cancelOrStopRecording(program, channel))}>
                <CircleStop size={14} /> {recording!.status === "recording" ? "Stop Recording" : "Cancel Recording"}
              </button>
            ) : (
              <button className="btn-plain" onClick={act(() => recordProgram(program, channel))}>
                <CircleDot size={14} color="var(--recording)" /> {airing ? "Record Now" : "Record"}
              </button>
            ))}
          {!past && (
            <button className={`btn-plain ${series ? "active" : ""}`} onClick={act(() => recordSeries(program, channel))}>
              <Repeat size={14} /> {series ? "Series Options…" : "Record Series…"}
            </button>
          )}
          {/* Saved, its reminder turns off and on without leaving the watchlist. */}
          {!past && !airing && !watchlistIds.has(program.id) && (
            <button className="btn-plain" onClick={act(() => toggleWatchlist(programEntry(program, channel)))}>
              <Bell size={14} /> Remind Me
            </button>
          )}
          {!past && !airing && watchlistIds.has(program.id) && (
            <button
              className={`btn-plain ${reminderIds.has(program.id) ? "active" : ""}`}
              onClick={act(() => setReminder({ ...programEntry(program, channel), remind: reminderIds.has(program.id) }, !reminderIds.has(program.id)))}
            >
              {reminderIds.has(program.id) ? <BellOff size={14} /> : <Bell size={14} />} {reminderIds.has(program.id) ? "Turn Off Reminder" : "Remind Me"}
            </button>
          )}
          {!past && watchlistIds.has(program.id) && (
            <button className="btn-plain" onClick={act(() => toggleWatchlist(programEntry(program, channel)))}>
              <BookmarkMinus size={14} /> Remove from Watchlist
            </button>
          )}
        </div>
      </div>
    </Sheet>
  );
}
