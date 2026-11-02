//! Library profiles: named views of the library ("English only", "Action")
//! that leave categories out by what their names say (classify.rs), and
//! Home's layout for each. The active profile joins hidden and locked items
//! in [`Core::excluded`], so every list that leaves those out leaves out what
//! the profile does too. A profile never reaches past the parental PIN:
//! locked categories stay locked whatever it says.
//!
//! Profiles are kept in `library.sqlite` as JSON, so fields added later read
//! older rows with their defaults. Until someone makes one, there's a single
//! implied profile, Everything, which leaves nothing out.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use xorvio_model::{CategoryKind, ContentGenre, LibraryProfile, Movie, ProfileCategory, Profiles, Series};

use crate::classify::{self, CategoryTraits};
use crate::store::lock;
use crate::{Core, Result};

/// Everything's id, fixed so its Home layout survives relaunches.
pub(crate) const EVERYTHING_ID: &str = "everything";

/// Where the active profile's id is kept in the library's preferences.
const ACTIVE_KEY: &str = "active_profile";

/// Home is laid out on its own for when the sources lead it and for when
/// add-ons do, so switching between them keeps each arrangement.
const HOME_MODES: &[&str] = &["sources", "addons"];

/// Multilingual countries whose titles' tags say nothing about language;
/// their categories decide.
const MULTILINGUAL: &[&str] = &["CA", "BE", "CH"];

/// The profile there is before anyone makes one: nothing left out.
pub(crate) fn everything() -> LibraryProfile {
    LibraryProfile { id: EVERYTHING_ID.into(), name: "Everything".into(), ..Default::default() }
}

/// The active profile and what category names say, kept between lists.
#[derive(Default)]
pub(crate) struct ProfileState {
    active: Mutex<Option<Arc<LibraryProfile>>>,
    /// Read once per name: classifying thousands of names for every list
    /// would be slow, and a name always says the same.
    traits: Mutex<HashMap<String, Arc<CategoryTraits>>>,
}

/// What the active profile leaves out of lists.
#[derive(Debug, Default)]
pub(crate) struct ProfileExclusions {
    pub categories: HashSet<String>,
    /// Country tags ("JP") whose films and series ("Title (JP)") are left out.
    pub title_tags: Vec<String>,
}

/// Whether a profile shows a category with these traits. A category chosen
/// by hand wins over every rule.
pub(crate) fn includes(profile: &LibraryProfile, category_id: &str, kind: CategoryKind, traits: &CategoryTraits) -> bool {
    if profile.included_categories.iter().any(|id| id == category_id) {
        return true;
    }
    if profile.excluded_categories.iter().any(|id| id == category_id) || profile.hidden_kinds.contains(&kind) {
        return false;
    }
    if profile.hide_adult && traits.adult {
        return false;
    }
    if !profile.languages.is_empty() {
        let shown = if traits.languages.is_empty() {
            profile.include_unlabeled
        } else {
            traits.languages.iter().any(|language| profile.languages.contains(language))
        };
        if !shown {
            return false;
        }
    }
    // Categories naming no genre stay: "NETFLIX MOVIES" could hold anything.
    !(profile.only_genres
        && !profile.genres.is_empty()
        && !traits.genres.is_empty()
        && !traits.genres.iter().any(|genre| profile.genres.contains(genre)))
}

/// Country tags a profile's languages leave out of titles ("(JP)", "(IN)"):
/// brand categories such as "NETFLIX SERIES" mix countries, but their titles
/// often carry their own tag.
pub(crate) fn title_tags(profile: &LibraryProfile) -> Vec<String> {
    if profile.languages.is_empty() {
        return Vec::new();
    }
    let mut tags: Vec<String> = classify::CODES
        .iter()
        .filter(|(code, language)| code.len() == 2 && !MULTILINGUAL.contains(code) && !profile.languages.contains(language))
        .map(|(code, _)| (*code).to_string())
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

/// Whether a title carries one of these country tags.
pub(crate) fn tagged(title: &str, tags: &[String]) -> bool {
    if tags.is_empty() || !title.contains('(') {
        return false;
    }
    let upper = title.to_uppercase();
    tags.iter().any(|tag| upper.contains(&format!("({tag})")))
}

/// Whether the profile could leave anything out, so lists can skip reading
/// every category when it can't.
fn filters(profile: &LibraryProfile) -> bool {
    !profile.languages.is_empty()
        || profile.hide_adult
        || (profile.only_genres && !profile.genres.is_empty())
        || !profile.hidden_kinds.is_empty()
        || !profile.excluded_categories.is_empty()
}

fn home_mode(mode: &str) -> Result<&'static str> {
    HOME_MODES.iter().copied().find(|known| *known == mode).ok_or_else(|| anyhow::anyhow!("Home has no layout called “{mode}”."))
}

/// A profile as it's kept: a name without stray spaces, and each choice once.
fn tidied(mut profile: LibraryProfile) -> LibraryProfile {
    profile.name = profile.name.trim().to_string();
    profile.languages.sort();
    profile.languages.dedup();
    let mut seen = HashSet::new();
    profile.genres.retain(|genre| seen.insert(*genre));
    let mut kinds = HashSet::new();
    profile.hidden_kinds.retain(|kind| kinds.insert(*kind));
    let mut included = HashSet::new();
    profile.included_categories.retain(|id| included.insert(id.clone()));
    // A category is either always shown or always left out, never both.
    profile.excluded_categories.retain(|id| !included.contains(id));
    profile.excluded_categories.sort();
    profile.excluded_categories.dedup();
    profile
}

impl Core {
    /// Every profile in the order they were made, and the one in use.
    pub fn profiles(&self) -> Result<Profiles> {
        Ok(Profiles { profiles: self.stored_profiles()?, active_id: self.current_profile()?.id.clone() })
    }

    /// Switches profile: every list follows at once.
    pub fn set_active_profile(&self, id: &str) -> Result<()> {
        if !self.stored_profiles()?.iter().any(|profile| profile.id == id) {
            anyhow::bail!("This profile no longer exists.");
        }
        self.library.set_preference(ACTIVE_KEY, Some(id))?;
        self.profile_changed();
        Ok(())
    }

    /// Adds a profile (one without an id gets one) or saves changes to one,
    /// and returns it as kept.
    pub fn save_profile(&self, profile: LibraryProfile) -> Result<LibraryProfile> {
        let mut profile = tidied(profile);
        if profile.name.is_empty() {
            anyhow::bail!("A profile needs a name.");
        }
        let existing = self.stored_profiles()?;
        if let Some(twin) = existing.iter().find(|other| other.id != profile.id && other.name.to_lowercase() == profile.name.to_lowercase()) {
            anyhow::bail!("There's already a profile called “{}”.", twin.name);
        }
        if profile.id.trim().is_empty() {
            profile.id = uuid::Uuid::new_v4().to_string();
        }
        // Everything was only implied until now; it's kept first so it stays first.
        if self.library.profiles()?.is_empty() && profile.id != EVERYTHING_ID {
            self.library.save_profile(EVERYTHING_ID, &serde_json::to_string(&everything())?)?;
        }
        self.library.save_profile(&profile.id, &serde_json::to_string(&profile)?)?;
        self.profile_changed();
        Ok(profile)
    }

    /// Deletes a profile and its Home layouts. The last one can't go; when
    /// the one in use goes, the first one left takes over.
    pub fn delete_profile(&self, id: &str) -> Result<()> {
        let profiles = self.stored_profiles()?;
        if !profiles.iter().any(|profile| profile.id == id) {
            return Ok(());
        }
        if profiles.len() == 1 {
            anyhow::bail!("Xorvio needs at least one profile. Make another one first.");
        }
        let active = self.current_profile()?.id == id;
        self.library.delete_profile(id)?;
        if active && let Some(next) = profiles.iter().find(|profile| profile.id != id) {
            self.library.set_preference(ACTIVE_KEY, Some(&next.id))?;
        }
        self.profile_changed();
        Ok(())
    }

    /// Every category that holds something, with what its name says and how
    /// much it holds, for the profile editor. Whatever the profile, and
    /// hidden ones too, but never locked ones until the PIN is entered.
    pub fn profile_categories(&self) -> Result<Vec<ProfileCategory>> {
        let edits = self.load_edits()?;
        let locked = if edits.has_pin && !self.is_unlocked() { self.locked_ids(&edits)? } else { HashSet::new() };
        let sources = self.active_source_ids();
        let counts = self.catalog.category_counts(&sources)?;
        Ok(self
            .catalog
            .all_categories(&sources)?
            .into_iter()
            .filter(|category| !locked.contains(&category.id))
            .filter_map(|category| {
                let item_count = counts.get(&category.id).copied().unwrap_or(0);
                if item_count == 0 {
                    return None;
                }
                let traits = self.traits_of(&category.name);
                let shown = self.present_category_with(&edits, category);
                Some(ProfileCategory {
                    id: shown.id,
                    source_id: shown.source_id,
                    kind: shown.kind,
                    name: shown.name,
                    item_count,
                    languages: traits.languages.iter().copied().collect(),
                    genres: traits.genres.iter().copied().collect(),
                    adult: traits.adult,
                })
            })
            .collect())
    }

    /// The active profile's Home layout for when the sources (`"sources"`)
    /// or add-ons (`"addons"`) lead, as the interface last saved it; `None`
    /// when it's never been changed.
    pub fn home_layout(&self, mode: &str) -> Result<Option<serde_json::Value>> {
        let mode = home_mode(mode)?;
        let profile = self.current_profile()?;
        Ok(self.library.home_layout(&profile.id, mode)?.and_then(|json| match serde_json::from_str(&json) {
            Ok(layout) => Some(layout),
            Err(error) => {
                tracing::warn!("Home's saved layout couldn't be read, so it's back to the standard one: {error}");
                None
            }
        }))
    }

    /// Saves the active profile's Home layout; `None` goes back to the standard one.
    pub fn save_home_layout(&self, mode: &str, layout: Option<serde_json::Value>) -> Result<()> {
        let mode = home_mode(mode)?;
        let profile = self.current_profile()?;
        if layout.as_ref().is_some_and(|layout| !layout.is_object()) {
            anyhow::bail!("Home's layout couldn't be saved.");
        }
        let json = layout.map(|layout| layout.to_string());
        self.library.set_home_layout(&profile.id, mode, json.as_deref())
    }

    /// Leaves out of add-on titles what the active profile can judge without
    /// a category: kinds it hides, adult genres when it hides them, other
    /// genres when it shows only its own, and other countries' title tags.
    /// Add-ons don't say a title's language, so languages can't apply.
    pub fn addon_titles_in_profile(&self, movies: &mut Vec<Movie>, series: &mut Vec<Series>) -> Result<()> {
        let profile = self.current_profile()?;
        if !filters(&profile) {
            return Ok(());
        }
        let tags = title_tags(&profile);
        // A title's genre field is judged like a category's name, by the rules that can apply to it.
        let rules = LibraryProfile {
            languages: Vec::new(),
            hidden_kinds: Vec::new(),
            included_categories: Vec::new(),
            excluded_categories: Vec::new(),
            ..(*profile).clone()
        };
        let shown = |title: &str, genre: Option<&str>| {
            if tagged(title, &tags) {
                return false;
            }
            let Some(genre) = genre else { return true };
            let traits = CategoryTraits { genres: classify::genres_in(genre), adult: classify::traits(genre).adult, ..Default::default() };
            includes(&rules, "", CategoryKind::Movie, &traits)
        };
        if profile.hidden_kinds.contains(&CategoryKind::Movie) {
            movies.clear();
        }
        if profile.hidden_kinds.contains(&CategoryKind::Series) {
            series.clear();
        }
        movies.retain(|movie| shown(&movie.title, movie.genre.as_deref()));
        series.retain(|show| shown(&show.title, show.genre.as_deref()));
        Ok(())
    }

    // MARK: Filtering

    /// The categories the active profile leaves out, of every source (a
    /// paused one can still be listed by itself), and its title tags.
    pub(crate) fn profile_exclusions(&self) -> Result<ProfileExclusions> {
        let profile = self.current_profile()?;
        if !filters(&profile) {
            return Ok(ProfileExclusions::default());
        }
        let sources: Vec<String> = self.configs().into_iter().map(|config| config.id).collect();
        let categories = self
            .catalog
            .all_categories(&sources)?
            .into_iter()
            .filter(|category| !includes(&profile, &category.id, category.kind, &self.traits_of(&category.name)))
            .map(|category| category.id)
            .collect();
        Ok(ProfileExclusions { categories, title_tags: title_tags(&profile) })
    }

    /// Categories of one kind named for a genre ("EN - ACTION", "Action
    /// Movies"), for Home's genre rows. Exclusions are applied by the list.
    pub(crate) fn categories_of_genre(&self, kind: CategoryKind, genre: ContentGenre) -> Result<Vec<String>> {
        Ok(self
            .catalog
            .all_categories(&self.active_source_ids())?
            .into_iter()
            .filter(|category| category.kind == kind && self.traits_of(&category.name).genres.contains(&genre))
            .map(|category| category.id)
            .collect())
    }

    /// The profile in use: the one chosen, or the first when there's no
    /// choice or it's gone.
    pub(crate) fn current_profile(&self) -> Result<Arc<LibraryProfile>> {
        let mut cached = lock(&self.profile_state.active);
        if let Some(profile) = cached.as_ref() {
            return Ok(profile.clone());
        }
        let chosen = self.library.preference(ACTIVE_KEY)?;
        let mut profiles = self.stored_profiles()?;
        let at = chosen.and_then(|id| profiles.iter().position(|profile| profile.id == id)).unwrap_or(0);
        let profile = Arc::new(profiles.swap_remove(at));
        *cached = Some(profile.clone());
        Ok(profile)
    }

    /// Forgets the cached profile after a change.
    pub(crate) fn profile_changed(&self) {
        *lock(&self.profile_state.active) = None;
    }

    /// Profiles as kept, or Everything alone when none are. A row that can't
    /// be read (from a newer version, say) is skipped rather than failing
    /// every list.
    fn stored_profiles(&self) -> Result<Vec<LibraryProfile>> {
        let mut profiles: Vec<LibraryProfile> = self
            .library
            .profiles()?
            .into_iter()
            .filter_map(|(id, json)| match serde_json::from_str::<LibraryProfile>(&json) {
                Ok(profile) => Some(LibraryProfile { id, ..profile }),
                Err(error) => {
                    tracing::warn!("a library profile couldn't be read: {error}");
                    None
                }
            })
            .collect();
        if profiles.is_empty() {
            profiles.push(everything());
        }
        Ok(profiles)
    }

    fn traits_of(&self, name: &str) -> Arc<CategoryTraits> {
        let mut known = lock(&self.profile_state.traits);
        if let Some(traits) = known.get(name) {
            return traits.clone();
        }
        // Names come and go with refreshes; a fresh start now and then keeps this small.
        if known.len() > 100_000 {
            known.clear();
        }
        let traits = Arc::new(classify::traits(name));
        known.insert(name.to_string(), traits.clone());
        traits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xorvio_model::{ContentGenre, ContentLanguage};

    fn shows(profile: &LibraryProfile, name: &str) -> bool {
        includes(profile, name, CategoryKind::Movie, &classify::traits(name))
    }

    #[test]
    fn profiles_filter_by_language_genre_and_adult() {
        let mut profile = LibraryProfile {
            name: "Me".into(),
            languages: vec![ContentLanguage::English],
            hide_adult: true,
            genres: vec![ContentGenre::Action],
            ..Default::default()
        };
        assert!(shows(&profile, "EN - ACTION"));
        assert!(!shows(&profile, "QC - ANIMATION"));
        assert!(shows(&profile, "NETFLIX MOVIES"));
        assert!(!shows(&profile, "FOR ADULTS"));
        assert!(shows(&profile, "EN - COMEDY"));

        profile.only_genres = true;
        assert!(!shows(&profile, "EN - COMEDY"));
        assert!(shows(&profile, "NETFLIX MOVIES"), "mixed categories stay");

        profile.include_unlabeled = false;
        assert!(!shows(&profile, "NETFLIX MOVIES"));
        profile.included_categories = vec!["NETFLIX MOVIES".into()];
        assert!(shows(&profile, "NETFLIX MOVIES"), "a choice made by hand wins");
        profile.excluded_categories = vec!["EN - ACTION".into()];
        assert!(!shows(&profile, "EN - ACTION"));

        profile.hidden_kinds = vec![CategoryKind::Movie];
        assert!(!shows(&profile, "EN - ACTION"));
        assert!(includes(&profile, "x", CategoryKind::Series, &classify::traits("EN - ACTION")));

        assert!(shows(&everything(), "QC - ANIMATION"));
        assert!(shows(&everything(), "FOR ADULTS"), "Everything is everything");
        assert!(!filters(&everything()));
    }

    #[test]
    fn an_english_profile_leaves_out_other_countries_title_tags() {
        let english = LibraryProfile { languages: vec![ContentLanguage::English], ..Default::default() };
        let tags = title_tags(&english);
        assert!(tags.contains(&"JP".to_string()) && tags.contains(&"ES".to_string()));
        assert!(!tags.contains(&"US".to_string()) && !tags.contains(&"GB".to_string()) && !tags.contains(&"CA".to_string()));
        assert!(title_tags(&everything()).is_empty());
        assert!(tagged("Money Heist (es)", &tags));
        assert!(!tagged("The Office (US)", &tags));
        assert!(!tagged("Esme (2021)", &tags));
    }
}
