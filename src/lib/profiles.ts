// Library profiles in the interface: names for what category names can say,
// a twin of xorvio-core's profiles::includes (so the editor can count what a
// profile shows as it's changed, without asking the backend each time), and
// the profile in use. Screens are remounted when `revision` changes, so
// everything reloads under the new profile at once.

import { create } from "zustand";
import { api, errorText } from "./api";
import { useApp } from "./store";
import type { CategoryKind, ContentGenre, ContentLanguage, LibraryProfile, ProfileCategory } from "./types";

export const EVERYTHING_ID = "everything";

export const LANGUAGE_NAMES: Record<ContentLanguage, string> = {
  english: "English",
  french: "French",
  spanish: "Spanish",
  portuguese: "Portuguese",
  german: "German",
  italian: "Italian",
  dutch: "Dutch",
  nordic: "Nordic",
  polish: "Polish",
  turkish: "Turkish",
  arabic: "Arabic",
  southAsian: "South Asian",
  russian: "Russian",
  ukrainian: "Ukrainian",
  greek: "Greek",
  albanian: "Albanian",
  balkan: "Balkan",
  hungarian: "Hungarian",
  romanian: "Romanian",
  czechSlovak: "Czech and Slovak",
  bulgarian: "Bulgarian",
  persian: "Persian",
  hebrew: "Hebrew",
  korean: "Korean",
  japanese: "Japanese",
  chinese: "Chinese",
  filipino: "Filipino",
  vietnamese: "Vietnamese",
  thai: "Thai",
  african: "African",
};

/** English first, then by name. */
export const LANGUAGES = (Object.keys(LANGUAGE_NAMES) as ContentLanguage[]).sort((a, b) =>
  a === "english" ? -1 : b === "english" ? 1 : LANGUAGE_NAMES[a].localeCompare(LANGUAGE_NAMES[b]),
);

export const GENRE_NAMES: Record<ContentGenre, string> = {
  action: "Action",
  adventure: "Adventure",
  animation: "Animation",
  comedy: "Comedy",
  crime: "Crime",
  documentary: "Documentary",
  drama: "Drama",
  family: "Family",
  fantasy: "Fantasy",
  horror: "Horror",
  kids: "Kids",
  music: "Music",
  mystery: "Mystery",
  news: "News",
  reality: "Reality",
  romance: "Romance",
  scienceFiction: "Science Fiction",
  sports: "Sports",
  thriller: "Thriller",
  western: "Western",
  faith: "Faith",
  holiday: "Holiday",
  anime: "Anime",
};

export const GENRES = Object.keys(GENRE_NAMES) as ContentGenre[];

export const KIND_NAMES: Record<CategoryKind, string> = { live: "Live TV", movie: "Films", series: "Series" };

/** A new profile starts as Swift's did: English, adult content hidden. */
export function newProfile(name: string): LibraryProfile {
  return {
    id: "",
    name,
    languages: ["english"],
    includeUnlabeled: true,
    hideAdult: true,
    genres: [],
    onlyGenres: false,
    hiddenKinds: [],
    includedCategories: [],
    excludedCategories: [],
  };
}

/** Whether a profile shows a category. The twin of xorvio-core's profiles::includes. */
export function includes(profile: LibraryProfile, category: Pick<ProfileCategory, "id" | "kind" | "languages" | "genres" | "adult">) {
  if (profile.includedCategories.includes(category.id)) return true;
  if (profile.excludedCategories.includes(category.id) || profile.hiddenKinds.includes(category.kind)) return false;
  if (profile.hideAdult && category.adult) return false;
  if (profile.languages.length) {
    const shown = category.languages.length ? category.languages.some((l) => profile.languages.includes(l)) : profile.includeUnlabeled;
    if (!shown) return false;
  }
  // Categories naming no genre stay: "NETFLIX MOVIES" could hold anything.
  return !(profile.onlyGenres && profile.genres.length && category.genres.length && !category.genres.some((g) => profile.genres.includes(g)));
}

/** Whether a profile leaves anything out at all. */
export function filtersAnything(profile: LibraryProfile | undefined) {
  if (!profile) return false;
  return !!(profile.languages.length || profile.hideAdult || (profile.onlyGenres && profile.genres.length) || profile.hiddenKinds.length || profile.excludedCategories.length);
}

/** Channels, films and series a profile shows. */
export function profileTotals(profile: LibraryProfile, categories: ProfileCategory[]): Record<CategoryKind, number> {
  const totals: Record<CategoryKind, number> = { live: 0, movie: 0, series: 0 };
  for (const c of categories) if (includes(profile, c)) totals[c.kind] += c.itemCount;
  return totals;
}

/** What a profile filters by, without its name: a change here reloads every screen. */
function rules(profile: LibraryProfile | undefined) {
  return profile ? JSON.stringify({ ...profile, name: "" }) : "";
}

interface ProfilesState {
  profiles: LibraryProfile[];
  activeId: string;
  /** Bumped when the profile in use changes or its rules do. */
  revision: number;
  load(): Promise<void>;
  switchTo(id: string): Promise<void>;
  /** Saves a profile and resolves with it as kept; an error says why it wasn't. */
  save(profile: LibraryProfile): Promise<LibraryProfile>;
  remove(id: string): Promise<void>;

  /** The profile editor, open on a profile (null: closed). */
  editing: string | null;
  editProfiles(id?: string): void;
  closeEditor(): void;
  /** The Customize Home sheet. */
  customizingHome: boolean;
  customizeHome(open: boolean): void;
}

export const useProfiles = create<ProfilesState>((set, get) => {
  /** Takes what the backend says now, reloading the screens if the library they show changed. */
  const refresh = async () => {
    const before = get();
    const { profiles, activeId } = await api.profiles();
    const changed = before.activeId !== activeId || rules(active(before)) !== rules(profiles.find((p) => p.id === activeId));
    set((s) => ({ profiles, activeId, revision: changed && before.activeId ? s.revision + 1 : s.revision }));
    // The watchlist marks and Home's rows follow the profile too.
    if (changed && before.activeId) useApp.getState().loadLibrary().catch(() => {});
  };
  return {
    profiles: [],
    activeId: "",
    revision: 0,
    // Should the profiles not load, Home still lays out as the standard Everything would.
    load: () => refresh().catch(() => (get().activeId ? undefined : set({ activeId: EVERYTHING_ID }))),
    switchTo: async (id) => {
      if (id === get().activeId) return;
      try {
        await api.setActiveProfile(id);
      } catch (e) {
        useApp.getState().showToast(errorText(e), "error");
      }
      await refresh();
    },
    save: async (profile) => {
      const saved = await api.saveProfile(profile);
      await refresh();
      return saved;
    },
    remove: async (id) => {
      await api.deleteProfile(id);
      await refresh();
    },

    editing: null,
    editProfiles: (id) => set({ editing: id ?? get().activeId ?? "" }),
    closeEditor: () => set({ editing: null }),
    customizingHome: false,
    customizeHome: (customizingHome) => set({ customizingHome }),
  };
});

/** The profile in use. */
export function active(state: Pick<ProfilesState, "profiles" | "activeId">): LibraryProfile | undefined {
  return state.profiles.find((p) => p.id === state.activeId) ?? state.profiles[0];
}

export function useActiveProfile() {
  return useProfiles((s) => active(s));
}
