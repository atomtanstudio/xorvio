import { CalendarPlus, CircleDot, CircleStop, Ellipsis, FastForward, FolderOpen, Pencil, Play, Repeat, Trash2, TriangleAlert, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Art, ChannelLogo } from "../components/Artwork";
import { openMenu, type MenuEntry } from "../components/Menu";
import { Sheet } from "../components/Sheet";
import { api, errorText } from "../lib/api";
import { clockTime, dayName, duration, fileSize, nowSeconds, playerClock, whenLabel } from "../lib/format";
import { showInFiles } from "../lib/platform";
import { deleteSeries, editSeries, seriesSummary, useSeries } from "../lib/series";
import { useApp } from "../lib/store";
import type { Channel, Recording, SeriesRule } from "../lib/types";

/** Recording now, scheduled, series, and recorded, newest first. */
export function RecordingsScreen() {
  const recordings = useApp((s) => s.recordings);
  const rules = useSeries((s) => s.rules);
  useEffect(() => {
    useSeries.getState().refresh().catch(() => {});
  }, []);
  const [scheduling, setScheduling] = useState(false);
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((n) => n + 1), 5000);
    return () => clearInterval(t);
  }, []);

  const groups = useMemo(() => {
    const all = [...recordings.values()];
    return {
      active: all.filter((r) => r.status === "recording").sort((a, b) => a.start - b.start),
      scheduled: all.filter((r) => r.status === "scheduled").sort((a, b) => a.start - b.start),
      done: all.filter((r) => r.status === "completed" || r.status === "partial").sort((a, b) => b.start - a.start),
      failed: all.filter((r) => r.status === "failed").sort((a, b) => b.start - a.start),
    };
  }, [recordings]);

  const empty = !groups.active.length && !groups.scheduled.length && !groups.done.length && !groups.failed.length && !rules.length;

  return (
    <div className="content">
      <div className="page recordings-page">
        <div className="page-header">
          <div>
            <h1 className="page-title">Recordings</h1>
            <p className="page-subtitle">Record what's on now, or schedule anything in the guide. Xorvio needs to be running to record.</p>
          </div>
          <button className="btn-accent" onClick={() => setScheduling(true)}>
            <CalendarPlus size={14} /> New Recording
          </button>
        </div>
        {empty ? (
          <div className="empty-state">
            <div>
              <CircleDot size={40} strokeWidth={1.4} />
              <h3>Nothing recorded yet</h3>
              <p>In Live TV, choose a programme and click Record, or Record Series for every new episode. You can also record a channel at a time you choose.</p>
              <div style={{ display: "flex", gap: 10, justifyContent: "center" }}>
                <button className="btn-plain" onClick={() => useApp.getState().go("live")}>
                  Open the Guide
                </button>
                <button className="btn-accent" onClick={() => setScheduling(true)}>
                  New Recording
                </button>
              </div>
            </div>
          </div>
        ) : (
          <>
            <RecordingGroup title="Recording Now" items={groups.active} />
            <RecordingGroup title="Scheduled" items={groups.scheduled} />
            <SeriesGroup rules={rules} />
            <RecordingGroup title="Recorded" items={groups.done} />
            <RecordingGroup title="Didn't Record" items={groups.failed} />
          </>
        )}
      </div>
      {scheduling && <ScheduleSheet onClose={() => setScheduling(false)} />}
    </div>
  );
}

function RecordingGroup({ title, items }: { title: string; items: Recording[] }) {
  if (!items.length) return null;
  return (
    <section className="recording-group">
      <h2 className="section-title">
        {title} <span className="section-count">{items.length}</span>
      </h2>
      <div className="recording-list">
        {items.map((r) => (
          <RecordingRow key={r.id} recording={r} />
        ))}
      </div>
    </section>
  );
}

/** The series recordings, each with its next airing and what it has. */
function SeriesGroup({ rules }: { rules: SeriesRule[] }) {
  if (!rules.length) return null;
  return (
    <section className="recording-group">
      <h2 className="section-title">
        Series <span className="section-count">{rules.length}</span>
      </h2>
      <div className="recording-list">
        {rules.map((rule) => (
          <SeriesRow key={rule.id} rule={rule} />
        ))}
      </div>
    </section>
  );
}

function SeriesRow({ rule }: { rule: SeriesRule }) {
  const items = (): MenuEntry[] => [
    { label: "Series Options…", icon: <Pencil size={15} />, onSelect: () => editSeries(rule) },
    { separator: true },
    { label: "Delete Series…", icon: <Trash2 size={15} />, destructive: true, onSelect: () => deleteSeries(rule) },
  ];
  const next = rule.nextAiring
    ? `Next ${whenLabel(rule.nextAiring)}${rule.anyChannel && rule.nextChannelName ? ` on ${rule.nextChannelName}` : ""}`
    : rule.newOnly
      ? "No new airings in the guide yet"
      : "Nothing in the guide yet";
  return (
    <div className="recording-row" onContextMenu={(e) => openMenu(e, items())} onDoubleClick={() => editSeries(rule)}>
      <Art className="recording-thumb" src={rule.artworkUrl} fallback="">
        {!rule.artworkUrl && (
          <div className="recording-thumb-logo">
            <ChannelLogo src={rule.channelLogoUrl} name={rule.channelName} size={40} />
          </div>
        )}
      </Art>
      <div className="recording-text">
        <div className="recording-title">
          <Repeat size={14} />
          {rule.title}
        </div>
        <div className="recording-meta">{seriesSummary(rule)}</div>
        <div className="recording-note">
          {next} · {rule.recordedCount} recorded
          {rule.scheduledCount > 1 ? ` · ${rule.scheduledCount} scheduled` : ""}
        </div>
        {rule.conflictCount > 0 && (
          <div className="recording-note" style={{ color: "var(--warning)" }}>
            <TriangleAlert size={12} /> {rule.conflictCount === 1 ? "One airing clashes" : `${rule.conflictCount} airings clash`} with other recordings for your provider's streams.
          </div>
        )}
      </div>
      <div className="recording-actions">
        <button className="btn-plain" onClick={() => editSeries(rule)}>
          Edit
        </button>
        <button className="icon-button" onClick={(e) => openMenu(e, items(), true)} aria-label="More">
          <Ellipsis size={16} />
        </button>
      </div>
    </div>
  );
}

function RecordingRow({ recording: r }: { recording: Recording }) {
  const refreshRecordings = useApp((s) => s.refreshRecordings);
  const showToast = useApp((s) => s.showToast);
  const rule = useSeries((s) => (r.ruleId ? s.rules.find((rule) => rule.id === r.ruleId) : undefined));
  const commercialSkip = useApp((s) => s.settings.commercialSkip);
  const t = nowSeconds();
  const playable = (r.status === "completed" || r.status === "partial" || r.status === "recording") && !!r.filePath;
  const recStart = r.start - r.paddingBeforeSeconds;
  const recEnd = r.end + r.paddingAfterSeconds;

  const act = async (fn: () => Promise<void>) => {
    try {
      await fn();
    } catch (e) {
      showToast(errorText(e), "error");
    }
    await refreshRecordings().catch(() => {});
  };

  const play = () =>
    useApp.getState().play({
      request: { kind: "recording", recordingId: r.id },
      kind: "recording",
      itemId: r.id,
      sourceId: r.sourceId,
      title: r.title,
      subtitle: `${r.channelName} · ${dayName(r.start)} ${clockTime(r.start)}`,
      artworkUrl: r.artworkUrl,
      durationHint: r.recordedSeconds ?? r.end - r.start,
    });

  // Looked for in the background; the player marks them once they're found.
  const findAdBreaks = () =>
    act(async () => {
      await api.findAdBreaks(r.id);
      showToast(`Looking for the ad breaks in “${r.title}”. You can watch it meanwhile.`);
    });

  const items = (): MenuEntry[] => [
      ...(playable ? [{ label: "Play", icon: <Play size={15} />, onSelect: play }] : []),
      ...(r.status === "recording" ? [{ label: "Stop Recording", icon: <CircleStop size={15} />, onSelect: () => act(() => api.stopRecording(r.id)) }] : []),
      ...(r.status === "scheduled" ? [{ label: "Cancel", icon: <X size={15} />, onSelect: () => act(() => api.cancelRecording(r.id)) }] : []),
      ...(r.filePath ? [{ label: showInFiles, icon: <FolderOpen size={15} />, onSelect: () => act(() => api.revealRecording(r.id)) }] : []),
      ...(playable && r.status !== "recording" && commercialSkip !== "off" ? [{ label: "Find Ad Breaks", icon: <FastForward size={15} />, onSelect: findAdBreaks }] : []),
      ...(rule ? [{ label: "Series Options…", icon: <Repeat size={15} />, onSelect: () => editSeries(rule) }] : []),
      ...(r.status !== "recording" && r.status !== "scheduled"
        ? [
            { separator: true as const },
            { label: "Delete Recording", icon: <Trash2 size={15} />, destructive: true, onSelect: () => act(() => api.deleteRecording(r.id, true)) },
            { label: "Remove from List, Keep File", icon: <X size={15} />, onSelect: () => act(() => api.deleteRecording(r.id, false)) },
          ]
        : []),
  ];

  // A catch-up save goes as fast as the provider sends it, so its progress is what's saved, not the clock.
  const progress =
    r.status !== "recording" ? null : r.fromCatchup ? Math.min(1, (r.recordedSeconds ?? 0) / Math.max(1, recEnd - recStart)) : Math.min(1, Math.max(0, (t - recStart) / (recEnd - recStart)));

  return (
    <div className={`recording-row ${r.status}`} onContextMenu={(e) => openMenu(e, items())} onDoubleClick={() => playable && play()}>
      <Art className="recording-thumb" src={r.artworkUrl} fallback="">
        {!r.artworkUrl && (
          <div className="recording-thumb-logo">
            <ChannelLogo src={r.channelLogoUrl} name={r.channelName} size={40} />
          </div>
        )}
        {playable && (
          <button className="recording-play" onClick={play} aria-label={`Play ${r.title}`}>
            <Play size={18} fill="currentColor" strokeWidth={0} />
          </button>
        )}
      </Art>
      <div className="recording-text">
        <div className="recording-title">
          {r.status === "recording" && (
            <span className="rec-badge active">
              <span className="dot" />
              REC
            </span>
          )}
          {r.status === "partial" && <span className="badge warn">PARTIAL</span>}
          {r.conflict && <span className="badge warn">CONFLICT</span>}
          {r.fromCatchup && <span className="badge">CATCH-UP</span>}
          {r.ruleId && r.status === "scheduled" && <Repeat size={13} aria-label="Part of a series" />}
          {r.title}
        </div>
        <div className="recording-meta">
          {r.channelName} · {dayName(r.start)} {clockTime(r.start)} – {clockTime(r.end)} · {duration(r.end - r.start)}
          {r.episode ? ` · ${r.episode}` : ""}
          {r.subtitle && r.subtitle !== r.episode ? ` · ${r.subtitle}` : ""}
        </div>
        {r.status === "recording" && (
          <div className="recording-progress">
            <div className="live-progress">
              <div style={{ width: `${(progress ?? 0) * 100}%` }} />
            </div>
            <span>
              {r.fromCatchup
                ? `Saving from catch-up · ${playerClock(r.recordedSeconds ?? 0)} of ${playerClock(recEnd - recStart)}${r.fileSizeBytes ? ` · ${fileSize(r.fileSizeBytes)}` : ""}`
                : `${playerClock(r.recordedSeconds ?? 0)} recorded${r.fileSizeBytes ? ` · ${fileSize(r.fileSizeBytes)}` : ""} · ends ${clockTime(recEnd)}`}
            </span>
          </div>
        )}
        {r.conflict && (
          <div className="recording-note" style={{ color: "var(--warning)" }}>
            <TriangleAlert size={12} /> {r.conflict}
          </div>
        )}
        {r.note && (
          <div className="recording-note">
            <FolderOpen size={12} /> {r.note}
          </div>
        )}
        {r.status === "scheduled" && (
          <div className="recording-note">
            Starts {whenLabel(recStart)}
            {r.paddingBeforeSeconds || r.paddingAfterSeconds ? ` (${Math.round(r.paddingBeforeSeconds / 60)} min early, ${Math.round(r.paddingAfterSeconds / 60)} min late)` : ""}
          </div>
        )}
        {(r.status === "completed" || r.status === "partial") && (
          <div className="recording-note">
            {r.recordedSeconds ? duration(r.recordedSeconds) : ""}
            {r.fileSizeBytes ? ` · ${fileSize(r.fileSizeBytes)}` : ""}
            {r.status === "partial" && r.error ? ` · ${r.error}` : ""}
          </div>
        )}
        {r.status === "failed" && (
          <div className="recording-note error">
            <TriangleAlert size={12} /> {r.error ?? "The recording couldn't start."}
          </div>
        )}
      </div>
      <div className="recording-actions">
        {r.status === "recording" && (
          <button className="btn-plain rec-active" onClick={() => act(() => api.stopRecording(r.id))}>
            <CircleStop size={14} /> Stop
          </button>
        )}
        {r.status === "scheduled" && (
          <button className="btn-plain" onClick={() => act(() => api.cancelRecording(r.id))}>
            Cancel
          </button>
        )}
        {playable && r.status !== "recording" && (
          <button className="btn-plain" onClick={play}>
            <Play size={13} fill="currentColor" strokeWidth={0} /> Play
          </button>
        )}
        <button className="icon-button" onClick={(e) => openMenu(e, items(), true)} aria-label="More">
          <Ellipsis size={16} />
        </button>
      </div>
    </div>
  );
}

function toLocalInput(t: number) {
  const d = new Date(t * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Record a channel at a chosen time, for channels without a guide or anything the guide gets wrong. */
function ScheduleSheet({ onClose }: { onClose: () => void }) {
  const settings = useApp((s) => s.settings);
  const upsert = useApp((s) => s.upsertRecording);
  const showToast = useApp((s) => s.showToast);
  const [channels, setChannels] = useState<Channel[]>([]);
  const [filter, setFilter] = useState("");
  const [channel, setChannel] = useState<Channel | null>(null);
  const roundedNow = Math.ceil(nowSeconds() / 300) * 300;
  const [start, setStart] = useState(toLocalInput(roundedNow));
  const [end, setEnd] = useState(toLocalInput(roundedNow + 3600));
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.channels({ limit: 5000 }).then(setChannels).catch(() => {});
  }, []);

  const shown = channels.filter((c) => !filter || c.name.toLowerCase().includes(filter.toLowerCase()) || String(c.number ?? "") === filter).slice(0, 60);
  const startT = Math.floor(new Date(start).getTime() / 1000);
  const endT = Math.floor(new Date(end).getTime() / 1000);
  const valid = channel && endT > startT && endT > nowSeconds();

  const submit = async () => {
    if (!channel || !valid) return;
    setBusy(true);
    try {
      const request = {
        sourceId: channel.sourceId,
        channelId: channel.id,
        channelName: channel.name,
        channelLogoUrl: channel.logoUrl,
        title: title.trim() || channel.name,
        subtitle: null,
        summary: null,
        artworkUrl: null,
        programId: null,
        start: startT,
        end: endT,
        paddingBeforeSeconds: 0,
        paddingAfterSeconds: 0,
      };
      const recording = startT <= nowSeconds() ? await api.recordNow(request) : await api.scheduleRecording(request);
      upsert(recording);
      showToast(startT <= nowSeconds() ? `Recording ${channel.name} now` : `${channel.name} will record ${whenLabel(startT)}`);
      onClose();
    } catch (e) {
      showToast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet
      title="New Recording"
      subtitle="Record a channel between two times."
      onClose={onClose}
      width={560}
      footer={
        <>
          <button className="btn-plain" onClick={onClose}>
            Cancel
          </button>
          <button className="btn-accent" disabled={!valid || busy} onClick={submit}>
            {startT <= nowSeconds() ? "Record Now" : "Schedule"}
          </button>
        </>
      }
    >
      <div className="form-row">
        <label>Channel</label>
        <input className="text-input" placeholder="Search channels" value={filter} onChange={(e) => setFilter(e.target.value)} autoFocus />
      </div>
      <div className="channel-picker">
        {shown.map((c) => (
          <button key={c.id} className={`channel-pick ${channel?.id === c.id ? "selected" : ""}`} onClick={() => setChannel(c)}>
            <ChannelLogo src={c.logoUrl} name={c.name} size={26} />
            <span className="num">{c.number ?? ""}</span>
            <span>{c.name}</span>
          </button>
        ))}
        {!shown.length && <p className="muted" style={{ padding: 10 }}>No channels match.</p>}
      </div>
      <div className="form-row">
        <label>Starts</label>
        <input className="text-input" type="datetime-local" value={start} onChange={(e) => setStart(e.target.value)} />
      </div>
      <div className="form-row">
        <label>Ends</label>
        <input className="text-input" type="datetime-local" value={end} onChange={(e) => setEnd(e.target.value)} />
      </div>
      <div className="form-row">
        <label>Name</label>
        <input className="text-input" placeholder={channel?.name ?? "Recording name"} value={title} onChange={(e) => setTitle(e.target.value)} />
      </div>
      <p className="muted small">
        Padding from Settings › Recording ({settings.recordPaddingBefore} min before, {settings.recordPaddingAfter} min after) applies to guide programmes, not to times you choose here.
      </p>
    </Sheet>
  );
}
