// Someone from a cast. With a TMDB key it's their page: photo, biography and
// what of theirs the sources carry. Without one (or when TMDB doesn't know
// them) it's their filmography from Wikidata, and when neither knows them,
// a search for their name, as before.

import { useEffect, useState } from "react";
import { placeholderGradient } from "../components/Artwork";
import { api } from "../lib/api";
import { initials } from "../lib/format";
import { imageSrc } from "../lib/images";
import type { PersonDetails } from "../lib/types";
import { SearchScreen, TitleShelf } from "./Search";

export function PersonScreen({ name, tmdbId, fromId }: { name: string; tmdbId?: string | null; fromId?: string | null }) {
  // undefined while asking; null when neither TMDB nor Wikidata can say.
  const [person, setPerson] = useState<PersonDetails | null | undefined>(undefined);

  useEffect(() => {
    let cancelled = false;
    setPerson(undefined);
    api
      .person(name, tmdbId ?? null, fromId ?? null)
      .then((p) => !cancelled && setPerson(p))
      .catch(() => !cancelled && setPerson(null));
    return () => {
      cancelled = true;
    };
  }, [name, tmdbId, fromId]);

  if (person === undefined) return <div className="content" />;
  if (person === null) return <SearchScreen query={name} />;
  const nothing = !person.movies.length && !person.series.length;

  return (
    <div className="content person-page">
      <header className="person-header">
        <div className="person-photo" style={{ background: placeholderGradient(person.name) }}>
          {person.photoUrl ? <img src={imageSrc(person.photoUrl)} alt="" /> : <span>{initials(person.name)}</span>}
        </div>
        <div className="person-intro">
          <h1 className="page-title">{person.name}</h1>
          <p className="person-facts">{facts(person).join(" · ")}</p>
          {person.biography && <Biography text={person.biography} />}
        </div>
      </header>
      {person.movies.length > 0 && <TitleShelf title="Films" items={person.movies} kind="movie" />}
      {person.series.length > 0 && <TitleShelf title="Series" items={person.series} kind="series" />}
      {nothing && <p className="person-note muted">Nothing with {person.name} is in your sources yet.</p>}
      {person.attribution && <p className="person-note person-credit">{person.attribution === "Wikidata" ? "Credits from Wikidata" : `Details from ${person.attribution}`}</p>}
      {person.photoCredit && (
        <p className="person-note person-credit">
          Photo: {person.photoPage ? (
            <button className="btn-link" onClick={() => api.openExternal(person.photoPage!).catch(() => {})}>
              {person.photoCredit}
            </button>
          ) : (
            person.photoCredit
          )}
          , via Wikimedia Commons
        </p>
      )}
    </div>
  );
}

function Biography({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const long = text.length > 420;
  return (
    <div className="person-biography">
      <p className={open || !long ? "" : "clamped"}>{text}</p>
      {long && (
        <button className="link-button" onClick={() => setOpen((o) => !o)}>
          {open ? "Less" : "More"}
        </button>
      )}
    </div>
  );
}

/** "Acting · Born 2 September 1964 in Beirut, Lebanon", or with the years of a life; Wikidata's line about them leads when there's no department. */
function facts(person: PersonDetails) {
  const born = day(person.birthday);
  const died = day(person.deathday);
  const life = died
    ? [person.birthday?.slice(0, 4), person.deathday?.slice(0, 4)].filter(Boolean).join("–")
    : born
      ? `Born ${born}${person.placeOfBirth ? ` in ${person.placeOfBirth}` : ""}`
      : null;
  return [person.department ?? person.summary, life].filter((part): part is string => !!part);
}

const longDate = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "long", year: "numeric" });

/** "1964-09-02" as the system writes a date. */
function day(date: string | null) {
  const match = date?.match(/^(\d{4})-(\d{2})-(\d{2})$/);
  return match ? longDate.format(new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]))) : null;
}
