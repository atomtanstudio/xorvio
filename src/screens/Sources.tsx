import {
  ArrowDown,
  ArrowUp,
  CalendarClock,
  CircleCheck,
  FileText,
  Folder,
  FolderPlus,
  GripVertical,
  HardDrive,
  Lightbulb,
  Link2,
  Pause,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Server,
  Sparkles,
  Trash2,
  TriangleAlert,
  X,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { confirmAction } from "../components/Confirm";
import { openMenu } from "../components/Menu";
import { Sheet } from "../components/Sheet";
import { api, errorText } from "../lib/api";
import { duration, shortDate } from "../lib/format";
import { isMac, keychainName, networkShare, trayName } from "../lib/platform";
import { useSources } from "../lib/sources";
import { useApp } from "../lib/store";
import { openMediaFolder } from "./MediaLibrary";
import type {
  GuideSource,
  GuideSourceSummary,
  LiveStreamFormat,
  NewSource,
  PlexServerChoice,
  PlexSignInStart,
  RefreshInterval,
  SourceConfig,
  SourceKind,
  SourceSecrets,
  SourceSummary,
} from "../lib/types";
import "../styles/sources.css";

const kindIcon: Record<SourceKind, React.ReactNode> = {
  xtream: <Server size={18} />,
  m3uUrl: <Link2 size={18} />,
  m3uFile: <FileText size={18} />,
  demo: <Sparkles size={18} />,
  folder: <Folder size={18} />,
  plex: <HardDrive size={18} />,
  jellyfin: <HardDrive size={18} />,
  emby: <HardDrive size={18} />,
};

const kindName: Record<SourceKind, string> = {
  xtream: "Xtream Codes",
  m3uUrl: "M3U Playlist",
  m3uFile: "M3U File",
  demo: "Demonstration",
  folder: "Media Folder",
  plex: "Plex",
  jellyfin: "Jellyfin",
  emby: "Emby",
};

const providerKinds: SourceKind[] = ["xtream", "m3uUrl", "m3uFile"];
const serverKinds: SourceKind[] = ["plex", "jellyfin", "emby"];
const folderKinds: SourceKind[] = ["folder"];
const shortKindName: Partial<Record<SourceKind, string>> = { m3uUrl: "M3U Link", m3uFile: "M3U File" };

/** The kinds a new source can be, grouped as Add a Source shows them, each with a line on what it needs. */
const kindGroups: { title: string; kinds: SourceKind[] }[] = [
  { title: "IPTV provider", kinds: providerKinds },
  { title: "Media server", kinds: serverKinds },
  { title: isMac ? "On this Mac" : "On this computer", kinds: folderKinds },
];
const kindNote: Partial<Record<SourceKind, string>> = {
  xtream: "Server and login",
  m3uUrl: "A playlist's address",
  m3uFile: "A playlist file",
  plex: "Sign in with Plex",
  jellyfin: "Server and account",
  emby: "Server and account",
  folder: "Your video files",
};

/** Plex, Jellyfin or Emby: a person's own films and series, rather than TV. */
const isMediaServer = (kind: SourceKind) => serverKinds.includes(kind);
/** A media server or folder: films and series only. */
const isLibrary = (kind: SourceKind) => isMediaServer(kind) || kind === "folder";
/** "Films" for "/Volumes/NAS/Films". */
const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

const scheduleName: Record<RefreshInterval, string> = { daily: "Refreshes daily", every6Hours: "Refreshes every 6 hours", manual: "Refreshes when you ask" };

export function SourcesScreen() {
  const sources = useSources((s) => s.sources);
  const progress = useSources((s) => s.progress);
  const stopping = useSources((s) => s.stopping);
  const pendingAdd = useSources((s) => s.pendingAdd);
  const { refresh, refreshAll, cancel, move } = useSources.getState();
  const [editing, setEditing] = useState<SourceSummary | "new" | null>(null);
  const [startKind, setStartKind] = useState<SourceKind | null>(null);
  const [canImport, setCanImport] = useState(false);
  const [importing, setImporting] = useState(false);
  const [dragging, setDragging] = useState<string | null>(null);
  const [drop, setDrop] = useState<{ id: string; after: boolean } | null>(null);
  const showToast = useApp((s) => s.showToast);

  // File › Add Source… and the toolbar's + open the sheet here.
  useEffect(() => {
    if (!pendingAdd) return;
    useSources.setState({ pendingAdd: null });
    setStartKind(pendingAdd.kind);
    setEditing("new");
  }, [pendingAdd]);

  const [canImportLibrary, setCanImportLibrary] = useState(false);
  const [importingLibrary, setImportingLibrary] = useState(false);

  useEffect(() => {
    api.swiftImportAvailable().then(setCanImport).catch(() => {});
    api.swiftLibraryAvailable().then(setCanImportLibrary).catch(() => {});
  }, []);

  const importLibrary = async () => {
    setImportingLibrary(true);
    try {
      const result = await api.importSwiftLibrary();
      const parts = [
        result.addons.length ? `Installed ${result.addons.join(", ")}` : null,
        `added ${result.watchlistAdded} title${result.watchlistAdded === 1 ? "" : "s"} to your watchlist`,
        result.progressImported ? `brought over your watch history (${result.progressImported} films and episodes)` : null,
        result.watchlistSkipped.length ? `${result.watchlistSkipped.length} couldn't be matched (${result.watchlistSkipped.join(", ")})` : null,
      ].filter(Boolean);
      showToast(parts.join("; ") + ".");
      if (result.addonProblems.length) setTimeout(() => showToast(result.addonProblems.join(" "), "error"), 4000);
      setCanImportLibrary(false);
      useApp.getState().loadLibrary();
    } catch (e) {
      showToast(errorText(e), "error");
    } finally {
      setImportingLibrary(false);
    }
  };

  const importSwift = async () => {
    setImporting(true);
    try {
      const added = await api.importSwiftSources();
      showToast(`Imported ${added.map((s) => s.name).join(", ")}. Loading the catalog now.`);
      setCanImport(false);
      await load();
      added.forEach((s) => refresh(s.id));
    } catch (e) {
      showToast(errorText(e), "error");
    } finally {
      setImporting(false);
    }
  };

  const load = useSources.getState().load;
  useEffect(() => {
    load();
  }, [load]);

  /** Moves the dragged source to where it was dropped, before or after another. */
  const dropOn = (target: string, after: boolean) => {
    const list = sources ?? [];
    if (!dragging || dragging === target) return;
    const from = list.findIndex((s) => s.config.id === dragging);
    let to = list.findIndex((s) => s.config.id === target) + (after ? 1 : 0);
    if (from < to) to -= 1;
    move(dragging, to);
  };

  const remove = async (s: SourceSummary) => {
    const ok = await confirmAction({
      title: `Remove “${s.config.name}”?`,
      message:
        s.config.kind === "folder"
          ? "Its films and series leave Xorvio. The files themselves stay where they are, and so do your watchlist and history."
          : "Its channels, films and series leave Xorvio. Your watchlist and history stay.",
      confirmLabel: "Remove",
      destructive: true,
    });
    if (!ok) return;
    await api.removeSource(s.config.id).catch((e) => showToast(errorText(e), "error"));
    load();
  };

  return (
    <div className="content">
      <div className="page sources-page">
        <div className="page-header">
          <div>
            <h1 className="page-title">Sources</h1>
            <p className="page-subtitle">
              Playlists, provider logins, media servers and folders of videos you already have. Xorvio ships no TV content of its own.
              {(sources?.length ?? 0) > 1 ? " Drag sources into the order Live TV lists their channels in." : ""}
            </p>
          </div>
          <div className="sources-header-buttons">
            {!!sources?.length && (
              <button className="btn-plain" disabled={sources.every((s) => s.config.isPaused || progress.has(s.config.id))} onClick={refreshAll}>
                <RefreshCw size={13} /> Refresh All
              </button>
            )}
            <button
              className="btn-accent"
              onClick={() => {
                setStartKind(null);
                setEditing("new");
              }}
            >
              <Plus size={14} /> Add Source
            </button>
          </div>
        </div>
        {canImport && (
          <div className="import-banner">
            <div>
              <strong>Bring your sources over from Xorvio for Mac</strong>
              <p className="muted small">Your provider logins are copied from the Mac app's keychain. macOS will ask you to allow it.</p>
            </div>
            <button className="btn-accent" disabled={importing} onClick={importSwift}>
              {importing ? "Importing…" : "Import"}
            </button>
          </div>
        )}
        {canImportLibrary && !canImport && (
          <div className="import-banner">
            <div>
              <strong>Bring your add-ons and watchlist over from Xorvio for Mac</strong>
              <p className="muted small">Installs the add-ons from Xorvio Personal (and Cinemeta, which it had built in) and copies both Mac apps' watchlists and watch history. macOS will ask you to allow reading the add-on links.</p>
            </div>
            <button className="btn-accent" disabled={importingLibrary} onClick={importLibrary}>
              {importingLibrary ? "Importing…" : "Import"}
            </button>
          </div>
        )}
        {sources?.length === 0 && (
          <div className="empty-state">
            <div>
              <h3>No sources yet</h3>
              <p>Add an Xtream Codes login or an M3U playlist from your provider, your own Plex, Jellyfin or Emby server, or a folder of your own videos. Or try the demo library to look around.</p>
              <div style={{ display: "flex", gap: 10, justifyContent: "center" }}>
                <button className="btn-accent" onClick={() => setEditing("new")}>
                  Add Source
                </button>
                <button
                  className="btn-plain"
                  onClick={async () => {
                    await api.addDemoSource();
                    await load();
                    refresh("demo");
                  }}
                >
                  Try the Demo
                </button>
              </div>
            </div>
          </div>
        )}
        <div className="source-list" onDragEnd={() => (setDragging(null), setDrop(null))}>
          {sources?.map((s, index) => {
            const p = progress.get(s.config.id);
            const running = s.refreshing || !!p;
            const st = s.status;
            const reorderable = sources.length > 1;
            const placement = drop?.id === s.config.id ? (drop.after ? "drop-after" : "drop-before") : "";
            return (
              <div
                key={s.config.id}
                className={`source-card ${dragging === s.config.id ? "dragging" : ""} ${placement}`}
                draggable={reorderable}
                onDragStart={(e) => {
                  e.dataTransfer.effectAllowed = "move";
                  e.dataTransfer.setData("text/plain", s.config.id);
                  setDragging(s.config.id);
                }}
                onDragOver={(e) => {
                  if (!dragging) return;
                  e.preventDefault();
                  const rect = e.currentTarget.getBoundingClientRect();
                  setDrop({ id: s.config.id, after: e.clientY > rect.top + rect.height / 2 });
                }}
                onDrop={(e) => {
                  e.preventDefault();
                  if (drop) dropOn(drop.id, drop.after);
                  setDragging(null);
                  setDrop(null);
                }}
              >
                {reorderable && (
                  <button
                    className="source-grip"
                    aria-label={`Move ${s.config.name} up or down with the arrow keys`}
                    title="Drag to change the order, or use the arrow keys"
                    onKeyDown={(e) => {
                      if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
                      e.preventDefault();
                      move(s.config.id, index + (e.key === "ArrowUp" ? -1 : 1));
                    }}
                  >
                    <GripVertical size={15} />
                  </button>
                )}
                <div className="source-icon">{kindIcon[s.config.kind]}</div>
                <div className="source-text">
                  <div className="source-name">
                    {s.config.name}
                    {s.config.isPaused && <span className="badge">PAUSED</span>}
                  </div>
                  <div className="source-meta">
                    {kindName[s.config.kind]}
                    {s.config.displayHost ? ` · ${s.config.displayHost}` : ""}
                    {st.lastSuccess ? ` · Updated ${shortDate(st.lastSuccess)}` : ""}
                    {s.config.kind !== "demo" ? ` · ${scheduleName[s.config.refreshInterval]}` : ""}
                  </div>
                  <div className="source-counts">
                    {isLibrary(s.config.kind)
                      ? `${st.movieCount.toLocaleString()} films · ${st.seriesCount.toLocaleString()} series`
                      : `${st.channelCount.toLocaleString()} channels · ${st.movieCount.toLocaleString()} films · ${st.seriesCount.toLocaleString()} series · ${st.programCount.toLocaleString()} listings`}
                    {st.maxConnections ? ` · ${st.maxConnections} connection${st.maxConnections === 1 ? "" : "s"}` : ""}
                    {st.accountExpires ? ` · Expires ${shortDate(st.accountExpires)}` : ""}
                  </div>
                  {running && (
                    <div className="source-progress">
                      <div className="spinner small" /> {stopping.has(s.config.id) ? "Stopping" : (p?.phase ?? "Refreshing")}…
                    </div>
                  )}
                  {st.lastError && !running && (
                    <div className="source-error">
                      <TriangleAlert size={12} /> {st.lastError}
                      {st.lastSuccess ? <span className="source-error-note">Still showing the catalogue from {shortDate(st.lastSuccess)}.</span> : null}
                    </div>
                  )}
                  {st.catalogWarning && <div className="source-warning">{st.catalogWarning}</div>}
                  {st.guideLastError && <div className="source-warning">Guide: {st.guideLastError}</div>}
                </div>
                <div className="source-actions">
                  {running ? (
                    <button className="btn-plain" disabled={stopping.has(s.config.id)} onClick={() => cancel(s.config.id)}>
                      <X size={13} /> Cancel
                    </button>
                  ) : (
                    <button className="btn-plain" disabled={s.config.isPaused} onClick={() => refresh(s.config.id)}>
                      <RefreshCw size={13} /> Refresh
                    </button>
                  )}
                  <button
                    className="icon-button"
                    aria-label="More"
                    onClick={(e) =>
                      openMenu(
                        e,
                        [
                          ...(s.config.kind === "folder" ? [{ label: "Open Library", icon: <Folder size={15} />, onSelect: () => openMediaFolder(s.config.id, null) }] : []),
                          ...(s.config.kind !== "demo" ? [{ label: "Edit…", icon: <Pencil size={15} />, onSelect: () => setEditing(s) }] : []),
                          {
                            label: s.config.isPaused ? "Resume" : "Pause",
                            icon: s.config.isPaused ? <Play size={15} /> : <Pause size={15} />,
                            onSelect: () => api.updateSource({ ...s.config, isPaused: !s.config.isPaused }, null).then(load),
                          },
                          ...(reorderable
                            ? [
                                { separator: true as const },
                                { label: "Move Up", icon: <ArrowUp size={15} />, disabled: index === 0, onSelect: () => move(s.config.id, index - 1) },
                                { label: "Move Down", icon: <ArrowDown size={15} />, disabled: index === sources.length - 1, onSelect: () => move(s.config.id, index + 1) },
                              ]
                            : []),
                          { separator: true },
                          { label: "Remove…", icon: <Trash2 size={15} />, destructive: true, onSelect: () => remove(s) },
                        ],
                        true,
                      )
                    }
                  >
                    •••
                  </button>
                </div>
              </div>
            );
          })}
        </div>
        {!!sources?.length && <GuidesSection />}
      </div>
      {editing && (
        <SourceSheet
          existing={editing === "new" ? null : editing.config}
          initialKind={startKind}
          onClose={() => setEditing(null)}
          onSaved={async (id) => {
            setEditing(null);
            await load();
            if (id) refresh(id);
          }}
        />
      )}
    </div>
  );
}

/**
 * Adding a source, or editing one. A playlist file can be dropped onto it;
 * an Xtream login can be tried before it's saved.
 */
export function SourceSheet({
  existing,
  initialKind = null,
  onClose,
  onSaved,
}: {
  existing: SourceConfig | null;
  /** The kind to start on when adding. */
  initialKind?: SourceKind | null;
  onClose: () => void;
  onSaved: (refreshId: string | null) => void;
}) {
  const [kind, setKind] = useState<SourceKind>(existing?.kind ?? initialKind ?? "xtream");
  const [name, setName] = useState(existing?.name ?? "");
  const [secrets, setSecrets] = useState<SourceSecrets>({});
  const [filePath, setFilePath] = useState<string | null>(existing?.filePath ?? null);
  const [liveFormat, setLiveFormat] = useState<LiveStreamFormat>(existing?.liveFormat ?? "hls");
  const [userAgent, setUserAgent] = useState(existing?.userAgent ?? "");
  const [guideOffset, setGuideOffset] = useState(existing?.guideOffsetMinutes ?? 0);
  const [folders, setFolders] = useState<string[]>(existing?.folders ?? []);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const plex = usePlexSignIn(setError);
  // Plex is signed in to through plex.tv unless an address is asked for.
  const [plexByAddress, setPlexByAddress] = useState(false);
  const [refreshInterval, setRefreshInterval] = useState<RefreshInterval>(existing?.refreshInterval ?? "daily");
  const [test, setTest] = useState<{ ok: boolean; text: string } | "testing" | null>(null);
  // An Xtream login found in the playlist link, to offer the Xtream connection instead.
  const [xtreamInLink, setXtreamInLink] = useState<SourceSecrets | null>(null);
  // A playlist dropped onto the sheet, kept by the app when the source is saved.
  const [dropped, setDropped] = useState<{ name: string; contents: string } | null>(null);
  const [dropping, setDropping] = useState(false);

  useEffect(() => {
    // A media folder has nothing in the keychain.
    if (existing && existing.kind !== "folder") api.sourceSecrets(existing.id).then(setSecrets).catch(() => {});
  }, [existing]);

  // Starting on Media Folder asks for one straight away, as choosing it does:
  // once, as the sheet opens (development runs mount effects twice).
  const askedForFolder = useRef(false);
  useEffect(() => {
    if (existing || initialKind !== "folder" || askedForFolder.current) return;
    askedForFolder.current = true;
    addFolders();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const playlistLink = kind === "m3uUrl" && !existing ? (secrets.playlistUrl ?? "") : "";
  useEffect(() => {
    if (!playlistLink) return setXtreamInLink(null);
    let current = true;
    api
      .xtreamLoginInLink(playlistLink)
      .then((found) => current && setXtreamInLink(found))
      .catch(() => {});
    return () => {
      current = false;
    };
  }, [playlistLink]);

  // A file dropped anywhere else while the sheet is open would otherwise open in the window.
  useEffect(() => {
    const hasFiles = (e: DragEvent) => !!e.dataTransfer?.types.includes("Files");
    const onOver = (e: DragEvent) => {
      if (!hasFiles(e) || e.defaultPrevented) return;
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "none";
    };
    const onDrop = (e: DragEvent) => hasFiles(e) && e.preventDefault();
    window.addEventListener("dragover", onOver);
    window.addEventListener("drop", onDrop);
    return () => {
      window.removeEventListener("dragover", onOver);
      window.removeEventListener("drop", onDrop);
    };
  }, []);

  /** Takes a dropped playlist file (the web view gives its contents, not where it is). */
  const takeDrop = async (file: File | undefined) => {
    setDropping(false);
    if (!file) return;
    setError(null);
    try {
      const contents = await file.text();
      if (!/#EXTM3U|#EXTINF/i.test(contents)) throw new Error(`“${file.name}” isn't an M3U playlist.`);
      setDropped({ name: file.name, contents });
      setFilePath(null);
      setKind("m3uFile");
      if (!name.trim() && !existing) setName(file.name.replace(/\.(m3u8?|txt)$/i, ""));
    } catch (e) {
      setError(errorText(e));
    }
  };
  // Only a playlist file source takes a file.
  const takesDrops = !existing || existing.kind === "m3uFile";

  const testLogin = async () => {
    setTest("testing");
    try {
      const account = await api.testXtreamLogin(secrets, userAgent.trim() || null);
      const facts = [
        account.maxConnections ? `${account.maxConnections} connection${account.maxConnections === 1 ? "" : "s"} allowed` : null,
        account.expires ? `Expires ${shortDate(account.expires)}` : "No expiry date",
      ].filter(Boolean);
      setTest({ ok: true, text: `Connected. ${facts.join(". ")}.` });
    } catch (e) {
      setTest({ ok: false, text: errorText(e) });
    }
  };

  const server = isMediaServer(kind);
  const folderKind = kind === "folder";
  const addFolders = async () => {
    try {
      const picked = await api.chooseMediaFolders();
      setFolders((list) => [...list, ...picked.filter((f) => !list.includes(f))]);
    } catch (e) {
      setError(errorText(e));
    }
  };
  const plexSignIn = kind === "plex" && !existing && !plexByAddress;
  const set = (patch: Partial<SourceSecrets>) => {
    setSecrets((s) => ({ ...s, ...patch }));
    // A result for other details would mislead.
    setTest(null);
  };
  const valid = folderKind
    ? folders.length > 0
    : existing && server
      ? !!secrets.serverUrl
      : kind === "xtream"
        ? !!(secrets.serverUrl && secrets.username && secrets.password)
        : kind === "m3uUrl"
          ? !!secrets.playlistUrl
          : kind === "m3uFile"
            ? !!(filePath || dropped)
            : kind === "jellyfin" || kind === "emby"
              ? !!(secrets.serverUrl && secrets.username)
              : kind === "plex"
                ? plexSignIn
                  ? !!plex.chosen
                  : !!secrets.serverUrl
                : false;

  /** A new source keeps the refresh schedule chosen here (adding one always starts daily). */
  const scheduled = async (config: SourceConfig) => {
    if (config.refreshInterval !== refreshInterval) await api.updateSource({ ...config, refreshInterval }, null);
    return config.id;
  };

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      // A dropped playlist becomes the app's own copy now, so a cancelled sheet leaves nothing behind.
      const playlistFile = kind === "m3uFile" && dropped ? await api.keepDroppedPlaylist(dropped.name, dropped.contents) : filePath;
      if (folderKind) {
        if (existing) await api.updateSource({ ...existing, name: name.trim() || existing.name, folders, refreshInterval }, null);
        onSaved(existing?.id ?? (await scheduled(await api.addFolderSource(name.trim(), folders))));
      } else if (existing) {
        await api.updateSource(
          { ...existing, name: name || existing.name, liveFormat, userAgent: userAgent || null, filePath: playlistFile, guideOffsetMinutes: guideOffset, refreshInterval },
          secrets,
        );
        if ((existing.kind === "jellyfin" || existing.kind === "emby") && secrets.password) await api.signInToSourceAgain(existing.id, secrets.password);
        if (existing.kind === "plex" && plex.pin && plex.servers) await api.replacePlexSignIn(existing.id, plex.pin.pinId);
        onSaved(existing.id);
      } else if (plexSignIn && plex.pin && plex.chosen) {
        onSaved(await scheduled(await api.addPlexServer(plex.pin.pinId, plex.chosen, name.trim())));
      } else {
        const source: NewSource = { name: name.trim() || defaultName(kind, secrets), kind, secrets, filePath: playlistFile, liveFormat, userAgent: userAgent.trim() || null };
        onSaved(await scheduled(await api.addSource(source)));
      }
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const choose = (k: SourceKind) => {
    setKind(k);
    setError(null);
    setTest(null);
    // Choosing Media Folder asks for one straight away.
    if (k === "folder" && kind !== "folder" && !folders.length) addFolders();
  };
  const kindTile = (k: SourceKind) => (
    <button key={k} className={`kind-tile ${kind === k ? "selected" : ""}`} role="radio" aria-checked={kind === k} onClick={() => choose(k)}>
      <span className="kind-tile-icon">{kindIcon[k]}</span>
      <span className="kind-tile-text">
        <span className="kind-tile-name">{shortKindName[k] ?? kindName[k]}</span>
        <span className="kind-tile-note">{kindNote[k]}</span>
      </span>
    </button>
  );
  const chosenServer = plex.servers?.find((s) => s.id === plex.chosen);

  return (
    <Sheet
      title={existing ? `Edit ${existing.name}` : "Add a Source"}
      subtitle={
        folderKind
          ? "Xorvio reads these folders. It changes what's in them only when you ask, on the folder's library page, and anything deleted there waits 30 days in its Recently Deleted."
          : server
            ? `Xorvio keeps a sign-in token in ${keychainName}, never your password.`
            : `Logins and playlist links are kept in ${keychainName}, never in plain files.`
      }
      onClose={onClose}
      width={560}
      footer={
        <>
          {kind === "xtream" && (
            <button className="btn-plain source-test" disabled={test === "testing" || !(secrets.serverUrl && secrets.username && secrets.password)} onClick={testLogin}>
              {test === "testing" ? (
                <>
                  <span className="spinner small" /> Testing…
                </>
              ) : (
                "Test Connection"
              )}
            </button>
          )}
          <button className="btn-plain" onClick={onClose}>
            Cancel
          </button>
          {!(plexSignIn && !plex.servers) && (
            <button className="btn-accent" disabled={!valid || busy} onClick={save}>
              {busy ? (server && !existing ? "Signing In…" : "Checking…") : existing ? "Save" : plexSignIn ? "Add Server" : "Add Source"}
            </button>
          )}
        </>
      }
    >
      <div
        className={`source-sheet-drop ${dropping ? "dropping" : ""}`}
        onDragOver={(e) => {
          if (!takesDrops || !e.dataTransfer.types.includes("Files")) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = "copy";
          setDropping(true);
        }}
        onDragLeave={(e) => !e.currentTarget.contains(e.relatedTarget as Node | null) && setDropping(false)}
        onDrop={(e) => {
          if (!takesDrops || !e.dataTransfer.types.includes("Files")) return;
          e.preventDefault();
          takeDrop(e.dataTransfer.files[0]);
        }}
      >
        {dropping && (
          <div className="source-drop-cue" aria-hidden="true">
            <FileText size={22} />
            Drop the playlist file to use it
          </div>
        )}
        {!existing && (
          <div className="kind-picker" role="radiogroup" aria-label="Type">
            {kindGroups.map((group) => (
              <div key={group.title} className="kind-group">
                <div className="kind-group-title">{group.title}</div>
                <div className="kind-tiles">{group.kinds.map(kindTile)}</div>
              </div>
            ))}
          </div>
        )}
        {!(plexSignIn && !plex.servers) && (
          <div className="form-row">
            <label>Name</label>
            <input
              className="text-input"
              placeholder={folderKind ? (folders[0] ? folderName(folders[0]) : "My Films") : server ? ((kind === "plex" && chosenServer?.name) || "The server's own name") : "My Provider"}
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>
        )}
        {kind === "xtream" && (
          <>
            <div className="form-row">
              <label>Server</label>
              <input className="text-input" placeholder="http://provider.example:8080" value={secrets.serverUrl ?? ""} onChange={(e) => set({ serverUrl: e.target.value.trim() })} spellCheck={false} />
            </div>
            <div className="form-row">
              <label>Username</label>
              <input className="text-input" value={secrets.username ?? ""} onChange={(e) => set({ username: e.target.value.trim() })} spellCheck={false} autoComplete="off" />
            </div>
            <div className="form-row">
              <label>Password</label>
              <input className="text-input" type="password" value={secrets.password ?? ""} onChange={(e) => set({ password: e.target.value })} autoComplete="off" />
            </div>
          </>
        )}
        {server && (existing || !plexSignIn) && (
          <div className="form-row">
            <label>Server</label>
            <input
              className="text-input"
              placeholder={kind === "plex" ? "http://192.168.1.20:32400" : "http://192.168.1.20:8096"}
              value={secrets.serverUrl ?? ""}
              onChange={(e) => set({ serverUrl: e.target.value.trim() })}
              spellCheck={false}
            />
          </div>
        )}
        {(kind === "jellyfin" || kind === "emby") && !existing && (
          <>
            <div className="form-row">
              <label>Username</label>
              <input className="text-input" value={secrets.username ?? ""} onChange={(e) => set({ username: e.target.value.trim() })} spellCheck={false} autoComplete="off" />
            </div>
            <div className="form-row">
              <label>Password</label>
              <input className="text-input" type="password" placeholder="Leave empty if the account has none" value={secrets.password ?? ""} onChange={(e) => set({ password: e.target.value })} autoComplete="off" />
            </div>
          </>
        )}
        {kind === "plex" && !existing && plexByAddress && (
          <div className="form-row">
            <label>Token</label>
            <input className="text-input" type="password" placeholder="Only if the server asks for one" value={secrets.token ?? ""} onChange={(e) => set({ token: e.target.value.trim() || null })} autoComplete="off" />
          </div>
        )}
        {plexSignIn && <PlexSignInPanel plex={plex} />}
        {existing && (existing.kind === "jellyfin" || existing.kind === "emby") && (
          <div className="form-row">
            <label>Password</label>
            <input className="text-input" type="password" placeholder="Only to sign in again" value={secrets.password ?? ""} onChange={(e) => set({ password: e.target.value })} autoComplete="off" />
          </div>
        )}
        {existing?.kind === "plex" && <PlexSignInPanel plex={plex} again />}
        {kind === "plex" && !existing && (
          <button className="btn-link sheet-aside" onClick={() => setPlexByAddress((v) => !v)}>
            {plexByAddress ? "Sign in with Plex instead" : "Connect to a server by its address instead"}
          </button>
        )}
        {existing && server && (
          <p className="muted small field-note">
            {existing.kind === "plex" ? "Signed in through Plex." : `Signed in as ${secrets.username ?? "you"}.`} To sign in as someone else, remove this source and add it again.
          </p>
        )}
        {kind === "m3uUrl" && (
          <div className="form-row">
            <label>Playlist link</label>
            <input className="text-input" placeholder="https://…/playlist.m3u" value={secrets.playlistUrl ?? ""} onChange={(e) => set({ playlistUrl: e.target.value.trim() })} spellCheck={false} />
          </div>
        )}
        {kind === "m3uUrl" && xtreamInLink && (
          <p className="source-hint">
            <Lightbulb size={14} />
            <span>
              This looks like an Xtream account, which adds series details, replay and the account's status.{" "}
              <button
                className="btn-link"
                onClick={() => {
                  // The login moves across; the playlist link and its guide aren't needed then.
                  setSecrets({ serverUrl: xtreamInLink.serverUrl, username: xtreamInLink.username, password: xtreamInLink.password });
                  setKind("xtream");
                  setXtreamInLink(null);
                }}
              >
                Use Xtream?
              </button>
            </span>
          </p>
        )}
        {kind === "m3uFile" && (
          <div className="form-row">
            <label>Playlist file</label>
            <div style={{ display: "flex", gap: 8, alignItems: "center", minWidth: 0 }}>
              <button
                className="btn-plain"
                onClick={async () => {
                  const chosen = await api.chooseFile(["m3u", "m3u8", "txt"]);
                  if (!chosen) return;
                  setFilePath(chosen);
                  setDropped(null);
                }}
              >
                Choose…
              </button>
              <span className="muted ellipsis" title={filePath ?? undefined}>
                {dropped ? dropped.name : (filePath ?? "No file chosen")}
              </span>
            </div>
          </div>
        )}
        {kind === "m3uFile" && takesDrops && (
          <p className="muted small field-note">
            {dropped ? "Xorvio keeps its own copy of a dropped playlist, so later changes to the file aren't read. Choose the file instead to follow them." : "Or drop a playlist file onto this sheet."}
          </p>
        )}
        {folderKind && (
          <>
            <div className="form-row">
              <label>Folders</label>
              <div className="folder-list">
                {folders.map((f) => (
                  <div key={f} className="folder-row">
                    <Folder size={15} />
                    <span className="ellipsis" title={f}>
                      {f}
                    </span>
                    <button className="icon-button" aria-label={`Remove ${folderName(f)}`} onClick={() => setFolders((list) => list.filter((x) => x !== f))}>
                      <X size={14} />
                    </button>
                  </div>
                ))}
                <button className="btn-plain" onClick={addFolders}>
                  <FolderPlus size={13} /> {folders.length ? "Add another folder…" : "Choose a folder…"}
                </button>
              </div>
            </div>
            <p className="muted small field-note">
              Films and series are found from their file and folder names, with any .nfo files and artwork beside them. Folders on a drive or {networkShare} work too.
            </p>
          </>
        )}
        {(kind === "m3uUrl" || kind === "m3uFile") && (
          <div className="form-row">
            <label>Guide (XMLTV)</label>
            <input className="text-input" placeholder="Optional — found in the playlist when it has one" value={secrets.epgUrl ?? ""} onChange={(e) => set({ epgUrl: e.target.value.trim() || null })} spellCheck={false} />
          </div>
        )}
        {!(plexSignIn && !plex.servers) && (
          <>
            <div className="form-row">
              <label>Refresh</label>
              <select className="select-input source-refresh" value={refreshInterval} onChange={(e) => setRefreshInterval(e.target.value as RefreshInterval)} aria-label="Refresh">
                <option value="daily">Daily</option>
                <option value="every6Hours">Every 6 hours</option>
                <option value="manual">Manually</option>
              </select>
            </div>
            <p className="muted small field-note">
              {folderKind ? "Folders are also read each time Xorvio opens." : refreshInterval === "manual" ? "Only when you refresh it in Sources." : `While Xorvio is open, or running in ${trayName}.`}
            </p>
          </>
        )}
        {test && test !== "testing" && (
          <p className={test.ok ? "form-success" : "form-error"}>
            {test.ok ? <CircleCheck size={13} /> : <TriangleAlert size={13} />} {test.text}
          </p>
        )}
        {!server && !folderKind && (
          <details className="advanced">
            <summary>Advanced</summary>
            <div className="form-row">
              <label>Live streams</label>
              <select className="select-input" value={liveFormat} onChange={(e) => setLiveFormat(e.target.value as LiveStreamFormat)}>
                <option value="hls">HLS (.m3u8) — recommended</option>
                <option value="mpegts">MPEG-TS (.ts)</option>
              </select>
            </div>
            <div className="form-row">
              <label>User agent</label>
              <input className="text-input" placeholder="Only if your provider asks for one" value={userAgent} onChange={(e) => setUserAgent(e.target.value)} spellCheck={false} />
            </div>
            {existing && (
              <div className="form-row">
                <label>Guide offset</label>
                <OffsetPicker minutes={guideOffset} onChange={setGuideOffset} />
              </div>
            )}
          </details>
        )}
        {error && (
          <p className="form-error">
            <TriangleAlert size={13} /> {error}
          </p>
        )}
      </div>
    </Sheet>
  );
}

// MARK: Plex sign-in

type PlexSignIn = ReturnType<typeof usePlexSignIn>;

/**
 * Signing in through plex.tv: a page opens in the browser where the person
 * signs in and approves Xorvio, while this asks every two seconds whether
 * they have. Then the account's servers are offered to choose from. The
 * account's token stays in the app's backend.
 */
function usePlexSignIn(onError: (message: string | null) => void) {
  const [pin, setPin] = useState<PlexSignInStart | null>(null);
  const [servers, setServers] = useState<PlexServerChoice[] | null>(null);
  const [chosen, setChosen] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);

  const start = async () => {
    setStarting(true);
    onError(null);
    setServers(null);
    try {
      const fresh = await api.plexStartSignIn();
      setPin(fresh);
      await api.openExternal(fresh.authUrl);
    } catch (e) {
      onError(errorText(e));
    } finally {
      setStarting(false);
    }
  };

  useEffect(() => {
    if (!pin || servers) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const check = async () => {
      try {
        const found = await api.plexCheckSignIn(pin.pinId);
        if (stopped) return;
        if (found) {
          setServers(found);
          setChosen(found.find((s) => s.owned)?.id ?? found[0]?.id ?? null);
          return;
        }
      } catch (e) {
        if (stopped) return;
        onError(errorText(e));
        setPin(null);
        return;
      }
      timer = setTimeout(check, 2000);
    };
    timer = setTimeout(check, 2000);
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pin, servers]);

  const cancel = () => {
    setPin(null);
    setServers(null);
    setChosen(null);
  };
  return { pin, servers, chosen, setChosen, starting, start, cancel };
}

/** `again`: signing an existing Plex source in again, rather than choosing a server to add. */
function PlexSignInPanel({ plex, again = false }: { plex: PlexSignIn; again?: boolean }) {
  if (plex.servers && again) {
    return (
      <div className="plex-panel">
        <p className="muted small">Signed in to Plex again. Save to use the new sign-in.</p>
      </div>
    );
  }
  if (plex.servers) {
    if (!plex.servers.length) {
      return (
        <div className="plex-panel">
          <p>This Plex account has no servers yet. Set one up in Plex Media Server, or ask someone to share theirs with you.</p>
          <button className="btn-plain" onClick={plex.cancel}>
            Use Another Account
          </button>
        </div>
      );
    }
    return (
      <div className="form-row">
        <label>Server</label>
        <div className="server-choices">
          {plex.servers.map((s) => (
            <label key={s.id} className={`server-choice ${plex.chosen === s.id ? "selected" : ""}`}>
              <input type="radio" name="plex-server" checked={plex.chosen === s.id} onChange={() => plex.setChosen(s.id)} />
              <HardDrive size={15} />
              <span className="ellipsis">{s.name}</span>
              {!s.owned && <span className="badge">SHARED</span>}
            </label>
          ))}
        </div>
      </div>
    );
  }
  if (plex.pin) {
    return (
      <div className="plex-panel">
        <div className="plex-waiting">
          <div className="spinner small" />
          <div>
            <strong>Approve Xorvio in your browser</strong>
            <p className="muted small">
              Sign in to Plex on the page that opened and allow Xorvio. Your code is <span className="plex-code">{plex.pin.code}</span>.
            </p>
          </div>
        </div>
        <div className="plex-actions">
          <button className="btn-plain" onClick={() => api.openExternal(plex.pin!.authUrl)}>
            Open the Plex Page Again
          </button>
          <button className="btn-plain" onClick={plex.cancel}>
            Cancel
          </button>
        </div>
      </div>
    );
  }
  return (
    <div className="plex-panel">
      <p className="muted small">
        {again
          ? "If Xorvio has been signed out of this server, sign in with Plex again."
          : "Sign in with your Plex account to choose from your servers and the ones shared with you. Your browser opens plex.tv to sign in; Xorvio never sees your password."}
      </p>
      <button className={again ? "btn-plain" : "btn-accent"} disabled={plex.starting} onClick={plex.start}>
        {plex.starting ? "Opening Plex…" : again ? "Sign In with Plex Again" : "Sign In with Plex"}
      </button>
    </div>
  );
}

// MARK: Guides

/** Guides added on their own, beside the ones sources bring. */
function GuidesSection() {
  const [guides, setGuides] = useState<GuideSourceSummary[] | null>(null);
  const [editing, setEditing] = useState<GuideSource | "new" | null>(null);
  const showToast = useApp((s) => s.showToast);

  const load = useCallback(() => api.guideSources().then(setGuides).catch(() => setGuides([])), []);
  useEffect(() => {
    load();
  }, [load]);

  const refresh = async (id: string) => {
    setGuides((list) => list?.map((g) => (g.guide.id === id ? { ...g, refreshing: true } : g)) ?? null);
    try {
      await api.refreshGuideSource(id);
      useApp.getState().guideChanged();
    } catch (e) {
      showToast(errorText(e), "error");
    } finally {
      load();
    }
  };

  const remove = async (g: GuideSourceSummary) => {
    const ok = await confirmAction({
      title: `Remove “${g.guide.name}”?`,
      message: "Its listings leave the guide, and channels matched to it by hand find their own guide again.",
      confirmLabel: "Remove",
      destructive: true,
    });
    if (!ok) return;
    await api.removeGuideSource(g.guide.id).catch((e) => showToast(errorText(e), "error"));
    useApp.getState().guideChanged();
    load();
  };

  return (
    <section className="guides-section">
      <div className="page-header">
        <div>
          <h2 className="section-title">Guides</h2>
          <p className="page-subtitle">Extra XMLTV guides fill in channels your sources' guides miss, matched by guide id and then by name. To choose a channel's guide yourself, right-click it in Live TV and choose Match Guide.</p>
        </div>
        <button className="btn-plain" onClick={() => setEditing("new")}>
          <Plus size={14} /> Add Guide
        </button>
      </div>
      <div className="source-list">
        {guides?.map((g) => {
          const st = g.status;
          return (
            <div key={g.guide.id} className="source-card">
              <div className="source-icon">
                <CalendarClock size={18} />
              </div>
              <div className="source-text">
                <div className="source-name">{g.guide.name}</div>
                <div className="source-meta">
                  XMLTV Guide
                  {g.guide.displayHost ? ` · ${g.guide.displayHost}` : ""}
                  {g.guide.offsetMinutes ? ` · ${offsetLabel(g.guide.offsetMinutes)}` : ""}
                  {st.guideLastSuccess ? ` · Updated ${shortDate(st.guideLastSuccess)}` : ""}
                </div>
                <div className="source-counts">{st.guideLastSuccess ? `${st.programCount.toLocaleString()} listings` : "Not loaded yet"}</div>
                {g.refreshing && (
                  <div className="source-progress">
                    <div className="spinner small" /> Loading guide…
                  </div>
                )}
                {st.guideLastError && !g.refreshing && (
                  <div className="source-error">
                    <TriangleAlert size={12} /> {st.guideLastError}
                  </div>
                )}
              </div>
              <div className="source-actions">
                <button className="btn-plain" disabled={g.refreshing} onClick={() => refresh(g.guide.id)}>
                  <RefreshCw size={13} /> Refresh
                </button>
                <button
                  className="icon-button"
                  aria-label="More"
                  onClick={(e) =>
                    openMenu(
                      e,
                      [
                        { label: "Edit…", icon: <Pencil size={15} />, onSelect: () => setEditing(g.guide) },
                        { separator: true },
                        { label: "Remove…", icon: <Trash2 size={15} />, destructive: true, onSelect: () => remove(g) },
                      ],
                      true,
                    )
                  }
                >
                  •••
                </button>
              </div>
            </div>
          );
        })}
      </div>
      {editing && (
        <GuideSheet
          existing={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={(id) => {
            setEditing(null);
            load();
            refresh(id);
          }}
        />
      )}
    </section>
  );
}

function GuideSheet({ existing, onClose, onSaved }: { existing: GuideSource | null; onClose: () => void; onSaved: (refreshId: string) => void }) {
  const [name, setName] = useState(existing?.name ?? "");
  const [url, setUrl] = useState("");
  const [offset, setOffset] = useState(existing?.offsetMinutes ?? 0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (existing) api.guideSourceLink(existing.id).then((link) => setUrl(link ?? "")).catch(() => {});
  }, [existing]);

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      if (existing) {
        await api.updateGuideSource({ ...existing, name: name.trim() || existing.name, offsetMinutes: offset }, url.trim() || null);
        onSaved(existing.id);
      } else {
        const added = await api.addGuideSource({ name: name.trim(), url: url.trim(), offsetMinutes: offset });
        onSaved(added.id);
      }
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet
      title={existing ? `Edit ${existing.name}` : "Add a Guide"}
      subtitle={`Guide links are kept in ${keychainName}, never in plain files.`}
      onClose={onClose}
      width={560}
      footer={
        <>
          <button className="btn-plain" onClick={onClose}>
            Cancel
          </button>
          <button className="btn-accent" disabled={(!existing && !url.trim()) || busy} onClick={save}>
            {busy ? "Saving…" : existing ? "Save" : "Add Guide"}
          </button>
        </>
      }
    >
      <div className="form-row">
        <label>Name</label>
        <input className="text-input" placeholder="My Guide" value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="form-row">
        <label>Guide link (XMLTV)</label>
        <input className="text-input" placeholder="https://…/guide.xml.gz" value={url} onChange={(e) => setUrl(e.target.value)} spellCheck={false} />
      </div>
      <div className="form-row">
        <label>Offset</label>
        <OffsetPicker minutes={offset} onChange={setOffset} />
      </div>
      {error && (
        <p className="form-error">
          <TriangleAlert size={13} /> {error}
        </p>
      )}
    </Sheet>
  );
}

/** Moves a guide's times, in half hours up to half a day either way. */
function OffsetPicker({ minutes, onChange }: { minutes: number; onChange: (minutes: number) => void }) {
  const choices = Array.from({ length: 49 }, (_, i) => (i - 24) * 30);
  if (!choices.includes(minutes)) choices.push(minutes);
  choices.sort((a, b) => a - b);
  return (
    <div className="offset-picker">
      <select className="select-input" value={minutes} onChange={(e) => onChange(Number(e.target.value))}>
        {choices.map((m) => (
          <option key={m} value={m}>
            {m === 0 ? "None" : offsetLabel(m)}
          </option>
        ))}
      </select>
      <span className="muted small">For a guide written for another time zone. Applies when the guide next loads.</span>
    </div>
  );
}

/** "1 hr 30 min later", "30 min earlier". */
function offsetLabel(minutes: number) {
  return `${duration(Math.abs(minutes) * 60)} ${minutes > 0 ? "later" : "earlier"}`;
}

function defaultName(kind: SourceKind, secrets: SourceSecrets) {
  // The app names a media server source after the server.
  if (isMediaServer(kind)) return "";
  try {
    const url = kind === "xtream" ? secrets.serverUrl : secrets.playlistUrl;
    if (url) return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    // fall through
  }
  return kind === "xtream" ? "Xtream Provider" : "Playlist";
}
