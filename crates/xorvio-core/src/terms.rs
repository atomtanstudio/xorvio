//! What search finds a film or series by besides its title: the people in
//! it and behind it, its genres and its story. They come from the title's
//! source (its list and its details) and, for titles whose source names no
//! one in them, from TMDB, looked up in the background
//! (`Core::fill_search_terms`). The catalog keeps them in an index of their
//! own beside the titles' (`title_terms`), so a title's name still finds it
//! first, as it always did.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;
use xorvio_model::SearchPerson;

use crate::details::DetailsRecord;
use crate::store::catalog::{SearchKind, TermColumn};
use crate::{Core, Result};

/// The most people a title is found by: its crew, then its best billed.
const MOST_PEOPLE: usize = 30;
/// Searches this short find titles by their names only.
const SHORTEST: usize = 3;
/// Stories are searched from this many letters on: shorter words are in
/// every story.
const SHORTEST_FOR_STORIES: usize = 4;

/// How someone is credited on a title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Credit {
    #[default]
    Cast,
    Director,
    Creator,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Person {
    pub name: String,
    pub credit: Credit,
    pub photo_url: Option<String>,
    pub tmdb_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct SearchTerms {
    pub people: Vec<Person>,
    pub genres: Vec<String>,
    pub story: Option<String>,
}

impl SearchTerms {
    /// What a title's details say it's found by.
    pub fn from_details(details: &DetailsRecord) -> SearchTerms {
        let mut terms = SearchTerms::default();
        terms.add_details(details);
        terms
    }

    /// A catalog title's terms: its details', then what its list said
    /// (`plot`, `genre`), then TMDB's for anything those leave out.
    pub fn of_title(plot: Option<&str>, genre: Option<&str>, details: Option<&DetailsRecord>, tmdb: Option<&SearchTerms>) -> SearchTerms {
        let mut terms = SearchTerms::default();
        if let Some(details) = details {
            terms.add_details(details);
        }
        for genre in genre.into_iter().flat_map(split_genres) {
            terms.add_genre(genre);
        }
        if terms.story.is_none() {
            terms.story = plot.map(str::trim).filter(|plot| !plot.is_empty()).map(str::to_string);
        }
        if let Some(tmdb) = tmdb {
            for person in &tmdb.people {
                terms.add_person(&person.name, person.credit, person.photo_url.as_deref(), person.tmdb_id.as_deref());
            }
            for genre in &tmdb.genres {
                terms.add_genre(genre);
            }
            if terms.story.is_none() {
                terms.story = tmdb.story.clone();
            }
        }
        terms
    }

    fn add_details(&mut self, details: &DetailsRecord) {
        for name in details.director.iter().flat_map(|names| names.split(',')) {
            self.add_person(name, Credit::Director, None, None);
        }
        for name in &details.creators {
            self.add_person(name, Credit::Creator, None, None);
        }
        for member in &details.people {
            self.add_person(&member.name, Credit::Cast, member.photo_url.as_deref(), member.tmdb_id.as_deref());
        }
        for name in &details.cast {
            self.add_person(name, Credit::Cast, None, None);
        }
        for genre in &details.genres {
            self.add_genre(genre);
        }
        if self.story.is_none() {
            self.story = details.overview.as_deref().map(str::trim).filter(|story| !story.is_empty()).map(str::to_string);
        }
    }

    /// Someone once, as first credited, with a photo and TMDB id from
    /// wherever one is known.
    fn add_person(&mut self, name: &str, credit: Credit, photo_url: Option<&str>, tmdb_id: Option<&str>) {
        let name = name.trim();
        let key = words(name).join(" ");
        if key.is_empty() {
            return;
        }
        if let Some(known) = self.people.iter_mut().find(|person| words(&person.name).join(" ") == key) {
            if known.photo_url.is_none() {
                known.photo_url = photo_url.map(str::to_string);
            }
            if known.tmdb_id.is_none() {
                known.tmdb_id = tmdb_id.map(str::to_string);
            }
            return;
        }
        if self.people.len() < MOST_PEOPLE {
            self.people.push(Person { name: name.to_string(), credit, photo_url: photo_url.map(str::to_string), tmdb_id: tmdb_id.map(str::to_string) });
        }
    }

    fn add_genre(&mut self, genre: &str) {
        let genre = genre.trim();
        if !genre.is_empty() && !self.genres.iter().any(|known| words(known) == words(genre)) {
            self.genres.push(genre.to_string());
        }
    }

    pub fn is_empty(&self) -> bool {
        self.people.is_empty() && self.genres.is_empty() && self.story.is_none()
    }

    /// The index's columns: people, genres and story.
    pub fn columns(&self) -> [String; 3] {
        [
            self.people.iter().map(|person| person.name.as_str()).collect::<Vec<_>>().join(", "),
            self.genres.join(", "),
            self.story.clone().unwrap_or_default(),
        ]
    }

    /// The one person all of `wanted` find, each the start of a word of
    /// their name: the index finds "Tom Hanks" in a film with Tom Cruise and
    /// Colin Hanks too.
    pub fn person_matching(&self, wanted: &[String]) -> Option<&Person> {
        self.people.iter().find(|person| starts_words(wanted, &person.name))
    }

    /// The one genre all of `wanted` find.
    pub fn genre_matching(&self, wanted: &[String]) -> Option<&str> {
        self.genres.iter().find(|genre| starts_words(wanted, genre)).map(String::as_str)
    }
}

/// Films or series found by the people in them, their genres or their
/// story rather than their names.
#[derive(Debug, Default)]
pub(crate) struct TermMatches {
    /// Their ids, best first (by people, then genres, then stories), each
    /// with why it was found.
    pub found: Vec<(String, String)>,
    /// Who was found, with the id of the title they were found in.
    pub people: Vec<(Person, String)>,
}

impl Core {
    /// Films or series (`kind`) of `sources` found by what's in
    /// `title_terms`, leaving out what the title search leaves out.
    pub(crate) fn term_matches(
        &self,
        text: &str,
        kind: SearchKind,
        sources: &[String],
        limit: usize,
        excluded: &[String],
        title_tags: &[String],
    ) -> Result<TermMatches> {
        let wanted = words(text);
        let mut matches = TermMatches::default();
        if !finds_people(&wanted) {
            return Ok(matches);
        }
        let by = |column, limit| self.catalog.search_terms(column, text, kind, sources, limit, excluded, title_tags);
        // The index finds "Tom Hanks" in a film with Tom Cruise and Colin
        // Hanks too, so more are asked for than are kept.
        let people = by(TermColumn::People, limit * 3)?;
        let genres = by(TermColumn::Genres, limit)?;
        let stories = if finds_stories(&wanted) { by(TermColumn::Story, limit)? } else { Vec::new() };
        let ids: Vec<String> = people.iter().chain(&genres).chain(&stories).map(|(id, _)| id.clone()).collect();
        let terms = self.catalog.title_terms(&ids)?;
        let mut seen = HashSet::new();
        for (id, _) in &people {
            if let Some(person) = terms.get(id).and_then(|terms| terms.person_matching(&wanted)) {
                matches.people.push((person.clone(), id.clone()));
                if seen.insert(id.clone()) {
                    matches.found.push((id.clone(), credited(person)));
                }
            }
        }
        for (id, _) in &genres {
            if let Some(genre) = terms.get(id).and_then(|terms| terms.genre_matching(&wanted))
                && seen.insert(id.clone())
            {
                matches.found.push((id.clone(), genre.to_string()));
            }
        }
        for (id, around) in &stories {
            if seen.insert(id.clone()) {
                matches.found.push((id.clone(), around.clone().filter(|around| !around.is_empty()).unwrap_or_else(|| "In its story".into())));
            }
        }
        Ok(matches)
    }
}

/// Why a title was found by someone in it.
fn credited(person: &Person) -> String {
    match person.credit {
        Credit::Cast => format!("With {}", person.name),
        Credit::Director => format!("Directed by {}", person.name),
        Credit::Creator => format!("Created by {}", person.name),
    }
}

/// The people found, each once, those in the most titles first (then as
/// they were found), at most `most`.
pub(crate) fn people_found(found: &[(Person, String)], most: usize) -> Vec<SearchPerson> {
    let mut people: Vec<(String, SearchPerson)> = Vec::new();
    for (person, item_id) in found {
        let key = words(&person.name).join(" ");
        match people.iter_mut().find(|(known, _)| *known == key) {
            Some((_, known)) => {
                known.titles += 1;
                if known.photo_url.is_none() {
                    known.photo_url = person.photo_url.clone();
                }
                if known.tmdb_id.is_none() {
                    known.tmdb_id = person.tmdb_id.clone();
                }
            }
            None => people.push((
                key,
                SearchPerson {
                    name: person.name.clone(),
                    photo_url: person.photo_url.clone(),
                    tmdb_id: person.tmdb_id.clone(),
                    from_id: item_id.clone(),
                    titles: 1,
                },
            )),
        }
    }
    let mut people: Vec<SearchPerson> = people.into_iter().map(|(_, person)| person).collect();
    people.sort_by_key(|person| std::cmp::Reverse(person.titles));
    people.truncate(most);
    people
}

/// Words as search compares them: in lower case, without accents.
pub(crate) fn words(text: &str) -> Vec<String> {
    text.nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether a search is long enough to look past titles' names.
pub(crate) fn finds_people(wanted: &[String]) -> bool {
    wanted.iter().map(|word| word.chars().count()).sum::<usize>() >= SHORTEST
}

/// Whether a search is long enough to look in stories.
pub(crate) fn finds_stories(wanted: &[String]) -> bool {
    wanted.iter().map(|word| word.chars().count()).sum::<usize>() >= SHORTEST_FOR_STORIES
}

/// Whether each of `wanted` starts a word of `text`.
fn starts_words(wanted: &[String], text: &str) -> bool {
    let have = words(text);
    !wanted.is_empty() && wanted.iter().all(|word| have.iter().any(|had| had.starts_with(word.as_str())))
}

/// Genres as providers write them: "Action, Adventure", "Action / Comedy",
/// "Drama | Crime" ("Sci-Fi & Fantasy" is one, as TMDB names it).
fn split_genres(text: &str) -> impl Iterator<Item = &str> {
    text.split([',', '/', '|', ';']).map(str::trim).filter(|genre| !genre.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use xorvio_model::CastMember;

    fn wanted(text: &str) -> Vec<String> {
        words(text)
    }

    #[test]
    fn details_list_crew_first_and_everyone_once() {
        let details = DetailsRecord {
            director: Some("Kathryn Bigelow, Mark Boal".into()),
            people: vec![CastMember { name: "Jeremy Renner".into(), photo_url: Some("p.jpg".into()), tmdb_id: Some("17604".into()), ..Default::default() }],
            cast: vec!["Jeremy Renner".into(), "Anthony Mackie".into(), " ".into()],
            genres: vec!["War".into(), "Thriller".into()],
            overview: Some("An Army bomb squad in Baghdad.".into()),
            ..Default::default()
        };
        let terms = SearchTerms::from_details(&details);
        let names: Vec<(&str, Credit)> = terms.people.iter().map(|person| (person.name.as_str(), person.credit)).collect();
        assert_eq!(
            names,
            [("Kathryn Bigelow", Credit::Director), ("Mark Boal", Credit::Director), ("Jeremy Renner", Credit::Cast), ("Anthony Mackie", Credit::Cast)]
        );
        assert_eq!(terms.people[2].tmdb_id.as_deref(), Some("17604"));
        assert_eq!(terms.columns(), ["Kathryn Bigelow, Mark Boal, Jeremy Renner, Anthony Mackie".to_string(), "War, Thriller".into(), "An Army bomb squad in Baghdad.".into()]);
    }

    #[test]
    fn tmdb_fills_only_what_the_source_left_out() {
        let tmdb = SearchTerms {
            people: vec![Person { name: "Amélie Poulain".into(), photo_url: Some("a.jpg".into()), ..Default::default() }],
            genres: vec!["Comedy".into(), "Romance".into()],
            story: Some("TMDB's story".into()),
        };
        let details = DetailsRecord { cast: vec!["Amelie Poulain".into()], ..Default::default() };
        let terms = SearchTerms::of_title(Some("The provider's story"), Some("Comedy / Drama"), Some(&details), Some(&tmdb));
        assert_eq!(terms.people.len(), 1, "the same name with and without accents is one person");
        assert_eq!(terms.people[0].photo_url.as_deref(), Some("a.jpg"));
        assert_eq!(terms.genres, ["Comedy", "Drama", "Romance"]);
        assert_eq!(terms.story.as_deref(), Some("The provider's story"));
        assert!(SearchTerms::of_title(None, None, None, None).is_empty());
    }

    #[test]
    fn one_person_has_to_match_every_word() {
        let terms = SearchTerms {
            people: ["Tom Cruise", "Colin Hanks", "Tom Hanks"].map(|name| Person { name: name.into(), ..Default::default() }).to_vec(),
            genres: vec!["Science Fiction".into()],
            story: None,
        };
        assert_eq!(terms.person_matching(&wanted("tom hanks")).map(|person| person.name.as_str()), Some("Tom Hanks"));
        assert_eq!(terms.person_matching(&wanted("hanks tom")).map(|person| person.name.as_str()), Some("Tom Hanks"), "in any order");
        assert_eq!(terms.person_matching(&wanted("col han")).map(|person| person.name.as_str()), Some("Colin Hanks"), "as typed");
        assert!(terms.person_matching(&wanted("cruise hanks")).is_none());
        assert_eq!(terms.genre_matching(&wanted("sci")), Some("Science Fiction"));
        assert!(terms.genre_matching(&wanted("fiction drama")).is_none());
    }

    #[test]
    fn short_searches_stay_with_titles() {
        assert!(!finds_people(&wanted("ab")));
        assert!(finds_people(&wanted("abc")));
        assert!(!finds_stories(&wanted("abc")));
        assert!(finds_stories(&wanted("heist")));
        assert_eq!(words("Zoë  O'Neil-Smith"), ["zoe", "o", "neil", "smith"]);
    }
}
