import { CircleAlert, Clapperboard, Copy, EyeOff, Film, Info, Lock, OctagonX, Puzzle, RefreshCw, Trash2, TriangleAlert, Tv, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { confirmAction } from "../components/Confirm";
import { Sheet } from "../components/Sheet";
import { api, errorText, isTauri } from "../lib/api";
import { editPin, lockAgain, resetName, setHidden, setLocked, showAllHidden, showLocked } from "../lib/channelEdits";
import { ago, clockTime, fileSize } from "../lib/format";
import { imageSrc } from "../lib/images";
import { LANGUAGES } from "../lib/languages";
import { isMac, isWindows, keychainName, recordingsFolderName, showInFiles, thisComputer, trayName } from "../lib/platform";
import { useActiveProfile, useProfiles } from "../lib/profiles";
import { useApp } from "../lib/store";
import type { AddonInfo, Appearance, Density, DiagnosticEvent, DiagnosticLevel, EditKind, HistoryEntry, ItemEdit, LaunchScreen, NotificationPermission, ProgramGenre, Settings, SourceSummary, StorageUsage, TmdbStatus, TraktOptions, TraktSignInStart, TraktStatus } from "../lib/types";
import { BackupSettings, SyncSettings } from "./SettingsBackup";

/** What TMDB's terms ask every app using it to say. */
const TMDB_NOTICE = "This product uses the TMDB API but is not endorsed or certified by TMDB.";

/** The kinds of programme the guide tints, as its legend lists them. */
const GENRES: [ProgramGenre, string][] = [
  ["film", "Films"],
  ["sport", "Sport"],
  ["news", "News"],
  ["entertainment", "Series & shows"],
  ["factual", "Documentaries"],
  ["kids", "Children"],
  ["music", "Music"],
];

export function SettingsScreen() {
  const settings = useApp((s) => s.settings);
  const update = useApp((s) => s.updateSettings);
  const set = <K extends keyof Settings>(key: K) => (value: Settings[K]) => update({ [key]: value } as Partial<Settings>);

  return (
    <div className="content">
      <div className="page settings-page">
        <h1 className="page-title">Settings</h1>

        <SettingsGroup title="General" note="Last Channel opens Live TV playing the channel you watched last.">
          <Row label="When Xorvio opens">
            <Segmented
              value={settings.launchScreen}
              options={[["home", "Home"], ["live", "Live TV"], ["watchlist", "Watchlist"], ["lastChannel", "Last Channel"]]}
              onChange={(v) => set("launchScreen")(v as LaunchScreen)}
            />
          </Row>
          <Row label="Appearance">
            <Segmented value={settings.appearance} options={[["system", "Match System"], ["dark", "Dark"], ["light", "Light"]]} onChange={(v) => set("appearance")(v as Appearance)} />
          </Row>
          <Row label="Artwork size">
            <Segmented value={settings.density} options={[["comfortable", "Comfortable"], ["compact", "Compact"]]} onChange={(v) => set("density")(v as Density)} />
          </Row>
        </SettingsGroup>

        <SettingsGroup title="Recording" note={`Xorvio records with ffmpeg, copying the stream as it arrives, so recordings keep the channel's own quality. Scheduled recordings need Xorvio running; it keeps going in ${trayName} when you close the window. If a folder isn't there when a recording starts, such as one on a drive that's unplugged, the recording is saved in the next one along and says so. Once a recording finishes, Xorvio looks for its ad breaks in the background (with comskip, when it's installed), leaving the file as it is; it errs towards missing a break rather than skipping any of the programme.`}>
          <Row label="Start early by">
            <Stepper value={settings.recordPaddingBefore} unit="min" min={0} max={30} onChange={set("recordPaddingBefore")} />
          </Row>
          <Row label="Keep going for">
            <Stepper value={settings.recordPaddingAfter} unit="min" min={0} max={120} onChange={set("recordPaddingAfter")} />
          </Row>
          <Row label="Save recordings in">
            <FolderChoice folder={settings.recordingsFolder} placeholder={recordingsFolderName} resetLabel={`Use ${recordingsFolderName}`} onChange={set("recordingsFolder")} />
          </Row>
          <SourceFolders folders={settings.sourceRecordingFolders ?? {}} onChange={set("sourceRecordingFolders")} />
          <Row label={isMac ? "Menu bar" : "System tray"}>
            <Toggle on={settings.keepRunningForRecordings} onChange={set("keepRunningForRecordings")} label="Keep running while recordings or reminders are scheduled" />
          </Row>
          <Row label="Ad breaks">
            <Segmented
              value={settings.commercialSkip}
              options={[
                ["off", "Off"],
                ["button", "Show a Skip button"],
                ["automatic", "Skip automatically"],
              ]}
              onChange={(v) => set("commercialSkip")(v as Settings["commercialSkip"])}
            />
          </Row>
        </SettingsGroup>

        <ReminderSettings />

        <SettingsGroup title="Playback" note="Languages apply when a film or episode has a choice. Skip times come from IntroDB, a free, community-timed database; only the series' IMDb id and the episode number are sent, and nothing at all with Skip times off.">
          <Row label="Audio">
            <select className="select-input settings-select" value={settings.audioLanguage ?? ""} onChange={(e) => set("audioLanguage")(e.target.value || null)}>
              <option value="">The film's own</option>
              {LANGUAGES.map(([code, name]) => (
                <option key={code} value={code}>
                  {name}
                </option>
              ))}
            </select>
          </Row>
          <Row label="Subtitles">
            <select className="select-input settings-select" value={settings.subtitleLanguage ?? ""} onChange={(e) => set("subtitleLanguage")(e.target.value || null)}>
              <option value="">Off</option>
              {LANGUAGES.map(([code, name]) => (
                <option key={code} value={code}>
                  {name}
                </option>
              ))}
            </select>
          </Row>
          <Row label="Subtitle size">
            <Segmented value={settings.subtitleSize} options={[["small", "Small"], ["medium", "Medium"], ["large", "Large"]]} onChange={(v) => set("subtitleSize")(v as Settings["subtitleSize"])} />
          </Row>
          <Row label="Next episode">
            <Toggle on={settings.autoplayNextEpisode} onChange={set("autoplayNextEpisode")} label="Play it automatically when an episode ends" />
          </Row>
          <Row label="Skip times">
            <Toggle on={settings.introDb} onChange={set("introDb")} label="Find intros, recaps and credits on IntroDB" />
          </Row>
          {settings.introDb && (
            <Row label="Intros and recaps">
              <Toggle on={settings.skipIntrosAutomatically} onChange={set("skipIntrosAutomatically")} label="Skip them without asking" />
            </Row>
          )}
          {isTauri && (
            <Row label="Pop-out player">
              <Toggle
                on={settings.keepPopOutOnTop}
                onChange={(on) => {
                  set("keepPopOutOnTop")(on);
                  api.setPopOutOnTop(on).catch(() => {});
                }}
                label="Keep it above other windows"
              />
            </Row>
          )}
        </SettingsGroup>

        <SettingsGroup
          title="Live TV Latency"
          note={`How close to live a channel plays when it isn't buffered. Lower means less delay behind the broadcast, but more stalls on a weak connection.${isMac ? " On a Mac, channels your source sends as HLS play in Apple's own player, which sets its own delay, so this applies to MPEG-TS channels there." : ""} Films and episodes always buffer enough to play without stopping.`}
        >
          <Row label="Latency">
            <Segmented
              value={settings.liveLatency}
              options={[["automatic", "Automatic"], ["low", "Low"], ["minimal", "Minimal"]]}
              onChange={(v) => set("liveLatency")(v as Settings["liveLatency"])}
            />
          </Row>
        </SettingsGroup>

        <SettingsGroup
          title="Pause and Rewind Live TV"
          note={`Live channels can play from a buffer kept on ${thisComputer}, so you can pause, rewind, skip ahead and go back to live. From a pause, the buffer starts when you pause; always, it starts when you tune in, so you can rewind straight away, though channels take a few seconds longer to start. An hour of HD takes 2 to 4 GB, and it's deleted when you change channel.`}
        >
          <Row label="Buffer">
            <Segmented
              value={settings.liveBuffer}
              options={[["off", "Off"], ["paused", "From a Pause"], ["always", "Always"]]}
              onChange={(v) => set("liveBuffer")(v as Settings["liveBuffer"])}
            />
          </Row>
          <Row label="Keep up to">
            <Segmented
              value={String(settings.liveBufferMinutes)}
              options={[["15", "15 min"], ["30", "30 min"], ["60", "1 hour"], ["120", "2 hours"]]}
              onChange={(v) => set("liveBufferMinutes")(Number(v))}
            />
          </Row>
        </SettingsGroup>

        <SettingsGroup
          title="Guide"
          note="Colours come from each programme's category in the guide; programmes it doesn't recognise stay plain. Tidying takes prefixes such as “UK |” and tags such as “FHD” off channel and category names; your source's own names are kept, and names you give always win. Grouping shows a channel your source lists more than once (HD, FHD, 4K, backups) as one row that plays the best version. Right-click it to play another or always play one."
        >
          <Row label="Hours shown">
            <Segmented value={String(settings.guideHours)} options={[["2", "2"], ["3", "3"], ["4", "4"]]} onChange={(v) => set("guideHours")(Number(v))} />
          </Row>
          <Row label="Channel numbers">
            <Toggle on={settings.showChannelNumbers} onChange={set("showChannelNumbers")} />
          </Row>
          <Row label="Programme colours">
            <Segmented value={settings.guideColors} options={[["category", "By category"], ["plain", "Plain"]]} onChange={(v) => set("guideColors")(v as Settings["guideColors"])} />
          </Row>
          {settings.guideColors === "category" && (
            <div className="genre-legend" aria-label="Programme colours">
              {GENRES.map(([genre, label]) => (
                <span key={genre}>
                  <span className={`genre-swatch genre-${genre}`} />
                  {label}
                </span>
              ))}
            </div>
          )}
          <Row label="Names">
            <Toggle on={settings.tidyNames} onChange={(on) => saveThenReload({ tidyNames: on })} label="Tidy channel and category names" />
          </Row>
          <Row label="Group similar channels">
            <Toggle on={settings.groupSimilarChannels} onChange={set("groupSimilarChannels")} />
          </Row>
        </SettingsGroup>

        <ProfileSettings />

        <ParentalSettings />

        <HiddenSettings />

        <DetailsSettings tmdb={settings.tmdb} onTmdb={set("tmdb")} tvmaze={settings.tvmaze} onTvmaze={set("tvmaze")} />

        <AddonsSettings />

        <TraktSettings />

        <SwiftImportSettings />

        <BackupSettings />

        <SyncSettings />

        <StorageSettings />

        <DiagnosticsSettings />

        <SettingsGroup title="About">
          <p className="settings-about">
            Xorvio is free, open-source software (GPL-3.0). It plays only the sources you add and ships no TV content of its own.
            {isTauri ? "" : " You're looking at the browser preview with the demo library."}
          </p>
          <p className="settings-about settings-credits">Film and series details from TMDB and TVmaze. {TMDB_NOTICE}</p>
        </SettingsGroup>
      </div>
    </div>
  );
}

function AddonsSettings() {
  const settings = useApp((s) => s.settings);
  const update = useApp((s) => s.updateSettings);
  const [addons, setAddons] = useState<AddonInfo[]>([]);
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = () => api.addons().then(setAddons).catch(() => {});
  useEffect(() => {
    load();
  }, []);

  const install = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.installAddon(url.trim());
      setUrl("");
      load();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsGroup
      title="Add-ons"
      note={`Add-ons that speak the Stremio add-on protocol can add catalogues, details, copies and subtitles. Paste an add-on's manifest link to install it. Links can contain account keys, so they're kept in ${keychainName} and only the host is shown.`}
    >
      <div className="addon-install">
        <input className="text-input" placeholder="https://…/manifest.json" value={url} onChange={(e) => setUrl(e.target.value)} onKeyDown={(e) => e.key === "Enter" && url && install()} spellCheck={false} />
        <button className="btn-accent" disabled={!url.trim() || busy} onClick={install}>
          {busy ? "Installing…" : "Install"}
        </button>
      </div>
      {error && (
        <p className="form-error">
          <TriangleAlert size={13} /> {error}
        </p>
      )}
      {addons.map((a) => (
        <div key={a.id} className="addon-row">
          {a.logoUrl ? <img className="addon-logo" src={imageSrc(a.logoUrl)} alt="" /> : <Puzzle className="addon-logo" size={22} />}
          <div className="addon-text">
            <div className="addon-name">
              {a.name} {a.version && <span className="muted small">{a.version}</span>}
            </div>
            <div className="muted small">
              {a.host} · {a.resources.join(", ")}
              {a.catalogs.length ? ` · ${a.catalogs.length} catalogue${a.catalogs.length === 1 ? "" : "s"}` : ""}
            </div>
          </div>
          <Toggle on={a.enabled} onChange={(on) => api.setAddonEnabled(a.id, on).then(load)} />
          <button className="icon-button" aria-label={`Remove ${a.name}`} onClick={async () => (await confirmAction({ title: `Remove ${a.name}?`, message: "Its catalogues and copies leave Xorvio. You can install it again with its link.", confirmLabel: "Remove", destructive: true })) && api.removeAddon(a.id).then(load)}>
            <Trash2 size={15} />
          </button>
        </div>
      ))}
      {addons.length > 0 && (
        <Row label="Lead with add-ons">
          <Toggle on={settings.addonsLead} onChange={(on) => update({ addonsLead: on })} label="Show add-on titles first in Home, search and copies" />
        </Row>
      )}
    </SettingsGroup>
  );
}

/** Library profiles and Home's rows, each in a sheet of its own. */
function ProfileSettings() {
  const profile = useActiveProfile();
  const previewClips = useApp((s) => s.settings.previewClips);
  const titleTrailers = useApp((s) => s.settings.titleTrailers);
  const count = useProfiles((s) => s.profiles.length);
  const { editProfiles, customizeHome } = useProfiles.getState();
  return (
    <SettingsGroup
      title="Profiles and Home"
      note={`A profile chooses which languages, genres and kinds of content show everywhere: Home, Live TV and the guide, Movies, Series, search and Discover. Switch profiles from the foot of the sidebar. Each profile has its own Home rows. The parental PIN keeps locked categories locked whatever the profile. A preview clip plays behind the featured film's title after a few seconds; it streams from your source, so it waits until nothing else plays and stops before anything does. A film or series page plays its trailer from YouTube behind the title, muted until you turn the sound on, and never while something else plays; with ${reduceMotion}, neither moves.`}
    >
      <Row label="Profile in use">
        <div className="settings-buttons">
          <span className="muted">{profile ? `${profile.name}${count > 1 ? ` (${count} profiles)` : ""}` : ""}</span>
          <button className="btn-plain" onClick={() => editProfiles()}>
            Edit profiles…
          </button>
        </div>
      </Row>
      <Row label="Home">
        <button className="btn-plain" onClick={() => customizeHome(true)}>
          Customize Home…
        </button>
      </Row>
      <Row label="Preview clips">
        <Toggle on={previewClips} onChange={(on) => useApp.getState().updateSettings({ previewClips: on })} label="Play a short muted clip of the featured film on Home" />
      </Row>
      <Row label="Trailers">
        <Toggle on={titleTrailers} onChange={(on) => useApp.getState().updateSettings({ titleTrailers: on })} label="Play trailers behind film and series pages" />
      </Row>
    </SettingsGroup>
  );
}

/** "Allowed", or what to do about it. */
const PERMISSION_TEXT: Record<NotificationPermission, string> = {
  allowed: "Allowed",
  denied: "Turned off in System Settings",
  notDetermined: "Not asked yet: send a test reminder to choose",
  unknown: "Set in your system's notification settings",
};

/** Where the system's notification settings are, for Reminders' note. */
const notificationSettings = isMac
  ? "Banners, sounds and Focus for them are set in System Settings › Notifications."
  : isWindows
    ? "Banners and sounds for them are set in Settings › System › Notifications."
    : "Banners and sounds for them are set in your desktop's notification settings.";

/** The system's switch that stills moving backdrops (prefers-reduced-motion). */
const reduceMotion = isMac ? "Reduce Motion on in macOS" : isWindows ? "animation effects off in Windows" : "animations off in your desktop's settings";

/** When reminders go out, new episodes, and whether the system lets Xorvio notify, as Xorvio for Mac's. */
function ReminderSettings() {
  const settings = useApp((s) => s.settings);
  const update = useApp((s) => s.updateSettings);
  const [permission, setPermission] = useState<NotificationPermission | null>(null);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);

  // Looked at again on coming back, from System Settings perhaps.
  useEffect(() => {
    const check = () => api.notificationPermission().then(setPermission).catch(() => setPermission("unknown"));
    check();
    window.addEventListener("focus", check);
    return () => window.removeEventListener("focus", check);
  }, []);

  const test = async () => {
    setMessage(null);
    try {
      await api.sendTestReminder();
      setMessage({ text: "A test reminder will appear in a moment.", error: false });
    } catch (e) {
      setMessage({ text: errorText(e), error: true });
    }
    api.notificationPermission().then(setPermission).catch(() => {});
  };

  return (
    <SettingsGroup
      title="Reminders"
      note={`Programmes you ask to be reminded of get a notification this long before they start; click it to watch the channel. Series on your watchlist, and series you're caught up on (mark a season watched from its page), are checked for new episodes every few hours and marked New Episode. Xorvio sends notifications while it's open, or running in ${trayName}. ${notificationSettings}`}
    >
      <Row label="Remind me">
        <Segmented
          value={String(settings.reminderMinutes)}
          options={[["0", "At start"], ["1", "1 min"], ["5", "5 min"], ["10", "10 min"], ["15", "15 min"]]}
          onChange={(v) => update({ reminderMinutes: Number(v) })}
        />
      </Row>
      <Row label="New episodes">
        <Toggle on={settings.newEpisodeAlerts} onChange={(on) => update({ newEpisodeAlerts: on })} label="Tell me when new episodes arrive" />
      </Row>
      <Row label="Notifications">
        <div className="settings-buttons">
          {permission && (
            <span className="muted" style={permission === "denied" ? { color: "var(--warning)" } : undefined}>
              {PERMISSION_TEXT[permission]}
            </span>
          )}
          <button className="btn-plain" onClick={test}>
            Send a Test Reminder
          </button>
          <button className="btn-plain" onClick={() => api.openNotificationSettings().catch((e) => setMessage({ text: errorText(e), error: true }))}>
            Notification Settings…
          </button>
        </div>
      </Row>
      {message && (
        <p className="muted small" style={{ margin: 0, padding: "8px 14px 10px", color: message.error ? "var(--warning)" : undefined }}>
          {message.text}
        </p>
      )}
    </SettingsGroup>
  );
}

/**
 * Saves settings the backend applies to every list (names, the PIN's time),
 * then reloads what's shown so lists follow at once.
 */
async function saveThenReload(patch: Partial<Settings>) {
  const settings = { ...useApp.getState().settings, ...patch };
  useApp.setState({ settings });
  await api.saveSettings(settings).catch((e) => useApp.getState().showToast(errorText(e), "error"));
  await useApp.getState().loadEdits().catch(() => {});
}

function ParentalSettings() {
  const parental = useApp((s) => s.parental);
  const lockMinutes = useApp((s) => s.settings.parentalLockMinutes);
  const count = parental.lockedCount;
  const locked = count === 0 ? "Nothing yet" : count === 1 ? "1 channel or category" : `${count} channels and categories`;
  const showing = parental.locksAt ? `${locked}, showing until ${clockTime(parental.locksAt)}` : `${locked}, showing for now`;
  return (
    <SettingsGroup
      title="Parental Controls"
      note="With a PIN, categories whose names look adult (XXX, Adults, 18+…) lock themselves. Lock or unlock others by right-clicking them in Live TV. Locked channels and categories stay out of Live TV, the guide, Home and search until the PIN is entered, then lock again after the time chosen, when Xorvio quits, or with Lock Now. Only a scrambled form of the PIN is stored."
    >
      <Row label="PIN">
        {parental.hasPin ? (
          <div className="settings-buttons">
            <button className="btn-plain" onClick={() => editPin("change")}>
              Change PIN…
            </button>
            <button className="btn-plain" onClick={() => editPin("remove")}>
              Remove PIN…
            </button>
          </div>
        ) : (
          <button className="btn-plain" onClick={() => editPin("create")}>
            Set PIN…
          </button>
        )}
      </Row>
      {parental.hasPin && (
        <Row label="Lock again after">
          <Segmented
            value={lockMinutes === null ? "quit" : String(lockMinutes)}
            options={[["5", "5 min"], ["15", "15 min"], ["60", "1 hour"], ["240", "4 hours"], ["quit", "Xorvio Quits"]]}
            onChange={(v) => saveThenReload({ parentalLockMinutes: v === "quit" ? null : Number(v) })}
          />
        </Row>
      )}
      {parental.hasPin && (
        <Row label="Locked">
          <div className="settings-buttons">
            <span className="muted">{parental.unlocked ? showing : locked}</span>
            {parental.unlocked ? (
              <button className="btn-plain" onClick={lockAgain}>
                Lock Now
              </button>
            ) : (
              parental.lockedCount > 0 && (
                <button className="btn-plain" onClick={showLocked}>
                  Show Locked…
                </button>
              )
            )}
          </div>
        </Row>
      )}
    </SettingsGroup>
  );
}

const editKindNames: Record<EditKind, string> = {
  channel: "Channel",
  liveCategory: "Live TV category",
  movieCategory: "Movies category",
  seriesCategory: "Series category",
  movie: "Film",
  series: "Series",
};
const editKindOrder: EditKind[] = ["liveCategory", "channel", "movieCategory", "seriesCategory", "movie", "series"];

/** Everything renamed, hidden or locked, to undo one at a time or all hidden at once. */
function HiddenSettings() {
  const edits = useApp((s) => s.edits);
  const parental = useApp((s) => s.parental);
  const items = [...edits.values()].sort(
    (a, b) => editKindOrder.indexOf(a.kind) - editKindOrder.indexOf(b.kind) || (a.displayName ?? a.originalName).localeCompare(b.displayName ?? b.originalName),
  );
  const hiddenCount = items.filter((e) => e.hidden).length;
  return (
    <SettingsGroup
      title="Hidden & Locked"
      note={`Right-click a channel or category in Live TV, or a category in Movies or Series, to rename, hide or lock it, and a film or series to hide it from the library. Renames and hiding last through refreshes; Reset Name goes back to your source's name.${parental.hasPin && !parental.unlocked ? " Locked ones appear here once the PIN is entered." : ""}`}
    >
      {items.length === 0 ? (
        <p className="muted small settings-empty">Nothing is renamed, hidden or locked.</p>
      ) : (
        <>
          {hiddenCount > 0 && (
            <Row label={`${hiddenCount} hidden`}>
              <button className="btn-plain" onClick={showAllHidden}>
                Show All
              </button>
            </Row>
          )}
          {items.map((edit) => (
            <EditRow key={edit.itemId} edit={edit} />
          ))}
        </>
      )}
    </SettingsGroup>
  );
}

function EditRow({ edit }: { edit: ItemEdit }) {
  const item = { id: edit.itemId, name: edit.displayName ?? edit.originalName };
  const facts = [editKindNames[edit.kind]];
  if (edit.displayName) facts.push(`Your source calls it “${edit.originalName}”`);
  if (edit.lockedAutomatically) facts.push("Locked because its name looks adult");
  return (
    <div className="edit-row">
      <div className="edit-text">
        <div className="edit-name">
          {item.name}
          {edit.hidden && <EyeOff size={12} aria-label="Hidden" />}
          {edit.locked && <Lock size={12} aria-label="Locked" />}
        </div>
        <div className="muted small">{facts.join(" · ")}</div>
      </div>
      <div className="settings-buttons">
        {edit.hidden && (
          <button className="btn-plain" onClick={() => setHidden(item, false)}>
            Show
          </button>
        )}
        {edit.displayName && (
          <button className="btn-plain" onClick={() => resetName(item)}>
            Reset Name
          </button>
        )}
        {edit.locked && (
          <button className="btn-plain" onClick={() => setLocked(item, false)}>
            Unlock
          </button>
        )}
      </div>
    </div>
  );
}

/** Bringing watch history over from the Swift app, any time. */
function SwiftImportSettings() {
  const [available, setAvailable] = useState(false);
  const [busy, setBusy] = useState(false);
  const showToast = useApp((s) => s.showToast);
  useEffect(() => {
    api.swiftDataAvailable().then(setAvailable).catch(() => {});
  }, []);
  if (!available) return null;
  return (
    <SettingsGroup title="Xorvio for Mac" note="Copies where you got to in each film and episode, and what you finished, from Xorvio and Xorvio Personal. The newest record wins, so nothing watched here is rolled back. The Mac apps' data is only read.">
      <Row label="Watch history">
        <button
          className="btn-plain"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            try {
              const result = await api.importSwiftProgress();
              showToast(`Brought over ${result.imported} films and episodes${result.skipped ? ` (${result.skipped} from sources that aren't here)` : ""}.`);
              useApp.getState().loadLibrary();
            } catch (e) {
              showToast(errorText(e), "error");
            } finally {
              setBusy(false);
            }
          }}
        >
          {busy ? "Importing…" : "Import"}
        </button>
      </Row>
    </SettingsGroup>
  );
}

/** What Xorvio keeps on this computer, and clearing what it can load again. */
function StorageSettings() {
  const [usage, setUsage] = useState<StorageUsage | null>(null);
  const [clearing, setClearing] = useState<"guide" | "temporary" | null>(null);
  const showToast = useApp((s) => s.showToast);
  const load = () => api.storageUsage().then(setUsage).catch(() => {});
  useEffect(() => {
    load();
  }, []);

  const size = (bytes: number | undefined) => (bytes === undefined ? "…" : bytes ? fileSize(bytes) : "None");
  const clear = async (what: "guide" | "temporary") => {
    const ok = await confirmAction(
      what === "guide"
        ? {
            title: "Clear guide listings?",
            message: "Every guide's programmes are deleted to free their space. They come back the next time each source or guide refreshes; refresh one in Sources to load it now. Your watchlist, reminders and recordings stay.",
            confirmLabel: "Clear Listings",
            destructive: true,
          }
        : { title: "Clear temporary files?", message: "Subtitles kept while converted streams play, and live TV buffers left from earlier, are deleted. What's playing now keeps its own.", confirmLabel: "Clear", destructive: true },
    );
    if (!ok) return;
    setClearing(what);
    try {
      if (what === "guide") {
        await api.clearGuideListings();
        showToast("Guide listings cleared. They come back as the guides refresh.");
      } else {
        const playing = useApp.getState().stream?.url;
        await api.clearTemporaryFiles(playing ? [playing] : []);
      }
    } catch (e) {
      showToast(errorText(e), "error");
    }
    setClearing(null);
    load();
  };

  return (
    <SettingsGroup title="Storage" note="The catalog holds what your sources and guides send, so refreshing loads it again. The library holds what you've made: your watchlist, progress, favorites and groups. Recordings are deleted only in Recordings.">
      <Row label="Catalog">
        <div className="settings-buttons">
          <span className="muted">
            {size(usage?.catalogBytes)}
            {usage?.guideListings ? ` · ${usage.guideListings.toLocaleString()} guide listings` : ""}
          </span>
          <button className="btn-plain" disabled={!usage?.guideListings || !!clearing} onClick={() => clear("guide")}>
            {clearing === "guide" ? "Clearing…" : "Clear Guide Listings…"}
          </button>
        </div>
      </Row>
      <Row label="Library">
        <span className="muted">{size(usage?.libraryBytes)}</span>
      </Row>
      <Row label="Recordings">
        <div className="settings-buttons">
          <span className="muted">{!usage ? "…" : usage.recordingCount ? `${fileSize(usage.recordingsBytes)} · ${usage.recordingCount} recording${usage.recordingCount === 1 ? "" : "s"}` : "None"}</span>
          <button className="btn-plain" onClick={() => api.showRecordingsFolder().catch((e) => showToast(errorText(e), "error"))}>
            {showInFiles}
          </button>
        </div>
      </Row>
      <Row label="Temporary files">
        <div className="settings-buttons">
          <span className="muted">{size(usage?.temporaryBytes)}</span>
          <button className="btn-plain" disabled={!usage?.temporaryBytes || !!clearing} onClick={() => clear("temporary")}>
            {clearing === "temporary" ? "Clearing…" : "Clear Temporary Files…"}
          </button>
        </div>
      </Row>
    </SettingsGroup>
  );
}

/**
 * Trakt: connecting with a code entered at trakt.tv/activate, what's synced,
 * and (for builds without one) the Trakt app to connect through.
 */
function DetailsSettings({ tmdb, onTmdb, tvmaze, onTvmaze }: { tmdb: boolean; onTmdb: (on: boolean) => void; tvmaze: boolean; onTvmaze: (on: boolean) => void }) {
  const [status, setStatus] = useState<TmdbStatus | null>(null);
  const [key, setKey] = useState("");
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.tmdbStatus().then(setStatus).catch(() => {});
  }, []);

  const save = async (value: string) => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await api.tmdbSetKey(value));
      setKey("");
      setEditing(false);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  if (!status) return null;
  const using = status.customKey ? "your key" : status.builtIn ? "the built-in key" : null;
  const showForm = !using || editing;
  return (
    <SettingsGroup
      title="Details"
      note={`TMDB and TVmaze fill in what your sources don't send. Only a title's name and year, or its TMDB or IMDb id, are sent, never the source it came from. TVmaze is asked about series only when TMDB can't help. Switched off, neither is asked anything, and what they said before still shows. ${TMDB_NOTICE}`}
    >
      <Row label="TMDB">
        <Toggle on={tmdb} onChange={onTmdb} label="Look up details, artwork and cast" />
      </Row>
      {tmdb && (
        <Row label="TMDB key">
          <div className="trakt-line">
            <span className={using ? "" : "muted"}>{using ? `Using ${using}` : "Not set up"}</span>
            {using && !editing && (
              <span className="tmdb-buttons">
                <button className="btn-plain" disabled={busy} onClick={() => setEditing(true)}>
                  {status.customKey ? "Change" : "Use Your Own"}
                </button>
                {status.customKey && (
                  <button className="btn-plain" disabled={busy} onClick={() => save("")}>
                    Remove
                  </button>
                )}
              </span>
            )}
          </div>
        </Row>
      )}
      {tmdb && showForm && (
        <div className="trakt-app">
          <p className="muted small">
            With a free key from themoviedb.org, Xorvio adds title logos, backdrops, cast photos, trailers, episode stills, people's pages and better suggestions to films and series. Find yours under Settings › API on themoviedb.org: the API key or the read access token both work.
          </p>
          <input
            className="text-input"
            type="password"
            placeholder="API key or read access token"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && key.trim() && !busy && save(key)}
            spellCheck={false}
            autoComplete="off"
            aria-label="TMDB key"
          />
          <div className="trakt-actions">
            <button className="btn-accent" disabled={busy || !key.trim()} onClick={() => save(key)}>
              {busy ? "Checking…" : "Use This Key"}
            </button>
            <button className="btn-plain" onClick={() => api.openExternal("https://www.themoviedb.org/settings/api").catch((e) => setError(errorText(e)))}>
              Get a Key
            </button>
            {editing && (
              <button className="btn-plain" onClick={() => setEditing(false)}>
                Cancel
              </button>
            )}
          </div>
        </div>
      )}
      {tmdb && status.rejected && !error && (
        <p className="form-error trakt-error">
          <TriangleAlert size={13} /> TMDB turned the key down. Enter it again, or check it under Settings › API on themoviedb.org.
        </p>
      )}
      {error && (
        <p className="form-error trakt-error">
          <TriangleAlert size={13} /> {error}
        </p>
      )}
      <Row label="TVmaze">
        <Toggle on={tvmaze} onChange={onTvmaze} label="Look up missing series details" />
      </Row>
    </SettingsGroup>
  );
}

function TraktSettings() {
  const rowsWithoutAccount = useApp((s) => s.settings.traktRowsWithoutAccount);
  const updateSettings = useApp((s) => s.updateSettings);
  const [status, setStatus] = useState<TraktStatus | null>(null);
  const [signIn, setSignIn] = useState<TraktSignInStart | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [showApp, setShowApp] = useState(false);
  const [clientId, setClientId] = useState("");
  const [redirectUri, setRedirectUri] = useState("http://localhost");

  useEffect(() => {
    api.traktStatus().then(setStatus).catch(() => {});
    return api.onTraktChanged(setStatus);
  }, []);

  // Asks every few seconds whether the code has been entered, until it expires.
  useEffect(() => {
    if (!signIn) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const expires = Date.now() + signIn.expiresIn * 1000;
    const check = async () => {
      try {
        const done = await api.traktCheckSignIn();
        if (stopped) return;
        if (done) {
          setSignIn(null);
          api.traktStatus().then(setStatus).catch(() => {});
          return;
        }
      } catch (e) {
        if (!stopped) {
          setError(errorText(e));
          setSignIn(null);
        }
        return;
      }
      if (Date.now() > expires) {
        setError("The code expired before it was entered. Connect again for a new one.");
        setSignIn(null);
        return;
      }
      timer = setTimeout(check, Math.max(1, signIn.interval) * 1000);
    };
    timer = setTimeout(check, Math.max(1, signIn.interval) * 1000);
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [signIn]);

  const run = async (work: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await work();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  const connect = () =>
    run(async () => {
      const started = await api.traktStartSignIn();
      setSignIn(started);
      await api.openExternal(started.verificationUrl);
    });
  const disconnect = async () => {
    const ok = await confirmAction({
      title: "Disconnect Trakt?",
      message: "Xorvio stops telling Trakt what you watch. What's already here and on Trakt stays.",
      confirmLabel: "Disconnect",
    });
    if (ok) run(() => api.traktDisconnect());
  };
  const setOption = (key: keyof TraktOptions) => (on: boolean) => {
    if (!status) return;
    const options = { ...status.options, [key]: on };
    setStatus({ ...status, options });
    api.traktSetOptions(options).catch((e) => setError(errorText(e)));
  };
  const saveApp = (id: string, redirect: string) =>
    run(async () => {
      await api.traktSetApp(id.trim(), redirect.trim());
      setClientId("");
      setShowApp(false);
      setStatus(await api.traktStatus());
    });

  if (!status) return null;
  const appForm = (
    <div className="trakt-app">
      <p className="muted small">
        Create an app at trakt.tv/oauth/applications with <code>http://localhost</code> as its redirect URI, then enter its Client ID here. Xorvio doesn't need the Client Secret. If you registered a different redirect URI, enter that one too.
      </p>
      <input className="text-input" placeholder="Client ID" value={clientId} onChange={(e) => setClientId(e.target.value)} spellCheck={false} autoComplete="off" />
      <input className="text-input" placeholder="Redirect URI" value={redirectUri} onChange={(e) => setRedirectUri(e.target.value)} spellCheck={false} autoComplete="off" aria-label="Redirect URI" />
      <div className="trakt-actions">
        <button className="btn-accent" disabled={busy || !clientId.trim()} onClick={() => saveApp(clientId, redirectUri)}>
          Use This App
        </button>
        {status.customApp && (
          <button className="btn-plain" disabled={busy} onClick={() => saveApp("", "")}>
            Use the Built-In App
          </button>
        )}
      </div>
    </div>
  );

  return (
    <SettingsGroup
      title="Trakt"
      note="Trakt keeps a record of what you watch across your apps. Xorvio sends a film or episode's IMDb or TMDB id (or its title and year when it has neither), never the source it came from."
    >
      {!status.configured ? (
        <>
          <p className="trakt-intro">This copy of Xorvio has no Trakt app built in, so it connects through one of your own.</p>
          {appForm}
        </>
      ) : status.connected ? (
        <>
          <Row label="Account">
            <div className="trakt-line">
              <span>
                Connected as <strong>{status.username ?? "you"}</strong>
              </span>
              <button className="btn-plain" disabled={busy} onClick={disconnect}>
                Disconnect
              </button>
            </div>
          </Row>
          <Row label="Last sync">
            <div className="trakt-line">
              <span className="muted">{status.syncing ? "Syncing…" : status.lastSync ? ago(status.lastSync) : "Not yet"}</span>
              <button className="btn-plain" disabled={busy || status.syncing} onClick={() => run(async () => setStatus(await api.traktSyncNow()))}>
                <RefreshCw size={13} /> Sync Now
              </button>
            </div>
          </Row>
          <Row label="While watching">
            <Toggle on={status.options.scrobble} onChange={setOption("scrobble")} label="Tell Trakt what's playing" />
          </Row>
          <Row label="History">
            <Toggle on={status.options.syncHistory} onChange={setOption("syncHistory")} label="Sync watched films and episodes, and where you left off" />
          </Row>
          <Row label="Watchlist">
            <Toggle on={status.options.syncWatchlist} onChange={setOption("syncWatchlist")} label="Sync films and series on your watchlist" />
          </Row>
          {status.lastError && (
            <p className="form-error trakt-error">
              <TriangleAlert size={13} /> {status.lastError}
            </p>
          )}
        </>
      ) : signIn ? (
        <div className="trakt-code">
          <div className="spinner small" />
          <div>
            <strong>Enter this code at {signIn.verificationUrl.replace(/^https?:\/\//, "")}</strong>
            <div className="trakt-code-row">
              <span className="trakt-user-code">{signIn.userCode}</span>
              <button className="icon-button" aria-label="Copy the code" onClick={() => navigator.clipboard?.writeText(signIn.userCode).catch(() => {})}>
                <Copy size={14} />
              </button>
            </div>
            <div className="trakt-actions">
              <button className="btn-plain" onClick={() => api.openExternal(signIn.verificationUrl)}>
                Open Trakt Again
              </button>
              <button className="btn-plain" onClick={() => setSignIn(null)}>
                Cancel
              </button>
            </div>
          </div>
        </div>
      ) : (
        <>
          <Row label="Account">
            <button className="btn-accent" disabled={busy} onClick={connect}>
              {busy ? "Opening Trakt…" : "Connect Trakt"}
            </button>
          </Row>
          <Row label="On Home">
            <Toggle on={rowsWithoutAccount} onChange={(on) => updateSettings({ traktRowsWithoutAccount: on })} label="Show Trakt's charts and lists before connecting" />
          </Row>
        </>
      )}
      {error && (
        <p className="form-error trakt-error">
          <TriangleAlert size={13} /> {error}
        </p>
      )}
      {status.configured && !signIn && (
        <div className="trakt-own-app">
          <button className="btn-link" onClick={() => setShowApp((v) => !v)}>
            {showApp ? "Hide" : status.customApp ? "Change the Trakt app Xorvio connects through" : "Connect through your own Trakt app"}
          </button>
          {showApp && appForm}
        </div>
      )}
    </SettingsGroup>
  );
}

const DIAGNOSTIC_AREAS = ["Sources", "Guide", "Playback", "Recording", "Network", "Services", "Add-ons", "Storage", "App"];

const diagnosticIcons: Record<DiagnosticLevel, ReactNode> = {
  info: <Info size={14} aria-label="Info" />,
  warning: <TriangleAlert size={14} aria-label="Warning" />,
  error: <OctagonX size={14} aria-label="Error" />,
};

/** The diagnostic log: what went wrong lately, redacted, for a bug report. */
function DiagnosticsSettings() {
  const detailed = useApp((s) => s.settings.detailedDiagnostics);
  const syncing = useApp((s) => s.settings.syncEnabled);
  const update = useApp((s) => s.updateSettings);
  const showToast = useApp((s) => s.showToast);
  const [events, setEvents] = useState<DiagnosticEvent[]>([]);
  const [area, setArea] = useState("");
  const [level, setLevel] = useState("");
  const [open, setOpen] = useState<number | null>(null);
  const [showingHistory, setShowingHistory] = useState(false);
  const load = () => api.diagnostics().then(setEvents).catch(() => {});
  useEffect(() => {
    load();
  }, []);
  const shown = events.filter((e) => (!area || e.area === area) && (!level || e.level === level));

  const exportLog = async () => {
    try {
      const path = await api.exportDiagnostics();
      if (path) showToast(`Saved the log as ${path.split(/[\\/]/).pop()}`);
    } catch (error) {
      showToast(errorText(error));
    }
  };

  return (
    <SettingsGroup
      title="Privacy & Diagnostics"
      note={`Xorvio collects no analytics and sends no viewing data anywhere; your history and progress stay on ${syncing ? "your computers and in your sync folder" : "this computer"}. Warnings and errors are kept here, until Xorvio quits, so you can see why something failed. Logins, tokens and link parameters are removed, and the log leaves this computer only if you export it.`}
    >
      <Row label="Viewing history">
        <button className="btn-plain" onClick={() => setShowingHistory(true)}>
          Show…
        </button>
      </Row>
      <Row label="Detailed events">
        <Toggle on={detailed} onChange={(on) => update({ detailedDiagnostics: on })} label="Keep everything, not only warnings and errors" />
      </Row>
      <div className="diagnostics-bar">
        <select className="select-input" value={area} onChange={(e) => setArea(e.target.value)} aria-label="Area">
          <option value="">All areas</option>
          {DIAGNOSTIC_AREAS.map((name) => (
            <option key={name}>{name}</option>
          ))}
        </select>
        <select className="select-input" value={level} onChange={(e) => setLevel(e.target.value)} aria-label="Level">
          <option value="">All levels</option>
          <option value="error">Errors</option>
          <option value="warning">Warnings</option>
          <option value="info">Info</option>
        </select>
        <div className="settings-buttons">
          <button className="btn-plain" onClick={load}>
            <RefreshCw size={13} /> Refresh
          </button>
          <button className="btn-plain" onClick={exportLog} disabled={events.length === 0}>
            Export…
          </button>
          <button
            className="btn-plain"
            disabled={events.length === 0}
            onClick={() => api.clearDiagnostics().then(() => setEvents([]))}
          >
            Clear
          </button>
        </div>
      </div>
      <ul className="diagnostics-list">
        {shown.length === 0 ? (
          <li className="diagnostics-empty muted small">
            <CircleAlert size={14} /> {events.length === 0 ? "Nothing has gone wrong since Xorvio opened." : "No events match."}
          </li>
        ) : (
          shown.map((e) => (
            <li key={e.id} className={`diagnostic diagnostic-${e.level}${open === e.id ? " open" : ""}`} onClick={() => setOpen(open === e.id ? null : e.id)}>
              {diagnosticIcons[e.level]}
              <span className="diagnostic-message">{e.message}</span>
              <span className="diagnostic-meta muted small">
                {e.area} · {new Date(e.at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit", second: "2-digit" })}
              </span>
            </li>
          ))
        )}
      </ul>
      {/* Over the whole window, not inside the card. */}
      {showingHistory && createPortal(<HistorySheet onClose={() => setShowingHistory(false)} />, document.body)}
    </SettingsGroup>
  );
}

const historyIcons: Partial<Record<HistoryEntry["kind"], ReactNode>> = {
  channel: <Tv size={15} />,
  movie: <Film size={15} />,
  episode: <Clapperboard size={15} />,
};

/** What's been watched on this computer, newest first. */
function HistorySheet({ onClose }: { onClose: () => void }) {
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);
  // Synced history is cleared on the other computers too.
  const syncing = useApp((s) => s.settings.syncEnabled);
  useEffect(() => {
    api.history().then(setEntries).catch(() => setEntries([]));
  }, []);
  const clear = async () => {
    const ok = await confirmAction({
      title: "Clear all viewing history?",
      message: `This removes the history list and Recently Watched channels on this computer${syncing ? " and the computers it syncs with" : ""}. Continue Watching and what's marked watched aren't affected.`,
      confirmLabel: "Clear History",
      destructive: true,
    });
    if (!ok) return;
    await api.clearHistory().catch(() => {});
    setEntries([]);
  };
  return (
    <Sheet
      title="Viewing History"
      subtitle={syncing ? "What you've watched, newest first, here and on the computers this one syncs with." : "What you've watched, newest first. It stays on this computer."}
      onClose={onClose}
      width={560}
      footer={
        <>
          <button className="btn-plain" disabled={!entries?.length} onClick={clear}>
            Clear History…
          </button>
          <div style={{ flex: 1 }} />
          <button className="btn-accent" onClick={onClose}>
            Done
          </button>
        </>
      }
    >
      {entries?.length === 0 ? (
        <p className="muted small settings-empty">Nothing yet. What you watch appears here.</p>
      ) : (
        <ul className="history-list">
          {(entries ?? []).map((entry, i) => (
            <li key={`${entry.itemId}|${entry.watchedAt}|${i}`} className="history-row">
              {historyIcons[entry.kind] ?? <Film size={15} />}
              <div className="history-text">
                <div className="ellipsis">{entry.title}</div>
                {entry.subtitle && <div className="muted small ellipsis">{entry.subtitle}</div>}
              </div>
              <span className="muted small">{ago(entry.watchedAt)}</span>
            </li>
          ))}
        </ul>
      )}
    </Sheet>
  );
}

/** A folder, chosen with the system's folder picker, or the default when none is. */
function FolderChoice({ folder, placeholder, resetLabel, onChange }: { folder: string | null; placeholder: string; resetLabel: string; onChange: (folder: string | null) => void }) {
  return (
    <div style={{ display: "flex", gap: 8, alignItems: "center", minWidth: 0 }}>
      <span className="muted ellipsis" title={folder ?? undefined}>
        {folder ?? placeholder}
      </span>
      <button
        className="btn-plain"
        onClick={async () => {
          const chosen = await api.chooseFolder();
          if (chosen) onChange(chosen);
        }}
      >
        Choose…
      </button>
      {folder && (
        <button className="btn-plain" onClick={() => onChange(null)} title={resetLabel} aria-label={resetLabel}>
          <X size={13} />
        </button>
      )}
    </div>
  );
}

/** A folder of its own for each source with live channels; new recordings from it go there. */
function SourceFolders({ folders, onChange }: { folders: Record<string, string>; onChange: (folders: Record<string, string>) => void }) {
  const [sources, setSources] = useState<SourceSummary[]>([]);
  useEffect(() => {
    api
      .listSources()
      .then((list) => setSources(list.filter((s) => s.status.capabilities.live || s.status.channelCount > 0)))
      .catch(() => {});
  }, []);
  if (!sources.length) return null;
  const choose = (sourceId: string, folder: string | null) => {
    const next = { ...folders };
    if (folder) next[sourceId] = folder;
    else delete next[sourceId];
    onChange(next);
  };
  return (
    <>
      {sources.map((source) => (
        <Row key={source.config.id} label={sources.length === 1 ? `From ${source.config.name}` : source.config.name}>
          <FolderChoice folder={folders[source.config.id] ?? null} placeholder="The folder above" resetLabel="Use the folder above" onChange={(folder) => choose(source.config.id, folder)} />
        </Row>
      ))}
    </>
  );
}

export function SettingsGroup({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
  return (
    <section className="settings-group">
      <h2 className="section-title">{title}</h2>
      <div className="settings-card">{children}</div>
      {note && <p className="settings-note">{note}</p>}
    </section>
  );
}

export function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="settings-row">
      <span className="settings-label">{label}</span>
      <div className="settings-control">{children}</div>
    </div>
  );
}

export function Segmented({ value, options, onChange }: { value: string; options: [string, string][]; onChange: (v: string) => void }) {
  return (
    <div className="segmented">
      {options.map(([v, label]) => (
        <button key={v} className={v === value ? "selected" : ""} onClick={() => onChange(v)}>
          {label}
        </button>
      ))}
    </div>
  );
}

export function Toggle({ on, onChange, label }: { on: boolean; onChange: (on: boolean) => void; label?: string }) {
  return (
    <label className="toggle-row">
      <button role="switch" aria-checked={on} className={`toggle ${on ? "on" : ""}`} onClick={() => onChange(!on)} />
      {label && <span>{label}</span>}
    </label>
  );
}

function Stepper({ value, unit, min, max, onChange }: { value: number; unit: string; min: number; max: number; onChange: (v: number) => void }) {
  return (
    <div className="stepper">
      <button className="btn-plain" onClick={() => onChange(Math.max(min, value - 1))} aria-label="Less">
        −
      </button>
      <span>
        {value} {unit}
      </span>
      <button className="btn-plain" onClick={() => onChange(Math.min(max, value + 1))} aria-label="More">
        +
      </button>
    </div>
  );
}
