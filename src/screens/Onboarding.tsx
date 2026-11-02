// First run, in three short steps, as Xorvio for Mac had them: what Xorvio
// is (and that it includes no content), connecting a source or the demo,
// then the library as it loads, with a choice of languages and whether to
// hide adult categories, kept as a library profile. It shows once, while
// there are no sources, over the whole window; every step can be left.

import { Check, ChevronLeft, CircleAlert, Clapperboard, Film, Folder, HardDrive, Info, Link2, Monitor, Radio, Server, Sparkles, Tv, CalendarDays } from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { BrandMark } from "../components/Artwork";
import { api, errorText } from "../lib/api";
import { keychainName, networkShare } from "../lib/platform";
import { LANGUAGE_NAMES, newProfile, useProfiles } from "../lib/profiles";
import { useSources } from "../lib/sources";
import { useApp } from "../lib/store";
import type { ContentLanguage, ProfileCategory, SourceKind } from "../lib/types";
import { isPopOutWindow } from "../lib/windows";
import { SourceSheet } from "./Sources";
import "../styles/onboarding.css";

/** Shows first run's welcome once, when there are no sources, until it's finished or skipped. */
export function OnboardingHost({ ready }: { ready: boolean }) {
  const done = useApp((s) => s.settings.onboardingDone);
  const sources = useSources((s) => s.sources);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (ready && !done && sources?.length === 0) setOpen(true);
  }, [ready, done, sources]);
  if (!open || isPopOutWindow) return null;
  return (
    <Onboarding
      finish={(then) => {
        useApp.getState().updateSettings({ onboardingDone: true });
        setOpen(false);
        useApp.getState().go(then);
      }}
    />
  );
}

type Step = "welcome" | "connect" | "library";

function Onboarding({ finish }: { finish: (then: "home" | "sources") => void }) {
  const [step, setStep] = useState<Step>("welcome");
  const [sourceId, setSourceId] = useState<string | null>(null);
  const [adding, setAdding] = useState<SourceKind | null>(null);

  /** A source is in: on to the library while it loads. */
  const connected = async (ids: string[]) => {
    if (!ids.length) return;
    setSourceId(ids[0]);
    setStep("library");
    await useSources.getState().load();
    ids.forEach((id) => useSources.getState().refresh(id));
  };

  return (
    <div className="onboarding" role="dialog" aria-modal="true" aria-label="Welcome to Xorvio">
      <div className="onboarding-drag" data-tauri-drag-region />
      <div className="onboarding-scroll">
        <div className="onboarding-step" key={step}>
          {step === "welcome" && <WelcomeStep next={() => setStep("connect")} skip={() => finish("home")} />}
          {step === "connect" && <ConnectStep back={() => setStep("welcome")} add={setAdding} connected={connected} skip={() => finish("home")} />}
          {step === "library" && sourceId && <LibraryStep sourceId={sourceId} again={() => setStep("connect")} finish={finish} />}
        </div>
      </div>
      {adding && (
        <SourceSheet
          existing={null}
          initialKind={adding}
          onClose={() => setAdding(null)}
          onSaved={(id) => {
            setAdding(null);
            if (id) connected([id]);
          }}
        />
      )}
    </div>
  );
}

// MARK: Welcome

function WelcomeStep({ next, skip }: { next: () => void; skip: () => void }) {
  return (
    <div className="onboarding-welcome">
      <div className="onboarding-hero">
        <div className="onboarding-mark" aria-hidden="true">
          <BrandMark size={52} />
        </div>
        <h1 className="onboarding-title">Welcome to Xorvio</h1>
        <p className="onboarding-lead">Live TV, films and series from the sources you already have, and the videos on your computer, in an app built for watching.</p>
      </div>
      <div className="onboarding-features">
        <Feature icon={<Radio size={20} />} title="A real TV guide">
          Browse by channel and time, record what's on or coming up, and replay what you missed where your provider offers it.
        </Feature>
        <Feature icon={<Clapperboard size={20} />} title="Films and series">
          Title pages with cast and episodes, Continue Watching, and the next episode ready when one ends.
        </Feature>
        <Feature icon={<Monitor size={20} />} title="Watch your way">
          Filling the window, in the corner while you browse, in a window of its own, or up to four channels at once.
        </Feature>
      </div>
      <ContentNotice />
      <div className="onboarding-actions centred">
        <button className="btn btn-primary onboarding-go" onClick={next} autoFocus>
          Get Started
        </button>
        <button className="btn-link" onClick={skip}>
          Skip for now
        </button>
      </div>
    </div>
  );
}

/** Xorvio is a player: said plainly, before anything is connected. */
function ContentNotice() {
  return (
    <div className="onboarding-notice">
      <Info size={18} />
      <div>
        <strong>Xorvio doesn't include any content</strong>
        <p>
          It's a player. It doesn't include, sell or recommend channels, films or series. You add folders of your own videos, your own media server, or playlists and logins you already have from a provider you're entitled to use; logins stay in {keychainName}. There's no account, and no viewing data is collected.
        </p>
      </div>
    </div>
  );
}

function Feature({ icon, title, children }: { icon: ReactNode; title: string; children: ReactNode }) {
  return (
    <div className="onboarding-card onboarding-feature">
      <span className="onboarding-icon">{icon}</span>
      <h3>{title}</h3>
      <p>{children}</p>
    </div>
  );
}

// MARK: Connect

function ConnectStep({ back, add, connected, skip }: { back: () => void; add: (kind: SourceKind) => void; connected: (ids: string[]) => void; skip: () => void }) {
  const [canImport, setCanImport] = useState(false);
  const [busy, setBusy] = useState<"demo" | "import" | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.swiftImportAvailable().then(setCanImport).catch(() => {});
  }, []);

  const run = async (what: "demo" | "import", work: () => Promise<string[]>) => {
    setBusy(what);
    setError(null);
    try {
      await connected(await work());
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="onboarding-connect">
      <div className="onboarding-heading">
        <button className="btn-link onboarding-back" onClick={back}>
          <ChevronLeft size={14} /> Back
        </button>
        <h1 className="onboarding-title small">Connect a source</h1>
        <p className="onboarding-lead">Your own videos or media server, or the details your provider gave you. You can add more later in Sources.</p>
      </div>
      {canImport && (
        <div className="import-banner">
          <div>
            <strong>Bring your sources over from Xorvio for Mac</strong>
            <p className="muted small">Your provider logins are copied from the Mac app's keychain. macOS will ask you to allow it.</p>
          </div>
          <button className="btn-accent" disabled={!!busy} onClick={() => run("import", async () => (await api.importSwiftSources()).map((s) => s.id))}>
            {busy === "import" ? "Importing…" : "Import"}
          </button>
        </div>
      )}
      <div className="onboarding-choices">
        <Choice icon={<Server size={22} />} title="Xtream login" action="Enter Login…" primary onChoose={() => add("xtream")}>
          A server address, username and password. Most providers offer this; it adds series details, replay and the account's status.
        </Choice>
        <Choice icon={<Link2 size={22} />} title="M3U playlist" action="Add Playlist…" onChoose={() => add("m3uUrl")}>
          A playlist link or file from your provider, with a guide (XMLTV) link if it has one.
        </Choice>
        <Choice icon={<HardDrive size={22} />} title="Media server" action="Connect Server…" onChoose={() => add("plex")}>
          Your own Plex, Jellyfin or Emby server: its films and series, and what you've watched on it.
        </Choice>
        <Choice icon={<Folder size={22} />} title="Media folder" action="Choose Folder…" onChoose={() => add("folder")}>
          Films and series already on this computer, a drive, or {networkShare}.
        </Choice>
      </div>
      <div className="onboarding-card onboarding-demo">
        <Sparkles size={20} />
        <div>
          <strong>Just looking?</strong>
          <p>Explore a demo library of made-up channels, films and series, always marked as demo content.</p>
        </div>
        <button
          className="btn-plain"
          disabled={!!busy}
          onClick={() =>
            run("demo", async () => {
              await api.addDemoSource();
              return ["demo"];
            })
          }
        >
          {busy === "demo" ? "Opening…" : "Open Demo"}
        </button>
      </div>
      {error && (
        <p className="form-error">
          <CircleAlert size={13} /> {error}
        </p>
      )}
      <div className="onboarding-actions">
        <button className="btn-link" onClick={skip}>
          Skip for now
        </button>
      </div>
    </div>
  );
}

function Choice({ icon, title, action, primary = false, onChoose, children }: { icon: ReactNode; title: string; action: string; primary?: boolean; onChoose: () => void; children: ReactNode }) {
  return (
    <div className="onboarding-card onboarding-choice">
      <span className="onboarding-icon large">{icon}</span>
      <h3>{title}</h3>
      <p>{children}</p>
      <button className={primary ? "btn-accent" : "btn-plain"} onClick={onChoose}>
        {action}
      </button>
    </div>
  );
}

// MARK: Library

const LIBRARY_KINDS: SourceKind[] = ["folder", "plex", "jellyfin", "emby"];

function LibraryStep({ sourceId, again, finish }: { sourceId: string; again: () => void; finish: (then: "home" | "sources") => void }) {
  const summary = useSources((s) => s.sources?.find((source) => source.config.id === sourceId));
  const progress = useSources((s) => s.progress.get(sourceId));
  const running = !!progress || !!summary?.refreshing;
  const status = summary?.status;
  const ready = !running && !!status?.lastSuccess;
  const failed = !running && !status?.lastSuccess ? (status?.lastError ?? null) : null;
  // A folder or media server has films and series only, so it isn't shown channels as missing.
  const library = !!summary && LIBRARY_KINDS.includes(summary.config.kind);

  const [categories, setCategories] = useState<ProfileCategory[] | null>(null);
  useEffect(() => {
    if (ready) api.profileCategories().then(setCategories).catch(() => setCategories([]));
  }, [ready]);
  const own = useMemo(() => (categories ?? []).filter((c) => c.sourceId === sourceId), [categories, sourceId]);
  /** The languages the source's categories are labelled with, most categories first. */
  const detected = useMemo(() => {
    const counts = new Map<ContentLanguage, number>();
    for (const c of own) for (const language of c.languages) counts.set(language, (counts.get(language) ?? 0) + 1);
    return [...counts.entries()].sort((a, b) => b[1] - a[1]);
  }, [own]);
  const hasAdult = own.some((c) => c.adult);

  const [chosen, setChosen] = useState<Set<ContentLanguage> | null>(null);
  const [hideAdult, setHideAdult] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Start with the languages this computer is set to, when the source has them; otherwise all of them.
  useEffect(() => {
    if (chosen || !detected.length) return;
    const found = detected.map(([language]) => language);
    const preferred = new Set(systemLanguages().filter((l) => found.includes(l)));
    setChosen(preferred.size ? preferred : new Set(found));
  }, [detected, chosen]);

  const toggle = (language: ContentLanguage) =>
    setChosen((now) => {
      const next = new Set(now ?? []);
      if (next.has(language)) next.delete(language);
      else next.add(language);
      return next;
    });

  /** Keeps the choices as a library profile, in use from now on, then opens Home. */
  const start = async () => {
    const all = detected.map(([language]) => language);
    const languages = all.length > 1 && chosen && chosen.size && chosen.size < all.length ? all.filter((l) => chosen.has(l)) : [];
    const hide = hasAdult && hideAdult;
    if (languages.length || hide) {
      setSaving(true);
      setError(null);
      try {
        const { save, switchTo } = useProfiles.getState();
        const saved = await save({ ...newProfile("My Library"), languages, hideAdult: hide });
        await switchTo(saved.id);
      } catch (e) {
        setSaving(false);
        setError(`Your choices couldn't be kept. ${errorText(e)}`);
        return;
      }
    }
    finish("home");
  };

  const title = ready ? "Your library is ready" : failed ? "That didn't connect" : "Setting up your library";
  const lead = ready
    ? "Pick what you'd like to see, then start watching. You can change any of this later in Settings › Profiles and Home."
    : failed
      ? "Xorvio couldn't load this source."
      : `${progress?.phase ?? "Connecting"}… Large libraries take a minute or two.`;

  return (
    <div className="onboarding-library">
      <div className="onboarding-heading">
        <h1 className="onboarding-title small">{title}</h1>
        <p className="onboarding-lead">{lead}</p>
      </div>
      <div className="onboarding-counts">
        {!library && <Count icon={<Tv size={18} />} value={status?.channelCount} label="Channels" />}
        <Count icon={<Film size={18} />} value={status?.movieCount} label="Films" />
        <Count icon={<Clapperboard size={18} />} value={status?.seriesCount} label="Series" />
        {!library && <Count icon={<CalendarDays size={18} />} value={status?.programCount} label="Guide listings" />}
      </div>
      {running && <div className="spinner onboarding-spinner" aria-label="Loading" />}
      {failed && (
        <div className="onboarding-problem">
          <CircleAlert size={16} />
          <div>
            <strong>{failed}</strong>
            <p>Check the details in Sources, where you can edit or replace the source, or choose another one.</p>
          </div>
        </div>
      )}
      {ready && (detected.length > 1 || hasAdult) && (
        <div className="onboarding-card onboarding-tidy">
          {detected.length > 1 && (
            <>
              <h3>Which languages do you watch?</h3>
              <p>Your source labels its categories in {detected.length} languages. Xorvio shows only the ones you pick, and categories with no language in their name.</p>
              <div className="onboarding-chips">
                {detected.map(([language, count]) => {
                  const on = !!chosen?.has(language);
                  return (
                    <button key={language} className={`onboarding-chip ${on ? "on" : ""}`} aria-pressed={on} onClick={() => toggle(language)}>
                      {on && <Check size={12} strokeWidth={3} />}
                      {LANGUAGE_NAMES[language]} · {count}
                    </button>
                  );
                })}
              </div>
            </>
          )}
          {hasAdult && (
            <>
              <label className="toggle-row onboarding-adult">
                <button role="switch" aria-checked={hideAdult} className={`toggle ${hideAdult ? "on" : ""}`} onClick={() => setHideAdult((on) => !on)} />
                <span>Hide adult categories</span>
              </label>
              <p className="muted small">Parental Controls in Settings can also lock categories behind a PIN.</p>
            </>
          )}
        </div>
      )}
      {error && (
        <p className="form-error">
          <CircleAlert size={13} /> {error}
        </p>
      )}
      <div className="onboarding-actions end">
        {failed ? (
          <>
            <button className="btn-plain" onClick={again}>
              Choose Another Source
            </button>
            <button className="btn btn-primary onboarding-go" onClick={() => finish("sources")}>
              Open Sources
            </button>
          </>
        ) : (
          <button className="btn btn-primary onboarding-go" disabled={saving} onClick={start} autoFocus>
            {saving ? "Saving…" : "Start Watching"}
          </button>
        )}
      </div>
    </div>
  );
}

function Count({ icon, value, label }: { icon: ReactNode; value: number | undefined; label: string }) {
  return (
    <div className="onboarding-card onboarding-count">
      <span className="onboarding-count-icon">{icon}</span>
      <span className={`onboarding-count-value ${value ? "" : "none"}`}>{value ? value.toLocaleString() : "–"}</span>
      <span className="onboarding-count-label">{label}</span>
    </div>
  );
}

/** The library languages for the languages this computer is set to ("en-GB" is English). */
function systemLanguages(): ContentLanguage[] {
  const map: Record<string, ContentLanguage> = {
    en: "english",
    fr: "french",
    es: "spanish",
    pt: "portuguese",
    de: "german",
    it: "italian",
    nl: "dutch",
    sv: "nordic",
    no: "nordic",
    nb: "nordic",
    nn: "nordic",
    da: "nordic",
    fi: "nordic",
    is: "nordic",
    pl: "polish",
    tr: "turkish",
    ar: "arabic",
    hi: "southAsian",
    ur: "southAsian",
    bn: "southAsian",
    pa: "southAsian",
    ta: "southAsian",
    te: "southAsian",
    gu: "southAsian",
    mr: "southAsian",
    ru: "russian",
    uk: "ukrainian",
    el: "greek",
    sq: "albanian",
    sr: "balkan",
    hr: "balkan",
    bs: "balkan",
    sl: "balkan",
    mk: "balkan",
    hu: "hungarian",
    ro: "romanian",
    cs: "czechSlovak",
    sk: "czechSlovak",
    bg: "bulgarian",
    fa: "persian",
    he: "hebrew",
    ko: "korean",
    ja: "japanese",
    zh: "chinese",
    fil: "filipino",
    tl: "filipino",
    vi: "vietnamese",
    th: "thai",
  };
  const tags = typeof navigator === "undefined" ? [] : navigator.languages?.length ? navigator.languages : [navigator.language];
  return [...new Set(tags.map((tag) => map[tag.toLowerCase().split("-")[0]]).filter((l): l is ContentLanguage => !!l))];
}
