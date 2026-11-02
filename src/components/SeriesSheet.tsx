// The sheets for a series recording: making one from a guide programme,
// changing one, and deleting one (keeping its recordings or not). The host
// also keeps the series and recordings fresh as the recorder schedules
// airings on its own.

import { TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { api, errorText } from "../lib/api";
import { createSeries, defaultSeriesOptions, removeSeries, updateSeries, useSeries, type SeriesSheetRequest } from "../lib/series";
import { useApp } from "../lib/store";
import type { SeriesRule, SeriesRuleOptions } from "../lib/types";
import { Sheet } from "./Sheet";

export function SeriesSheetHost() {
  const sheet = useSeries((s) => s.sheet);

  useEffect(() => {
    const { refresh } = useSeries.getState();
    refresh().catch(() => {});
    let timer: ReturnType<typeof setTimeout> | undefined;
    // A recording finishing changes its series' counts; a few at once are read once.
    const soon = () => {
      clearTimeout(timer);
      timer = setTimeout(() => refresh().catch(() => {}), 800);
    };
    const offList = api.onRecordingsChanged(() => {
      useApp.getState().refreshRecordings().catch(() => {});
      soon();
    });
    const offOne = api.onRecordingChanged((recording) => {
      if (recording.ruleId && recording.status !== "recording") soon();
    });
    return () => {
      clearTimeout(timer);
      offList();
      offOne();
    };
  }, []);

  if (!sheet) return null;
  const close = () => useSeries.getState().show(null);
  return sheet.type === "delete" ? <DeleteSeriesSheet key={sheet.rule.id} rule={sheet.rule} close={close} /> : <SeriesOptionsSheet key={sheet.type === "edit" ? sheet.rule.id : sheet.program.id} request={sheet} close={close} />;
}

const KEEP: [string, string][] = [
  ["0", "All"],
  ["3", "Latest 3"],
  ["5", "Latest 5"],
  ["10", "Latest 10"],
];

function SeriesOptionsSheet({ request, close }: { request: Exclude<SeriesSheetRequest, { type: "delete" }>; close: () => void }) {
  const settings = useApp((s) => s.settings);
  const editing = request.type === "edit" ? request.rule : null;
  const [options, setOptions] = useState<SeriesRuleOptions>(() =>
    editing
      ? { anyChannel: editing.anyChannel, newOnly: editing.newOnly, keep: editing.keep, paddingBeforeSeconds: editing.paddingBeforeSeconds, paddingAfterSeconds: editing.paddingAfterSeconds }
      : defaultSeriesOptions,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const set = (change: Partial<SeriesRuleOptions>) => setOptions((o) => ({ ...o, ...change }));

  const title = editing?.title ?? (request.type === "new" ? request.program.title : "");
  const channelName = editing?.channelName ?? (request.type === "new" ? request.channel.name : "");
  const ownPadding = options.paddingBeforeSeconds !== null || options.paddingAfterSeconds !== null;
  const before = Math.round((options.paddingBeforeSeconds ?? settings.recordPaddingBefore * 60) / 60);
  const after = Math.round((options.paddingAfterSeconds ?? settings.recordPaddingAfter * 60) / 60);

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      if (request.type === "new") await createSeries(request.program, request.channel, options);
      else await updateSeries(request.rule, options);
      close();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Sheet
      title={editing ? "Series Options" : "Record Series"}
      subtitle={title}
      onClose={close}
      width={540}
      footer={
        <>
          <button className="btn-plain" onClick={close}>
            Cancel
          </button>
          <button className="btn-accent" disabled={busy} onClick={save}>
            {editing ? "Save" : "Record Series"}
          </button>
        </>
      }
    >
      <div className="form-row">
        <label>Channel</label>
        <div className="segmented" style={{ justifySelf: "start" }}>
          <button className={!options.anyChannel ? "selected" : ""} onClick={() => set({ anyChannel: false })}>
            {channelName}
          </button>
          <button className={options.anyChannel ? "selected" : ""} onClick={() => set({ anyChannel: true })}>
            Any channel
          </button>
        </div>
      </div>
      <div className="form-row">
        <label>Episodes</label>
        <div className="segmented" style={{ justifySelf: "start" }}>
          <button className={options.newOnly ? "selected" : ""} onClick={() => set({ newOnly: true })}>
            New only
          </button>
          <button className={!options.newOnly ? "selected" : ""} onClick={() => set({ newOnly: false })}>
            Every airing
          </button>
        </div>
      </div>
      <p className="muted small sheet-note">
        {options.newOnly
          ? "Skips repeats, when the guide says which they are, and episodes already recorded or scheduled."
          : "Records every time the guide lists it, repeats included."}
        {options.anyChannel ? " An airing on several versions of a channel (HD, 4K) records once." : ""}
      </p>
      <div className="form-row">
        <label>Keep</label>
        <div className="segmented" style={{ justifySelf: "start" }}>
          {KEEP.map(([value, label]) => (
            <button key={value} className={String(options.keep) === value ? "selected" : ""} onClick={() => set({ keep: Number(value) })}>
              {label}
            </button>
          ))}
        </div>
      </div>
      {options.keep > 0 && <p className="muted small sheet-note">Beyond the latest {options.keep}, older recordings of the series go to the Trash.</p>}
      <div className="form-row">
        <label>Padding</label>
        <label className="toggle-row" style={{ justifySelf: "start" }}>
          <button
            role="switch"
            aria-checked={!ownPadding}
            className={`toggle ${!ownPadding ? "on" : ""}`}
            onClick={() => set(ownPadding ? { paddingBeforeSeconds: null, paddingAfterSeconds: null } : { paddingBeforeSeconds: before * 60, paddingAfterSeconds: after * 60 })}
          />
          <span>
            Same as Settings ({settings.recordPaddingBefore} min early, {settings.recordPaddingAfter} min late)
          </span>
        </label>
      </div>
      {ownPadding && (
        <>
          <div className="form-row">
            <label>Start early by</label>
            <MinuteStepper value={before} max={30} onChange={(m) => set({ paddingBeforeSeconds: m * 60 })} />
          </div>
          <div className="form-row">
            <label>Keep going for</label>
            <MinuteStepper value={after} max={120} onChange={(m) => set({ paddingAfterSeconds: m * 60 })} />
          </div>
        </>
      )}
      {error && (
        <p className="form-error">
          <TriangleAlert size={13} /> {error}
        </p>
      )}
    </Sheet>
  );
}

function MinuteStepper({ value, max, onChange }: { value: number; max: number; onChange: (minutes: number) => void }) {
  return (
    <div className="stepper">
      <button className="btn-plain" onClick={() => onChange(Math.max(0, value - 1))} disabled={value <= 0} aria-label="Less">
        −
      </button>
      <span>{value} min</span>
      <button className="btn-plain" onClick={() => onChange(Math.min(max, value + 1))} disabled={value >= max} aria-label="More">
        +
      </button>
    </div>
  );
}

function DeleteSeriesSheet({ rule, close }: { rule: SeriesRule; close: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const remove = async (deleteRecordings: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await removeSeries(rule, deleteRecordings);
      close();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  const kept = rule.recordedCount;
  return (
    <Sheet
      title="Delete Series?"
      subtitle={rule.title}
      onClose={close}
      width={460}
      footer={
        <>
          <button className="btn-plain" onClick={close}>
            Cancel
          </button>
          <div style={{ flex: 1 }} />
          {kept > 0 && (
            <button className="btn-plain" disabled={busy} onClick={() => remove(true)}>
              Delete Recordings Too
            </button>
          )}
          <button className="btn-accent danger" disabled={busy} onClick={() => remove(false)} autoFocus>
            {kept > 0 ? "Keep Recordings" : "Delete Series"}
          </button>
        </>
      }
    >
      <p className="muted" style={{ margin: 0, lineHeight: 1.5 }}>
        Nothing more of “{rule.title}” will record, and {rule.scheduledCount ? `its ${rule.scheduledCount} scheduled airing${rule.scheduledCount === 1 ? " is" : "s are"} cancelled` : "nothing is scheduled"}.
        {kept > 0 ? ` You can keep its ${kept} recording${kept === 1 ? "" : "s"}, or move ${kept === 1 ? "it" : "them"} to the Trash.` : ""}
      </p>
      {error && (
        <p className="form-error">
          <TriangleAlert size={13} /> {error}
        </p>
      )}
    </Sheet>
  );
}
