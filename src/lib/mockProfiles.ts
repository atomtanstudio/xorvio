// Library profiles and Home's layouts for the browser preview (mock.ts): a
// twin of xorvio-core's classify.rs, as far as category names go, and
// profiles kept in memory. The demo library's categories name genres but no
// languages, so two genre profiles come ready to switch to.

import type { Backend } from "./api";
import { EVERYTHING_ID, includes, newProfile } from "./profiles";
import type { Category, ContentGenre, ContentLanguage, LibraryProfile, ProfileCategory } from "./types";

const CODES: Record<string, ContentLanguage> = {
  US: "english", USA: "english", UK: "english", GB: "english", EN: "english", ENG: "english", CA: "english", AU: "english", AUS: "english",
  NZ: "english", IE: "english", IRL: "english", ZA: "english", SCO: "english", FR: "french", FRA: "french", QC: "french", BE: "french",
  CH: "german", ES: "spanish", ESP: "spanish", MX: "spanish", LAT: "spanish", CO: "spanish", CL: "spanish", PE: "spanish", VE: "spanish",
  PT: "portuguese", BR: "portuguese", DE: "german", GER: "german", AT: "german", IT: "italian", ITA: "italian", NL: "dutch", SE: "nordic",
  SW: "nordic", NO: "nordic", DK: "nordic", FI: "nordic", IS: "nordic", PL: "polish", TR: "turkish", AR: "arabic", ARB: "arabic",
  IN: "southAsian", PK: "southAsian", BD: "southAsian", RU: "russian", UA: "ukrainian", GR: "greek", AL: "albanian", RS: "balkan",
  HR: "balkan", BA: "balkan", SI: "balkan", MK: "balkan", EXYU: "balkan", HU: "hungarian", RO: "romanian", CZ: "czechSlovak",
  SK: "czechSlovak", BG: "bulgarian", IR: "persian", IL: "hebrew", KR: "korean", JP: "japanese", CN: "chinese", HK: "chinese",
  TW: "chinese", PH: "filipino", VN: "vietnamese", TH: "thai", NG: "african", KE: "african", GH: "african",
};
const TRAILING: Record<string, ContentLanguage> = { EN: "english", FR: "french", ES: "spanish", DE: "german", IT: "italian", PT: "portuguese", AR: "arabic", TR: "turkish" };
const WORDS: Record<string, ContentLanguage> = {
  ENGLISH: "english", FRENCH: "french", FRANCAIS: "french", QUEBEC: "french", SPANISH: "spanish", ESPANOL: "spanish", LATINO: "spanish",
  PORTUGUESE: "portuguese", BRAZIL: "portuguese", GERMAN: "german", DEUTSCH: "german", ITALIAN: "italian", ITALIANO: "italian",
  DUTCH: "dutch", SWEDISH: "nordic", NORWEGIAN: "nordic", DANISH: "nordic", FINNISH: "nordic", NORDIC: "nordic", POLISH: "polish",
  POLSKA: "polish", TURKISH: "turkish", TURKSIH: "turkish", TURK: "turkish", ARABIC: "arabic", HINDI: "southAsian", PUNJABI: "southAsian",
  TAMIL: "southAsian", TELUGU: "southAsian", URDU: "southAsian", BANGLA: "southAsian", RUSSIAN: "russian", UKRAINIAN: "ukrainian",
  GREEK: "greek", ALBANIAN: "albanian", SERBIAN: "balkan", CROATIAN: "balkan", BOSNIAN: "balkan", HUNGARIAN: "hungarian",
  MAGYAR: "hungarian", ROMANIAN: "romanian", CZECH: "czechSlovak", SLOVAK: "czechSlovak", BULGARIAN: "bulgarian", PERSIAN: "persian",
  FARSI: "persian", HEBREW: "hebrew", KOREAN: "korean", KDRAMA: "korean", JAPANESE: "japanese", CHINESE: "chinese", FILIPINO: "filipino",
  PINOY: "filipino", VIETNAMESE: "vietnamese", THAI: "thai", AFRICAN: "african", NOLLYWOOD: "african",
};
const GENRE_KEYWORDS: Record<ContentGenre, string[]> = {
  action: ["ACTION"],
  adventure: ["ADVENTURE"],
  animation: ["ANIMATION", "ANIMATED", "CARTOON", "CARTOONS", "DREAMWORKS", "PIXAR"],
  comedy: ["COMEDY", "COMEDIE", "STAND-UP", "STANDUP"],
  crime: ["CRIME"],
  documentary: ["DOCUMENTARY", "DOCUMENTARIES", "DOCU", "DOCUMENTAIRE", "DOCS"],
  drama: ["DRAMA"],
  family: ["FAMILY"],
  fantasy: ["FANTASY"],
  horror: ["HORROR"],
  kids: ["KIDS", "KID", "CHILDREN", "JEUNESSE", "NICKELODEON"],
  music: ["MUSIC", "CONCERT", "CONCERTS", "MUSICAL", "MUSIQUE"],
  mystery: ["MYSTERY"],
  news: ["NEWS"],
  reality: ["REALITY"],
  romance: ["ROMANCE", "ROMANTIC"],
  scienceFiction: ["SCIENCE FICTION", "SCI-FI", "SCIFI"],
  sports: ["SPORT", "SPORTS", "PPV", "NFL", "NBA", "NHL", "MLB", "MLS", "UFC", "WWE", "BOXING", "SOCCER", "FOOTBALL", "F1", "GOLF", "TENNIS", "RUGBY", "CRICKET", "DAZN", "ESPN"],
  thriller: ["THRILLER", "THRILLERS", "SUSPENSE"],
  western: ["WESTERN", "WESTERNS"],
  faith: ["CHRISTIAN", "BIBLICAL", "RELIGIOUS", "FAITH"],
  holiday: ["CHRISTMAS", "HOLIDAY", "NOEL"],
  anime: ["ANIME", "MANGA", "CRUNCHYROLL"],
};
const ADULT_BRANDS = ["PLAYBOY", "HUSTLER", "BRAZZERS"];

function normalized(name: string) {
  return name
    .replace(/[\p{Lm}\p{No}]/gu, "")
    .replace(/[^\p{L}\p{Nd} |\-:()[\]/+&']/gu, "")
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toUpperCase()
    .split(/\s+/)
    .filter(Boolean)
    .join(" ");
}

function leadingCode(text: string) {
  const trimmed = text.replace(/^[ |([-]+/, "");
  const code = /^\p{L}*/u.exec(trimmed)![0];
  if ([...code].length < 2 || [...code].length > 4) return null;
  const next = trimmed.slice(code.length).replace(/^ +/, "")[0];
  return next && "|-:)]".includes(next) ? code : null;
}

export function genresOf(text: string, parts: Set<string>): ContentGenre[] {
  return (Object.keys(GENRE_KEYWORDS) as ContentGenre[]).filter((genre) =>
    GENRE_KEYWORDS[genre].some((keyword) => (/[ -]/.test(keyword) ? text.includes(keyword) : parts.has(keyword))),
  );
}

/** What a category's name says: the twin of classify::traits. */
export function traits(name: string, looksAdult: (name: string) => boolean): Pick<ProfileCategory, "languages" | "genres" | "adult"> {
  const text = normalized(name);
  const tokens = new Set(text.split(/[^\p{L}\p{N}+-]+/u).filter(Boolean));
  const parts = new Set([...tokens, ...text.split(/[^\p{L}\p{N}]+/u).filter(Boolean)]);
  let languages = new Set<ContentLanguage>();
  const lead = leadingCode(text);
  if (lead && CODES[lead]) languages.add(CODES[lead]);
  const last = text.split(" ").pop() ?? "";
  if (last.length === 2 && TRAILING[last]) languages = new Set([TRAILING[last]]);
  for (const [word, language] of Object.entries(WORDS)) if (tokens.has(word)) languages.add(language);
  if ((text.includes("SUB EN") || text.includes("SUB ENG")) && languages.size > 1) languages.delete("english");
  return { languages: [...languages], genres: genresOf(text, parts), adult: looksAdult(name) || ADULT_BRANDS.some((brand) => text.includes(brand)) };
}

/** The keywords a title's genre field is matched by, for Home's genre rows. */
export function genreMatches(genre: ContentGenre, field: string | null | undefined) {
  if (!field) return false;
  const text = normalized(field.replace(/,/g, " "));
  return genresOf(text, new Set(text.split(/[^\p{L}\p{N}]+/u).filter(Boolean))).includes(genre);
}

const everything: LibraryProfile = { ...newProfile("Everything"), id: EVERYTHING_ID, languages: [], hideAdult: false };
let profiles: LibraryProfile[] = [
  everything,
  { ...newProfile("Family"), id: "demo-family", languages: [], genres: ["family", "kids", "animation"], onlyGenres: true },
  { ...newProfile("Thrills"), id: "demo-thrills", languages: [], genres: ["action", "thriller", "crime", "adventure"], onlyGenres: true, hiddenKinds: ["live"] },
];
let activeId = EVERYTHING_ID;
const layouts = new Map<string, unknown>();

export interface ProfileDeps {
  categories: Category[];
  looksAdult(name: string): boolean;
  /** How many channels, films or series each category holds. */
  counts(): Map<string, number>;
  /** Categories behind the PIN right now. */
  locked(): Set<string>;
  named<T extends { id: string; name: string }>(item: T): T;
}

/** Categories the profile in use leaves out. */
export function profileExcluded(deps: Pick<ProfileDeps, "categories" | "looksAdult">) {
  const profile = profiles.find((p) => p.id === activeId) ?? profiles[0];
  return new Set(deps.categories.filter((c) => !includes(profile, { id: c.id, kind: c.kind, ...traits(c.name, deps.looksAdult) })).map((c) => c.id));
}

/** Categories named for a genre ("Action", "Thrillers & Crime"). */
export function genreCategories(deps: Pick<ProfileDeps, "categories" | "looksAdult">, genre: ContentGenre) {
  return new Set(deps.categories.filter((c) => traits(c.name, deps.looksAdult).genres.includes(genre)).map((c) => c.id));
}

export function profileBackend(deps: ProfileDeps): Pick<Backend, "profiles" | "setActiveProfile" | "saveProfile" | "deleteProfile" | "profileCategories" | "homeLayout" | "saveHomeLayout"> {
  return {
    profiles: async () => ({ profiles: profiles.map((p) => ({ ...p })), activeId }),
    setActiveProfile: async (id) => {
      if (!profiles.some((p) => p.id === id)) throw new Error("This profile no longer exists.");
      activeId = id;
    },
    saveProfile: async (profile) => {
      const name = profile.name.trim();
      if (!name) throw new Error("A profile needs a name.");
      const twin = profiles.find((p) => p.id !== profile.id && p.name.toLowerCase() === name.toLowerCase());
      if (twin) throw new Error(`There's already a profile called “${twin.name}”.`);
      const saved = { ...profile, name, id: profile.id || crypto.randomUUID() };
      const at = profiles.findIndex((p) => p.id === saved.id);
      profiles = at >= 0 ? profiles.map((p, i) => (i === at ? saved : p)) : [...profiles, saved];
      return { ...saved };
    },
    deleteProfile: async (id) => {
      if (!profiles.some((p) => p.id === id)) return;
      if (profiles.length === 1) throw new Error("Xorvio needs at least one profile. Make another one first.");
      profiles = profiles.filter((p) => p.id !== id);
      for (const key of [...layouts.keys()]) if (key.startsWith(`${id}/`)) layouts.delete(key);
      if (activeId === id) activeId = profiles[0].id;
    },
    profileCategories: async () => {
      const counts = deps.counts();
      const locked = deps.locked();
      return deps.categories
        .filter((c) => !locked.has(c.id) && (counts.get(c.id) ?? 0) > 0)
        .map((c) => ({ id: c.id, sourceId: c.sourceId, kind: c.kind, name: deps.named(c).name, itemCount: counts.get(c.id) ?? 0, ...traits(c.name, deps.looksAdult) }));
    },
    homeLayout: async (mode) => layouts.get(`${activeId}/${mode}`) ?? null,
    saveHomeLayout: async (mode, layout) => {
      if (layout === null) layouts.delete(`${activeId}/${mode}`);
      else layouts.set(`${activeId}/${mode}`, layout);
    },
  };
}
