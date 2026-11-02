// Settings › Backup and Settings › Sync. Backing up saves one file where the
// person chooses; restoring replaces this computer's library, schedules,
// settings and sources, and restarts Xorvio to do it. Syncing keeps the
// library in step with other computers through a folder they share.

import { Laptop, RefreshCw, TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { Sheet } from "../components/Sheet";
import { api, errorText } from "../lib/api";
import { ago } from "../lib/format";
import { showInFiles } from "../lib/platform";
import { useApp } from "../lib/store";
import type { AutoBackup, BackupInfo, BackupStatus, SyncStatus } from "../lib/types";
import { Row, Segmented, SettingsGroup, Toggle } from "./Settings";

/** A folder as people know it: "iCloud Drive › Xorvio" rather than its full path. */
function folderLabel(path: string) {
  const parts = (text: string) => text.split(/[\\/]/).filter(Boolean);
  const icloud = path.match(/\/Library\/Mobile Documents\/com~apple~CloudDocs\/?(.*)$/);
  if (icloud) return ["iCloud Drive", ...parts(icloud[1])].join(" › ");
  // Dropbox, Google Drive, OneDrive… ("GoogleDrive-name@example.com" is shown as "GoogleDrive").
  const cloud = path.match(/\/Library\/CloudStorage\/([^/]+)\/?(.*)$/);
  if (cloud) return [cloud[1].split("-")[0], ...parts(cloud[2])].join(" › ");
  const home = path.match(/^\/Users\/[^/]+\/(.*)$/);
  if (home) return parts(home[1]).join(" › ") || "Home";
  return path;
}

const plural = (count: number, one: string, many: string) => `${count.toLocaleString()} ${count === 1 ? one : many}`;

export function BackupSettings() {
  const settings = useApp((s) => s.settings);
  const update = useApp((s) => s.updateSettings);
  const showToast = useApp((s) => s.showToast);
  const [status, setStatus] = useState<BackupStatus | null>(null);
  const [backingUp, setBackingUp] = useState(false);
  const [restoring, setRestoring] = useState<BackupInfo | null>(null);

  const load = () => api.backupStatus().then(setStatus).catch(() => {});
  useEffect(() => api.onBackupChanged(load), []);
  useEffect(() => {
    load();
  }, [settings.autoBackup, settings.autoBackupFolder]);

  const chooseRestore = async () => {
    try {
      const info = await api.chooseBackup();
      if (info) setRestoring(info);
    } catch (e) {
      showToast(errorText(e), "error");
    }
  };
  const chooseFolder = async () => {
    const folder = await api.chooseFolder();
    if (folder) update({ autoBackupFolder: folder });
  };
  const automatic = settings.autoBackup !== "off";

  return (
    <SettingsGroup
      title="Backup"
      note="A backup holds your watchlist, progress and history, favorites and groups, changes to channels, recording schedules, settings and sources, but not recordings themselves. Logins and links go in only when you set a passphrase for that backup. Automatic backups never include them, and Xorvio keeps the latest 7."
    >
      <Row label="Back up now">
        <button className="btn-plain" onClick={() => setBackingUp(true)}>
          Back Up…
        </button>
      </Row>
      <Row label="Restore from a backup">
        <button className="btn-plain" onClick={chooseRestore}>
          Restore…
        </button>
      </Row>
      <Row label="Back up automatically">
        <Segmented
          value={settings.autoBackup}
          options={[
            ["off", "Off"],
            ["daily", "Daily"],
            ["weekly", "Weekly"],
          ]}
          onChange={(v) => update({ autoBackup: v as AutoBackup })}
        />
      </Row>
      {automatic && (
        <>
          <Row label="Save automatic backups in">
            <div className="settings-buttons">
              <span className="muted ellipsis">{settings.autoBackupFolder ? folderLabel(settings.autoBackupFolder) : "Xorvio's data folder › Backups"}</span>
              <button className="btn-plain" onClick={chooseFolder}>
                Choose…
              </button>
              {settings.autoBackupFolder && (
                <button className="btn-plain" onClick={() => update({ autoBackupFolder: null })}>
                  Use Default
                </button>
              )}
            </div>
          </Row>
          <Row label="Last automatic backup">
            <div className="settings-buttons">
              <span className="muted">{status?.lastAutomatic ? ago(status.lastAutomatic) : "Not yet"}</span>
              <button className="btn-plain" onClick={() => api.showBackupsFolder().catch((e) => showToast(errorText(e), "error"))}>
                {showInFiles}
              </button>
            </div>
          </Row>
        </>
      )}
      {automatic && status?.automaticError && (
        <p className="form-error settings-message">
          <TriangleAlert size={13} /> {status.automaticError}
        </p>
      )}
      {status?.restoreNotice && (
        <p className={status.restoreNotice.restored ? "muted small settings-message" : "form-error settings-message"}>
          {!status.restoreNotice.restored && <TriangleAlert size={13} />} {status.restoreNotice.message}
        </p>
      )}
      {/* Over the whole window, not inside the card. */}
      {backingUp && createPortal(<BackUpSheet minLength={status?.minPassphraseLength ?? 8} onClose={() => setBackingUp(false)} />, document.body)}
      {restoring && createPortal(<RestoreSheet info={restoring} onClose={() => setRestoring(null)} />, document.body)}
    </SettingsGroup>
  );
}

/** Backing up now, with or without logins. */
function BackUpSheet({ minLength, onClose }: { minLength: number; onClose: () => void }) {
  const showToast = useApp((s) => s.showToast);
  const [withLogins, setWithLogins] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  const [again, setAgain] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const problem = !withLogins
    ? null
    : [...passphrase].length < minLength
      ? `Use at least ${minLength} characters.`
      : again !== passphrase
        ? "Type the same passphrase twice."
        : null;

  const save = async () => {
    if (problem || busy) return;
    setBusy(true);
    setError(null);
    try {
      const path = await api.backUpNow(withLogins ? passphrase : null);
      if (path) {
        showToast(`Saved ${path.split(/[\\/]/).pop()}`);
        onClose();
      }
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet
      title="Back up Xorvio"
      subtitle="One file you can keep anywhere, and restore on this computer or another."
      onClose={() => !busy && onClose()}
      width={520}
      footer={
        <>
          <button className="btn-plain" disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <button className="btn-accent" disabled={busy || !!problem} onClick={save}>
            {busy ? "Backing Up…" : "Back Up…"}
          </button>
        </>
      }
    >
      <Toggle on={withLogins} onChange={setWithLogins} label="Include logins and links, sealed with a passphrase" />
      {withLogins ? (
        <form
          className="backup-passphrase"
          onSubmit={(e) => {
            e.preventDefault();
            save();
          }}
        >
          <div className="form-row">
            <label htmlFor="backup-passphrase">Passphrase</label>
            <input id="backup-passphrase" className="text-input" type="password" value={passphrase} onChange={(e) => setPassphrase(e.target.value)} autoComplete="new-password" autoFocus />
          </div>
          <div className="form-row">
            <label htmlFor="backup-passphrase-again">Type it again</label>
            <input id="backup-passphrase-again" className="text-input" type="password" value={again} onChange={(e) => setAgain(e.target.value)} autoComplete="new-password" />
          </div>
          {passphrase && again && problem && <p className="muted small sheet-note">{problem}</p>}
          <p className="muted small sheet-note">
            Xorvio never keeps the passphrase. Without it nobody can open the logins in this backup, you included, so keep it somewhere safe. Everything else restores without it.
          </p>
          {/* Return saves from either field. */}
          <button type="submit" hidden />
        </form>
      ) : (
        <p className="muted small sheet-note">Logins and links stay out of this backup. If you restore it on another computer, your sources will ask for their logins again.</p>
      )}
      {error && (
        <p className="form-error">
          <TriangleAlert size={13} /> {error}
        </p>
      )}
    </Sheet>
  );
}

/** What a backup holds and what restoring it replaces, before anything happens. */
function RestoreSheet({ info, onClose }: { info: BackupInfo; onClose: () => void }) {
  const syncing = useApp((s) => s.settings.syncEnabled);
  const [passphrase, setPassphrase] = useState("");
  const [busy, setBusy] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { counts } = info;
  const made = new Date(info.createdAt * 1000).toLocaleString([], { dateStyle: "long", timeStyle: "short" });
  const facts = [
    `${plural(counts.sources, "source", "sources")}${counts.addons ? ` and ${plural(counts.addons, "add-on", "add-ons")}` : ""}`,
    `${plural(counts.watchlist, "title", "titles")} on the watchlist`,
    `${plural(counts.progress, "film or episode", "films and episodes")} with progress, and ${plural(counts.history, "entry", "entries")} in viewing history`,
    `${plural(counts.favorites, "favorite", "favorites")}${counts.lists ? ` and ${plural(counts.lists, "group", "groups")}` : ""}`,
    counts.itemEdits ? `${plural(counts.itemEdits, "channel or category", "channels and categories")} renamed, hidden or locked` : null,
    counts.recordings ? `${plural(counts.recordings, "recording", "recordings")}${counts.scheduledRecordings ? `, ${counts.scheduledRecordings.toLocaleString()} still to come` : ""}` : null,
  ].filter((fact): fact is string => !!fact);
  const recording = info.recordingNow > 0;

  const restore = async () => {
    if (recording || busy) return;
    setBusy(true);
    setError(null);
    try {
      await api.restoreBackup(info.path, info.includesSecrets && passphrase ? passphrase : null);
      setRestarting(true);
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Sheet
      title="Restore this backup?"
      subtitle={`${info.fileName}, made on ${info.computer || "another computer"} on ${made}.`}
      onClose={() => !busy && onClose()}
      width={540}
      footer={
        restarting ? undefined : (
          <>
            <button className="btn-plain" disabled={busy} onClick={onClose}>
              Cancel
            </button>
            <button className="btn-accent danger" disabled={busy || recording} onClick={restore}>
              {busy ? "Restoring…" : "Restore and Restart"}
            </button>
          </>
        )
      }
    >
      {restarting ? (
        <div className="backup-restarting">
          <div className="spinner small" />
          <span>Xorvio is restarting to restore the backup…</span>
        </div>
      ) : (
        <>
          <ul className="backup-facts">
            {facts.map((fact) => (
              <li key={fact}>{fact}</li>
            ))}
          </ul>
          <p className="sheet-note">
            Restoring replaces this computer's library, recording schedules, settings and sources with the backup's. Recordings already made stay where they are. Xorvio restarts to do it.
          </p>
          {info.scheduledHere > 0 && (
            <p className="muted small sheet-note">
              {info.scheduledHere === 1 ? "The recording scheduled on this computer is" : `The ${info.scheduledHere} recordings scheduled on this computer are`} replaced by the backup's schedule.
            </p>
          )}
          {syncing && <p className="muted small sheet-note">Sync is on, so anything newer on your other computers comes back at the next sync.</p>}
          {info.includesSecrets ? (
            <form
              className="backup-passphrase"
              onSubmit={(e) => {
                e.preventDefault();
                restore();
              }}
            >
              <p className="muted small sheet-note">This backup includes logins and links. Enter its passphrase to restore them too, or leave it empty to restore everything else.</p>
              <div className="form-row">
                <label htmlFor="restore-passphrase">Passphrase</label>
                <input id="restore-passphrase" className="text-input" type="password" value={passphrase} onChange={(e) => setPassphrase(e.target.value)} autoComplete="off" autoFocus />
              </div>
              <button type="submit" hidden />
            </form>
          ) : (
            <p className="muted small sheet-note">This backup has no logins. Sources keep the ones this computer has; any it doesn't have will ask for theirs.</p>
          )}
          {recording && (
            <p className="form-error">
              <TriangleAlert size={13} /> Xorvio is recording right now. Restore once the recording has finished, or stop it in Recordings.
            </p>
          )}
          {error && (
            <p className="form-error">
              <TriangleAlert size={13} /> {error}
            </p>
          )}
        </>
      )}
    </Sheet>
  );
}

export function SyncSettings() {
  const settings = useApp((s) => s.settings);
  const update = useApp((s) => s.updateSettings);
  const [status, setStatus] = useState<SyncStatus | null>(null);

  useEffect(() => api.onSyncChanged(setStatus), []);
  // The backend notices a change of setting within a few seconds and syncs.
  useEffect(() => {
    api.syncStatus().then(setStatus).catch(() => {});
  }, [settings.syncEnabled, settings.syncFolder]);

  const chooseFolder = async () => {
    const folder = await api.chooseFolder();
    if (folder) update({ syncFolder: folder });
    return folder;
  };
  const turn = async (on: boolean) => {
    if (on && !settings.syncFolder) {
      const folder = await api.chooseFolder();
      if (folder) update({ syncEnabled: true, syncFolder: folder });
      return;
    }
    update({ syncEnabled: on });
  };
  const syncNow = () => api.syncNow().then(setStatus).catch(() => {});
  const on = settings.syncEnabled && !!settings.syncFolder;

  return (
    <SettingsGroup
      title="Sync"
      note="Your watchlist, progress and history, favorites and groups, changes to channels, and library profiles with their Home layouts go between your computers through a folder they share: one in iCloud Drive or Dropbox, a Syncthing folder or a network share. There's no Xorvio server. Logins, sources, settings and the parental PIN stay on each computer. Titles match when computers have the same sources, so set up another computer by restoring a backup from this one, then turn on sync on both."
    >
      <Row label="Sync this computer">
        <Toggle on={settings.syncEnabled} onChange={turn} label={`As “${status?.thisComputer ?? "this computer"}”`} />
      </Row>
      <Row label="Sync folder">
        <div className="settings-buttons">
          <span className="muted ellipsis" title={settings.syncFolder ?? undefined}>
            {settings.syncFolder ? folderLabel(settings.syncFolder) : "None chosen"}
          </span>
          <button className="btn-plain" onClick={chooseFolder}>
            Choose…
          </button>
        </div>
      </Row>
      {on && status && (
        <>
          <Row label="Last sync">
            <div className="settings-buttons">
              <span className="muted">{status.syncing ? "Syncing…" : status.lastSync ? ago(status.lastSync) : "Not yet"}</span>
              <button className="btn-plain" disabled={status.syncing} onClick={syncNow}>
                <RefreshCw size={13} /> Sync Now
              </button>
            </div>
          </Row>
          <Row label="Other computers">
            {status.computers.length === 0 ? (
              <span className="muted">{status.lastSync ? "None yet. Turn on sync on another computer with this folder." : "…"}</span>
            ) : (
              <ul className="sync-computers">
                {status.computers.map((computer) => (
                  <li key={`${computer.name}|${computer.lastSeen}`}>
                    <Laptop size={13} /> {computer.name} <span className="muted small">{ago(computer.lastSeen)}</span>
                  </li>
                ))}
              </ul>
            )}
          </Row>
        </>
      )}
      {on && status?.lastError && (
        <p className="form-error settings-message">
          <TriangleAlert size={13} /> {status.lastError}
        </p>
      )}
      {on && status?.notice && !status.lastError && <p className="muted small settings-message">{status.notice}</p>}
    </SettingsGroup>
  );
}
