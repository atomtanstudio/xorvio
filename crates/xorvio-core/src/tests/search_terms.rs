//! Search finding films and series by the people in them, their genres and
//! their story (`crate::terms`), in the demo library, whose titles come with
//! their cast, director, genre and plot as a source's would.

use super::*;
use crate::terms::{Credit, SearchTerms, words};

fn terms_of(core: &Core, id: &str) -> SearchTerms {
    core.catalog.title_terms(&[id.to_string()]).unwrap().remove(id).unwrap()
}

/// Whether a word of `text` starts with a word of `title`'s, or the other
/// way round: a search for it could find the title by its name.
fn shares_words(title: &str, text: &str) -> bool {
    let (title, text) = (words(title), words(text));
    title.iter().any(|a| text.iter().any(|b| a.starts_with(b.as_str()) || b.starts_with(a.as_str())))
}

/// A demo film with someone in its cast whose name has no word of its title.
fn film_and_actor(core: &Core) -> (Movie, String) {
    core.movies(&BrowseQuery { limit: 500, ..Default::default() })
        .unwrap()
        .into_iter()
        .find_map(|movie| {
            let actor = terms_of(core, &movie.id).people.into_iter().find(|person| person.credit == Credit::Cast)?.name;
            (!shares_words(&movie.title, &actor)).then_some((movie, actor))
        })
        .unwrap()
}

#[tokio::test]
async fn titles_are_found_by_their_people_genres_and_story() {
    let (_folder, core, _) = open_core();
    core.add_demo_source().unwrap();
    let (film, actor) = film_and_actor(&core);

    // Someone in it: the film, why, and them, from a title they're in.
    let found = core.search(&actor, 40).unwrap();
    assert!(found.movies.iter().any(|movie| movie.id == film.id), "{actor}");
    assert_eq!(found.reasons[&film.id], format!("With {actor}"));
    let person = found.people.iter().find(|person| person.name == actor).unwrap();
    assert!(person.titles >= 1);
    assert!(terms_of(&core, &person.from_id).person_matching(&words(&actor)).is_some());
    // Half a name, as it's typed, finds them too.
    let first_name = actor.split(' ').next().unwrap();
    let typed = format!("{first_name} {}", &actor[first_name.len() + 1..][..2]);
    assert!(core.search(&typed, 40).unwrap().people.iter().any(|person| person.name == actor), "{typed}");

    // Titles found by name come first, and need no reason.
    for found in [&found, &core.search(&film.title, 40).unwrap()] {
        let reasoned: Vec<bool> = found.movies.iter().map(|movie| found.reasons.contains_key(&movie.id)).collect();
        assert!(reasoned.windows(2).all(|pair| pair[0] <= pair[1]), "{reasoned:?}");
    }

    // Who made it.
    let director = terms_of(&core, &film.id).people.into_iter().find(|person| person.credit == Credit::Director).unwrap().name;
    let found = core.search(&director, 40).unwrap();
    assert!(found.reasons.values().any(|reason| *reason == format!("Directed by {director}")), "{director}: {:?}", found.reasons);

    // Its genre, as the source writes it.
    let genre = film.genre.clone().unwrap();
    let found = core.search(&genre, 40).unwrap();
    assert!(found.reasons.values().any(|reason| *reason == genre), "{genre}: {:?}", found.reasons);

    // A word of its story, with the words around it.
    let plot = film.plot.clone().unwrap();
    let word = plot
        .split(|c: char| !c.is_alphanumeric())
        .rfind(|word| word.len() >= 6 && !shares_words(&film.title, word))
        .unwrap()
        .to_string();
    let found = core.search(&word, 40).unwrap();
    assert!(found.reasons.values().any(|reason| reason.to_lowercase().contains(&word.to_lowercase())), "{word}: {:?}", found.reasons);

    // Short searches stay with names.
    let found = core.search(&actor[..2], 40).unwrap();
    assert!(found.people.is_empty() && found.reasons.is_empty());

    // What's hidden isn't found this way either, nor are the people in it.
    core.set_hidden(film.category_id.as_deref().unwrap(), true).unwrap();
    let found = core.search(&actor, 40).unwrap();
    assert!(!found.movies.iter().any(|movie| movie.id == film.id));
    assert!(found.people.iter().all(|person| person.from_id != film.id));
}

#[tokio::test]
async fn new_details_a_refresh_and_removing_a_source_keep_the_index_current() {
    let (_folder, core, _) = open_core();
    core.add_demo_source().unwrap();
    let film = core
        .movies(&BrowseQuery { limit: 500, ..Default::default() })
        .unwrap()
        .into_iter()
        .find(|movie| !shares_words(&movie.title, "Odalys Brennan-Quist"))
        .unwrap();
    assert!(core.search("brennan quist", 40).unwrap().movies.is_empty());

    // Details read later (as when a title's page is opened) are found at once.
    let mut details = std::collections::BTreeMap::new();
    let someone = CastMember { name: "Odalys Brennan-Quist".into(), ..Default::default() };
    details.insert(film.id.clone(), crate::details::DetailsRecord { people: vec![someone], ..Default::default() });
    core.catalog.save_details(DEMO_SOURCE_ID, &details, Some(now())).unwrap();
    let found = core.search("brennan quist", 40).unwrap();
    assert_eq!(found.movies.iter().map(|movie| movie.id.as_str()).collect::<Vec<_>>(), [film.id.as_str()]);
    assert_eq!(found.people.iter().map(|person| person.name.as_str()).collect::<Vec<_>>(), ["Odalys Brennan-Quist"]);

    // A refresh writes every row again, keeping details read before that it
    // doesn't bring itself.
    core.refresh_source(DEMO_SOURCE_ID, &|_| {}).await.unwrap();
    assert_eq!(core.search("brennan quist", 40).unwrap().movies.len(), 1);

    // Removing the source takes its rows with it.
    core.remove_source(DEMO_SOURCE_ID).unwrap();
    assert!(core.search("brennan quist", 40).unwrap().movies.is_empty());
    assert!(core.catalog.title_terms(std::slice::from_ref(&film.id)).unwrap().is_empty());
}
