// Library profiles: the sidebar's switcher, and the sheet for making and
// changing them, with counts read from the provider's category names (as
// Xorvio for Mac's ProfilesView). Changes save as they're made.

import { Check, ChevronsUpDown, Plus, Search, Trash2, TriangleAlert } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, errorText } from "../lib/api";
import { EVERYTHING_ID, GENRE_NAMES, GENRES, includes, KIND_NAMES, LANGUAGE_NAMES, LANGUAGES, newProfile, profileTotals, useActiveProfile, useProfiles } from "../lib/profiles";
import { useApp } from "../lib/store";
import type { CategoryKind, ContentGenre, ContentLanguage, LibraryProfile, ProfileCategory } from "../lib/types";
import { placeholderGradient } from "./Artwork";
import { confirmAction } from "./Confirm";
import { openMenu, type MenuEntry } from "./Menu";
import { Sheet } from "./Sheet";
import "../styles/profiles.css";

const count = (n: number) => n.toLocaleString();

export function ProfileAvatar({ name, size = 22 }: { name: string; size?: number }) {
  return (
    <span className="profile-avatar" style={{ width: size, height: size, fontSize: size * 0.48, background: placeholderGradient(name) }} aria-hidden="true">
      {name.trim().slice(0, 1).toUpperCase() || "?"}
    </span>
  );
}

/** At the foot of the sidebar: the profile in use, and a menu to switch. */
export function ProfileSwitcher() {
  const profile = useActiveProfile();
  const profiles = useProfiles((s) => s.profiles);
  const activeId = useProfiles((s) => s.activeId);
  if (!profile) return null;
  const menu = (e: React.MouseEvent) => {
    const { switchTo, editProfiles, customizeHome } = useProfiles.getState();
    const items: MenuEntry[] = [
      { heading: "Library profile" },
      ...profiles.map((p): MenuEntry => ({ label: p.name, icon: p.id === activeId ? <Check size={15} strokeWidth={2} /> : <span style={{ width: 15 }} />, onSelect: () => switchTo(p.id) })),
      { separator: true },
      { label: "Edit profiles…", onSelect: () => editProfiles() },
      { label: "Customize Home…", onSelect: () => customizeHome(true) },
    ];
    // The menu opens upwards from the button, which sits at the foot of the window.
    const rect = e.currentTarget.getBoundingClientRect();
    openMenu({ clientX: rect.left, clientY: rect.top - 4, preventDefault: () => e.preventDefault(), stopPropagation: () => e.stopPropagation() }, items);
  };
  return (
    <div className="sidebar-footer">
      <button className="profile-switcher" onClick={menu} title="Library profile: choose which languages, genres and content show" aria-label={`Library profile, ${profile.name}`}>
        <ProfileAvatar name={profile.name} />
        <span className="profile-switcher-name">{profile.name}</span>
        <ChevronsUpDown size={14} strokeWidth={1.8} />
      </button>
    </div>
  );
}

export function ProfilesSheetHost() {
  const editing = useProfiles((s) => s.editing);
  if (editing === null) return null;
  return <ProfilesSheet initial={editing} />;
}

function ProfilesSheet({ initial }: { initial: string }) {
  const profiles = useProfiles((s) => s.profiles);
  const activeId = useProfiles((s) => s.activeId);
  const [selected, setSelected] = useState(initial || activeId);
  const [categories, setCategories] = useState<ProfileCategory[] | null>(null);
  const flush = useRef<(() => Promise<void>) | null>(null);

  useEffect(() => {
    api.profileCategories().then(setCategories).catch(() => setCategories([]));
  }, []);

  const profile = profiles.find((p) => p.id === selected) ?? profiles.find((p) => p.id === activeId) ?? profiles[0];
  const close = async () => {
    await flush.current?.();
    useProfiles.getState().closeEditor();
  };
  const choose = async (id: string) => {
    await flush.current?.();
    setSelected(id);
  };
  const add = async () => {
    await flush.current?.();
    const taken = new Set(profiles.map((p) => p.name.toLowerCase()));
    let name = "New profile";
    for (let n = 2; taken.has(name.toLowerCase()); n++) name = `New profile ${n}`;
    try {
      const saved = await useProfiles.getState().save(newProfile(name));
      setSelected(saved.id);
    } catch (e) {
      useApp.getState().showToast(errorText(e), "error");
    }
  };

  return (
    <Sheet
      title="Profiles"
      subtitle="A profile chooses which languages, genres and kinds of content show across Xorvio. Switch profiles from the foot of the sidebar."
      onClose={close}
      width={860}
      footer={
        <button className="btn-accent" onClick={close}>
          Done
        </button>
      }
    >
      <div className="profiles-sheet">
        <div className="profiles-list">
          {profiles.map((p) => (
            <button key={p.id} className={`profiles-list-item ${p.id === profile?.id ? "selected" : ""}`} onClick={() => choose(p.id)}>
              <ProfileAvatar name={p.name} />
              <span className="profile-switcher-name">{p.name}</span>
              {p.id === activeId && <Check size={14} strokeWidth={2.4} className="profiles-in-use" aria-label="In use" />}
            </button>
          ))}
          <button className="btn-plain profiles-add" onClick={add}>
            <Plus size={14} /> New profile
          </button>
        </div>
        {profile && <ProfileForm key={profile.id} profile={profile} categories={categories} canDelete={profiles.length > 1} flush={flush} onDeleted={() => setSelected(useProfiles.getState().activeId)} />}
      </div>
    </Sheet>
  );
}

function ProfileForm({
  profile: saved,
  categories,
  canDelete,
  flush,
  onDeleted,
}: {
  profile: LibraryProfile;
  categories: ProfileCategory[] | null;
  canDelete: boolean;
  flush: React.MutableRefObject<(() => Promise<void>) | null>;
  onDeleted: () => void;
}) {
  const activeId = useProfiles((s) => s.activeId);
  const parental = useApp((s) => s.parental);
  const [draft, setDraft] = useState(saved);
  const [error, setError] = useState<string | null>(null);
  const pending = useRef<LibraryProfile | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);

  // Changes save a moment after the last one, and before the sheet closes or another profile is chosen.
  const save = async () => {
    clearTimeout(timer.current);
    const next = pending.current;
    pending.current = null;
    if (!next) return;
    if (!next.name.trim()) return setError("A profile needs a name.");
    try {
      await useProfiles.getState().save(next);
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  };
  flush.current = save;
  useEffect(() => () => clearTimeout(timer.current), []);

  const change = (patch: Partial<LibraryProfile>) => {
    const next = { ...draft, ...patch };
    setDraft(next);
    pending.current = next;
    clearTimeout(timer.current);
    timer.current = setTimeout(save, 450);
  };

  const all = categories ?? [];
  const languageCounts = useMemo(() => tally(all, (c) => c.languages), [all]);
  const genreCounts = useMemo(() => tally(all, (c) => c.genres), [all]);
  const kindCounts = useMemo(() => {
    const totals: Record<CategoryKind, number> = { live: 0, movie: 0, series: 0 };
    for (const c of all) totals[c.kind] += c.itemCount;
    return totals;
  }, [all]);
  const detected = LANGUAGES.filter((l) => languageCounts.has(l) || draft.languages.includes(l));
  const unlabeled = all.filter((c) => !c.languages.length);
  const adult = all.filter((c) => c.adult);
  const totals = profileTotals(draft, all);
  const everyLanguage = draft.languages.length === 0;

  const toggleLanguage = (language: ContentLanguage) => {
    const languages = draft.languages.includes(language) ? draft.languages.filter((l) => l !== language) : [...draft.languages, language];
    change({ languages });
  };
  const toggleGenre = (genre: ContentGenre) => {
    const genres = draft.genres.includes(genre) ? draft.genres.filter((g) => g !== genre) : [...draft.genres, genre];
    change({ genres, onlyGenres: genres.length ? draft.onlyGenres : false });
  };
  const toggleKind = (kind: CategoryKind) => {
    const hiddenKinds = draft.hiddenKinds.includes(kind) ? draft.hiddenKinds.filter((k) => k !== kind) : [...draft.hiddenKinds, kind];
    change({ hiddenKinds });
  };
  const remove = async () => {
    const ok = await confirmAction({
      title: `Delete “${draft.name}”?`,
      message: "Its choices and its Home layout go. Your watchlist, favorites and history stay, and so does everything in your sources.",
      confirmLabel: "Delete",
      destructive: true,
    });
    if (!ok) return;
    pending.current = null;
    clearTimeout(timer.current);
    try {
      await useProfiles.getState().remove(draft.id);
      onDeleted();
    } catch (e) {
      setError(errorText(e));
    }
  };

  return (
    <div className="profile-form">
      <div className="profile-name-row">
        <input className="text-input profile-name" value={draft.name} onChange={(e) => change({ name: e.target.value })} aria-label="Profile name" placeholder="Name" spellCheck={false} />
        {draft.id === activeId ? (
          <span className="muted small">In use</span>
        ) : (
          <button className="btn-plain" onClick={async () => (await save(), useProfiles.getState().switchTo(draft.id))}>
            Use this profile
          </button>
        )}
      </div>
      {error && (
        <p className="form-error">
          <TriangleAlert size={13} /> {error}
        </p>
      )}

      <Group title="Languages and regions" note="Read from your provider's category names, such as “US|”, “EN -” or “ENGLISH SERIES”. Numbers are channels, films and series.">
        <ToggleRow label="Show every language" on={everyLanguage} onChange={(on) => change({ languages: on ? [] : ["english"] })} />
        {!everyLanguage && (
          <>
            <div className="profile-chips">
              {detected.map((language) => (
                <Chip key={language} on={draft.languages.includes(language)} onClick={() => toggleLanguage(language)} count={languageCounts.get(language)}>
                  {LANGUAGE_NAMES[language]}
                </Chip>
              ))}
            </div>
            <ToggleRow
              label="Include categories that name no language"
              note={unlabeled.length ? `${count(sum(unlabeled))} items, such as ${unlabeled.slice(0, 3).map((c) => c.name).join(", ")}` : "There are none in your sources."}
              on={draft.includeUnlabeled}
              onChange={(includeUnlabeled) => change({ includeUnlabeled })}
            />
          </>
        )}
      </Group>

      <Group title="Favorite genres" note="Each favorite genre gets a row on Home. With “Only show my genres” on, categories named for other genres are left out; mixed ones such as “NETFLIX MOVIES” stay.">
        <div className="profile-chips">
          {GENRES.map((genre) => (
            <Chip key={genre} on={draft.genres.includes(genre)} onClick={() => toggleGenre(genre)} count={genreCounts.get(genre)}>
              {GENRE_NAMES[genre]}
            </Chip>
          ))}
        </div>
        <ToggleRow label="Only show my genres" on={draft.onlyGenres} disabled={!draft.genres.length} onChange={(onlyGenres) => change({ onlyGenres })} />
      </Group>

      <Group title="Show">
        {(["live", "movie", "series"] as CategoryKind[]).map((kind) => (
          <ToggleRow key={kind} label={KIND_NAMES[kind]} note={categories ? `${count(kindCounts[kind])} in your sources` : undefined} on={!draft.hiddenKinds.includes(kind)} onChange={() => toggleKind(kind)} />
        ))}
      </Group>

      <Group
        title="Adult content"
        note={
          parental.hasPin && !parental.unlocked
            ? "The parental PIN keeps adult categories locked whatever the profile says."
            : adult.length
              ? `${adult.length} adult categor${adult.length === 1 ? "y" : "ies"} found in your sources.`
              : "No adult categories were found in your sources."
        }
      >
        <ToggleRow label="Hide adult content" on={draft.hideAdult} onChange={(hideAdult) => change({ hideAdult })} />
      </Group>

      <CategoryChoices draft={draft} categories={categories} change={change} />

      <div className="profile-footer">
        <span className="muted">
          {categories ? `This profile shows ${count(totals.live)} channels, ${count(totals.movie)} films and ${count(totals.series)} series.` : "Counting what's in your sources…"}
        </span>
        {canDelete && (
          <button className="btn-plain danger-text" onClick={remove}>
            <Trash2 size={14} /> Delete profile
          </button>
        )}
      </div>
      {draft.id === EVERYTHING_ID && everyLanguage && !draft.hideAdult && !draft.genres.length && !draft.hiddenKinds.length && (
        <p className="settings-note">Everything shows all your sources hold. Make another profile to see less.</p>
      )}
    </div>
  );
}

type Choice = "rules" | "show" | "hide";

/** Single categories shown or left out by hand, whatever the rules say. */
function CategoryChoices({ draft, categories, change }: { draft: LibraryProfile; categories: ProfileCategory[] | null; change: (patch: Partial<LibraryProfile>) => void }) {
  const [text, setText] = useState("");
  const choiceOf = (id: string): Choice => (draft.includedCategories.includes(id) ? "show" : draft.excludedCategories.includes(id) ? "hide" : "rules");
  const choose = (id: string, choice: Choice) =>
    change({
      includedCategories: choice === "show" ? [...draft.includedCategories.filter((c) => c !== id), id] : draft.includedCategories.filter((c) => c !== id),
      excludedCategories: choice === "hide" ? [...draft.excludedCategories.filter((c) => c !== id), id] : draft.excludedCategories.filter((c) => c !== id),
    });
  const wanted = text.trim().toLowerCase();
  const list = (categories ?? []).filter((c) => (wanted ? c.name.toLowerCase().includes(wanted) : choiceOf(c.id) !== "rules"));
  const shown = list.slice(0, 80);

  return (
    <Group title="Categories" note="Choose single categories to always show or always leave out. A category locked with the parental PIN stays locked.">
      <label className="profile-search">
        <Search size={14} strokeWidth={2} />
        <input value={text} placeholder="Find a category" onChange={(e) => setText(e.target.value)} spellCheck={false} autoComplete="off" />
      </label>
      {shown.map((c) => {
        const choice = choiceOf(c.id);
        const showing = includes(draft, c);
        return (
          <div key={c.id} className="profile-category">
            <div className="profile-category-text">
              <div className="ellipsis">{c.name}</div>
              <div className="muted small">
                {KIND_NAMES[c.kind]} · {count(c.itemCount)} · {showing ? "Shown" : "Left out"}
              </div>
            </div>
            <div className="segmented">
              {(
                [
                  ["rules", "By the rules"],
                  ["show", "Always show"],
                  ["hide", "Leave out"],
                ] as [Choice, string][]
              ).map(([value, label]) => (
                <button key={value} className={choice === value ? "selected" : ""} onClick={() => choose(c.id, value)}>
                  {label}
                </button>
              ))}
            </div>
          </div>
        );
      })}
      {!shown.length && <p className="muted small profile-empty">{wanted ? "No category has that in its name." : "No category is chosen by hand. Find one to choose it."}</p>}
      {list.length > shown.length && <p className="muted small profile-empty">{`And ${count(list.length - shown.length)} more. Type more of the name to narrow them down.`}</p>}
    </Group>
  );
}

function tally<T extends string>(categories: ProfileCategory[], of: (c: ProfileCategory) => T[]) {
  const counts = new Map<T, number>();
  for (const c of categories) for (const value of of(c)) counts.set(value, (counts.get(value) ?? 0) + c.itemCount);
  return counts;
}

const sum = (categories: ProfileCategory[]) => categories.reduce((n, c) => n + c.itemCount, 0);

function Group({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
  return (
    <section className="settings-group profile-group">
      <h2 className="section-title">{title}</h2>
      <div className="settings-card">{children}</div>
      {note && <p className="settings-note">{note}</p>}
    </section>
  );
}

function ToggleRow({ label, note, on, disabled, onChange }: { label: string; note?: string; on: boolean; disabled?: boolean; onChange: (on: boolean) => void }) {
  return (
    <div className={`settings-row ${disabled ? "profile-disabled" : ""}`}>
      <span className="settings-label">
        {label}
        {note && <span className="profile-row-note">{note}</span>}
      </span>
      <button role="switch" aria-checked={on} aria-label={label} disabled={disabled} className={`toggle ${on ? "on" : ""}`} onClick={() => onChange(!on)} />
    </div>
  );
}

function Chip({ on, count: n, onClick, children }: { on: boolean; count?: number; onClick: () => void; children: ReactNode }) {
  return (
    <button className={`genre-chip profile-chip ${on ? "selected" : ""}`} aria-pressed={on} onClick={onClick}>
      {children}
      {n ? <span className="profile-chip-count">{count(n)}</span> : null}
    </button>
  );
}
