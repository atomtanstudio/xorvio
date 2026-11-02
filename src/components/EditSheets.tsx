// Sheets for naming things (a channel, a category, a group), the parental
// PIN, a channel's catch-up offset and the versions of a channel listed
// several times. Each opens from anywhere through a function that resolves
// when the sheet closes, like confirmAction.

import { Lock, Play, TriangleAlert } from "lucide-react";
import { useState } from "react";
import { create } from "zustand";
import { api, errorText } from "../lib/api";
import { useApp } from "../lib/store";
import type { Channel } from "../lib/types";
import { Sheet } from "./Sheet";

interface NameRequest {
  type: "name";
  title: string;
  initial: string;
  placeholder: string;
  note?: string;
  confirmLabel: string;
  /** Saves the name; an error keeps the sheet open and shows why. */
  submit(name: string): Promise<void>;
  /** "Reset Name", for something renamed. */
  reset?: () => Promise<void>;
  resolve(saved: boolean): void;
}

interface PinRequest {
  type: "pin";
  message: string;
  resolve(unlocked: boolean): void;
}

export type PinChange = "create" | "change" | "remove";

interface PinChangeRequest {
  type: "pinChange";
  mode: PinChange;
  resolve(changed: boolean): void;
}

interface OffsetRequest {
  type: "offset";
  channel: Channel;
  /** Saves the minutes; an error keeps the sheet open and shows why. */
  submit(minutes: number): Promise<void>;
  resolve(saved: boolean): void;
}

interface VersionsRequest {
  type: "versions";
  row: Channel;
  play(versionId: string): void;
  /** Always plays a version, or (null) the best one again. */
  prefer(versionId: string | null): Promise<void>;
  resolve(done: boolean): void;
}

type Request = NameRequest | PinRequest | PinChangeRequest | OffsetRequest | VersionsRequest;

const useSheets = create<{ request: Request | null; set(request: Request | null): void }>((set) => ({ request: null, set: (request) => set({ request }) }));

/** Asks for a name. Resolves true once it's saved (or reset). */
export function askName(options: Omit<NameRequest, "type" | "resolve" | "initial" | "placeholder"> & { initial?: string; placeholder?: string }): Promise<boolean> {
  return new Promise((resolve) => useSheets.getState().set({ type: "name", initial: "", placeholder: "", ...options, resolve }));
}

/** Asks for the parental PIN and unlocks with it. Resolves true when it was right. */
export function askPin(message: string): Promise<boolean> {
  return new Promise((resolve) => useSheets.getState().set({ type: "pin", message, resolve }));
}

/** Creates, changes or removes the PIN. Resolves true when it was done. */
export function changePin(mode: PinChange): Promise<boolean> {
  return new Promise((resolve) => useSheets.getState().set({ type: "pinChange", mode, resolve }));
}

/** Asks how many minutes to move a channel's replays by. Resolves true once saved. */
export function askCatchupOffset(options: Omit<OffsetRequest, "type" | "resolve">): Promise<boolean> {
  return new Promise((resolve) => useSheets.getState().set({ type: "offset", ...options, resolve }));
}

/** Lists the versions of a row of similar channels, to play one or always play it. */
export function showVersions(options: Omit<VersionsRequest, "type" | "resolve">): Promise<boolean> {
  return new Promise((resolve) => useSheets.getState().set({ type: "versions", ...options, resolve }));
}

export function EditSheetHost() {
  const request = useSheets((s) => s.request);
  if (!request) return null;
  const close = (result: boolean) => {
    useSheets.getState().set(null);
    request.resolve(result);
  };
  switch (request.type) {
    case "name":
      return <NameSheet key={request.title + request.initial} request={request} close={close} />;
    case "pin":
      return <PinSheet request={request} close={close} />;
    case "pinChange":
      return <PinChangeSheet key={request.mode} request={request} close={close} />;
    case "offset":
      return <OffsetSheet key={request.channel.id} request={request} close={close} />;
    case "versions":
      return <VersionsSheet key={request.row.id} request={request} close={close} />;
  }
}

/** "+2 min", "−3 min", or none. */
export function offsetLabel(minutes: number) {
  return minutes === 0 ? "No offset" : `${minutes > 0 ? "+" : "−"}${Math.abs(minutes)} min`;
}

const MAX_OFFSET = 30;

function OffsetSheet({ request, close }: { request: OffsetRequest; close: (saved: boolean) => void }) {
  const initial = request.channel.catchupOffsetMinutes;
  const [minutes, setMinutes] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const clamp = (value: number) => Math.max(-MAX_OFFSET, Math.min(MAX_OFFSET, value));
  const save = async (value: number) => {
    setBusy(true);
    setError(null);
    try {
      await request.submit(value);
      close(true);
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  return (
    <Sheet
      title="Catch-up Offset"
      subtitle={`For replays of ${request.channel.name}`}
      onClose={() => close(false)}
      width={420}
      footer={
        <>
          {initial !== 0 && (
            <button className="btn-plain" disabled={busy} onClick={() => save(0)}>
              Remove Offset
            </button>
          )}
          <div style={{ flex: 1 }} />
          <button className="btn-plain" onClick={() => close(false)}>
            Cancel
          </button>
          <button className="btn-accent" disabled={busy || minutes === initial} onClick={() => save(minutes)}>
            Save
          </button>
        </>
      }
    >
      <div className="catchup-offset">
        <button className="btn-plain" onClick={() => setMinutes((m) => clamp(m - 1))} disabled={minutes <= -MAX_OFFSET} aria-label="A minute earlier">
          −
        </button>
        <span className="catchup-offset-value">{offsetLabel(minutes)}</span>
        <button className="btn-plain" onClick={() => setMinutes((m) => clamp(m + 1))} disabled={minutes >= MAX_OFFSET} aria-label="A minute later">
          +
        </button>
      </div>
      <input
        className="catchup-offset-slider"
        type="range"
        min={-MAX_OFFSET}
        max={MAX_OFFSET}
        step={1}
        value={minutes}
        onChange={(e) => setMinutes(Number(e.target.value))}
        aria-label="Minutes"
        autoFocus
      />
      <p className="muted small sheet-note">
        If replays on this channel begin before the programme does, move them later; if they miss its start, move them earlier. The guide and the player keep the programme's own times.
      </p>
      {error && (
        <p className="form-error">
          <TriangleAlert size={13} /> {error}
        </p>
      )}
    </Sheet>
  );
}

function VersionsSheet({ request, close }: { request: VersionsRequest; close: (done: boolean) => void }) {
  const { row } = request;
  const playingId = useApp((s) => s.nowPlaying?.channel?.id);
  const [busy, setBusy] = useState(false);
  const chosen = row.versions?.some((v) => v.preferred);
  const prefer = async (versionId: string | null) => {
    setBusy(true);
    await request.prefer(versionId);
    close(true);
  };
  return (
    <Sheet
      title={row.name}
      subtitle="Your source lists this channel more than once. Xorvio plays the best version unless you choose one."
      onClose={() => close(false)}
      width={500}
      footer={
        <>
          {chosen && (
            <button className="btn-plain" disabled={busy} onClick={() => prefer(null)}>
              Play the Best Version
            </button>
          )}
          <div style={{ flex: 1 }} />
          <button className="btn-accent" onClick={() => close(false)}>
            Done
          </button>
        </>
      }
    >
      <div className="version-list">
        {row.versions?.map((v) => (
          <div key={v.id} className="version-row">
            <span className="version-text">
              <span className="version-name">
                {v.name}
                {v.quality && <span className="quality-tag">{v.quality}</span>}
              </span>
              <span className="muted small">{v.id === playingId ? "Playing now" : v.preferred ? "Always plays" : v.id === row.id ? "Plays by default" : " "}</span>
            </span>
            <button
              className="btn-plain"
              onClick={() => {
                request.play(v.id);
                close(true);
              }}
            >
              <Play size={13} /> Play
            </button>
            <button className="btn-plain" disabled={busy || v.preferred} onClick={() => prefer(v.id)}>
              Always Play
            </button>
          </div>
        ))}
      </div>
    </Sheet>
  );
}

function NameSheet({ request, close }: { request: NameRequest; close: (saved: boolean) => void }) {
  const [name, setName] = useState(request.initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async (work: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await work();
      close(true);
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  const save = () => name.trim() && !busy && run(() => request.submit(name.trim()));
  return (
    <Sheet
      title={request.title}
      onClose={() => close(false)}
      width={440}
      footer={
        <>
          {request.reset && (
            <button className="btn-plain" disabled={busy} onClick={() => run(request.reset!)}>
              Reset Name
            </button>
          )}
          <div style={{ flex: 1 }} />
          <button className="btn-plain" onClick={() => close(false)}>
            Cancel
          </button>
          <button className="btn-accent" disabled={!name.trim() || busy} onClick={save}>
            {request.confirmLabel}
          </button>
        </>
      }
    >
      <input className="text-input" value={name} placeholder={request.placeholder} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && save()} onFocus={(e) => e.currentTarget.select()} autoFocus spellCheck={false} />
      {request.note && <p className="muted small sheet-note">{request.note}</p>}
      {error && (
        <p className="form-error">
          <TriangleAlert size={13} /> {error}
        </p>
      )}
    </Sheet>
  );
}

/** Four digits, shown as dots. */
function PinField({ value, onChange, onComplete, autoFocus, label }: { value: string; onChange: (pin: string) => void; onComplete?: (pin: string) => void; autoFocus?: boolean; label: string }) {
  return (
    <input
      className="text-input pin-input"
      type="password"
      inputMode="numeric"
      autoComplete="off"
      maxLength={4}
      aria-label={label}
      placeholder="••••"
      value={value}
      autoFocus={autoFocus}
      onChange={(e) => {
        const pin = e.target.value.replace(/\D/g, "").slice(0, 4);
        onChange(pin);
        if (pin.length === 4) onComplete?.(pin);
      }}
    />
  );
}

function PinSheet({ request, close }: { request: PinRequest; close: (unlocked: boolean) => void }) {
  const [pin, setPin] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async (value = pin) => {
    if (value.length !== 4 || busy) return;
    setBusy(true);
    try {
      if (await api.unlockParental(value)) return close(true);
      setError("That PIN isn't correct.");
    } catch (e) {
      setError(errorText(e));
    }
    setPin("");
    setBusy(false);
  };
  return (
    <Sheet
      onClose={() => close(false)}
      width={380}
      footer={
        <>
          <button className="btn-plain" onClick={() => close(false)}>
            Cancel
          </button>
          <button className="btn-accent" disabled={pin.length !== 4 || busy} onClick={() => submit()}>
            Unlock
          </button>
        </>
      }
    >
      <div className="pin-sheet">
        <Lock size={28} className="pin-sheet-icon" />
        <h2>Enter Parental PIN</h2>
        <p className="muted">{request.message}</p>
        <PinField label="PIN" value={pin} onChange={setPin} onComplete={submit} autoFocus />
        {error && (
          <p className="form-error">
            <TriangleAlert size={13} /> {error}
          </p>
        )}
      </div>
    </Sheet>
  );
}

const pinChangeText: Record<PinChange, { title: string; subtitle: string; confirm: string }> = {
  create: {
    title: "Set a Parental PIN",
    subtitle: "Categories whose names look adult lock straight away. Lock others by right-clicking them in Live TV.",
    confirm: "Set PIN",
  },
  change: { title: "Change PIN", subtitle: "Enter the PIN you have now, then the new one.", confirm: "Change PIN" },
  remove: {
    title: "Remove PIN",
    subtitle: "Locked channels and categories show again. Their locks come back if you set a PIN later.",
    confirm: "Remove PIN",
  },
};

function PinChangeSheet({ request, close }: { request: PinChangeRequest; close: (changed: boolean) => void }) {
  const { mode } = request;
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const text = pinChangeText[mode];
  const needsCurrent = mode !== "create";
  const needsNew = mode !== "remove";
  const ready = (!needsCurrent || current.length === 4) && (!needsNew || (next.length === 4 && again.length === 4));

  const submit = async () => {
    if (!ready || busy) return;
    if (needsNew && next !== again) return setError("The new PINs don't match.");
    setBusy(true);
    setError(null);
    try {
      await api.setPin(needsCurrent ? current : null, needsNew ? next : null);
      close(true);
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Sheet
      title={text.title}
      subtitle={text.subtitle}
      onClose={() => close(false)}
      width={440}
      footer={
        <>
          <button className="btn-plain" onClick={() => close(false)}>
            Cancel
          </button>
          <button className={mode === "remove" ? "btn-accent danger" : "btn-accent"} disabled={!ready || busy} onClick={submit}>
            {text.confirm}
          </button>
        </>
      }
    >
      <form
        className="pin-form"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        {needsCurrent && (
          <div className="form-row">
            <label>Current PIN</label>
            <PinField label="Current PIN" value={current} onChange={setCurrent} autoFocus />
          </div>
        )}
        {needsNew && (
          <>
            <div className="form-row">
              <label>New PIN</label>
              <PinField label="New PIN" value={next} onChange={setNext} autoFocus={!needsCurrent} />
            </div>
            <div className="form-row">
              <label>Confirm</label>
              <PinField label="Confirm new PIN" value={again} onChange={setAgain} />
            </div>
          </>
        )}
        {/* Return submits from any field. */}
        <button type="submit" hidden />
      </form>
      {error && (
        <p className="form-error">
          <TriangleAlert size={13} /> {error}
        </p>
      )}
    </Sheet>
  );
}
