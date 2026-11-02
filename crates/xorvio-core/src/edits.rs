//! Changes people make to how channels and categories appear: a name of
//! their own, hiding, and locking behind the parental PIN; and films and
//! series hidden from the library. The catalog keeps the provider's
//! originals, so each change can be undone, and catalog ids are built from
//! the provider's keys, so changes survive refreshes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use xorvio_model::{CategoryKind, Channel, EditKind, ItemEdit, Movie, Series};

use crate::parental::{self, PIN_KEY};
use crate::profiles;
use crate::sources::ids;
use crate::store::library::EditRow;
use crate::store::lock;
use crate::{Core, Result, names};

/// Someone's edits as the library holds them, with whether there's a PIN,
/// and how channels play: catch-up offsets and versions chosen.
#[derive(Default)]
pub(crate) struct Edits {
    rows: HashMap<String, EditRow>,
    pub has_pin: bool,
    catchup_offsets: HashMap<String, i64>,
    /// Channel ids by group key.
    preferred_versions: HashMap<String, String>,
}

impl Edits {
    /// The name someone gave a channel or category.
    pub fn name(&self, item_id: &str) -> Option<&str> {
        self.rows.get(item_id)?.display_name.as_deref()
    }

    /// Minutes a channel's replays are moved by.
    pub fn catchup_offset(&self, channel_id: &str) -> i64 {
        self.catchup_offsets.get(channel_id).copied().unwrap_or(0)
    }

    /// The channel someone chose to play for a group of similar channels.
    pub fn preferred_version(&self, group_key: &str) -> Option<&str> {
        self.preferred_versions.get(group_key).map(String::as_str)
    }

    /// Channels someone renamed, as (id, their name).
    pub fn channel_names(&self) -> impl Iterator<Item = (&str, &str)> {
        self.rows.values().filter(|row| edit_kind(&row.item_id) == Some(EditKind::Channel)).filter_map(|row| {
            let name = row.display_name.as_deref()?;
            Some((row.item_id.as_str(), name))
        })
    }

    /// No choice was made about locking this, so its name decides.
    fn lock_left_to_name(&self, item_id: &str) -> bool {
        self.rows.get(item_id).is_none_or(|row| row.locked.is_none())
    }
}

/// Channels and categories kept out of lists right now: hidden, locked
/// behind the PIN, or left out by the active profile. Their ids never
/// collide ("…|ch|…" against "…|livecat|…"), so one set holds both.
pub(crate) struct Excluded {
    ids: HashSet<String>,
    /// Country tags ("JP") whose films and series ("Title (JP)") the profile leaves out.
    title_tags: Vec<String>,
}

impl Excluded {
    /// The channel, or the category it's in, is kept out.
    pub fn channel(&self, channel: &Channel) -> bool {
        self.ids.contains(&channel.id) || self.category(channel.category_id.as_deref())
    }

    pub fn category(&self, category_id: Option<&str>) -> bool {
        category_id.is_some_and(|id| self.ids.contains(id))
    }

    /// The film or series, or the category it's in, is kept out.
    pub fn title(&self, id: &str, category_id: Option<&str>) -> bool {
        self.ids.contains(id) || self.category(category_id)
    }

    /// The film, or its category, is kept out, or its title carries a tag the profile leaves out.
    pub fn movie(&self, movie: &Movie) -> bool {
        self.title(&movie.id, movie.category_id.as_deref()) || profiles::tagged(&movie.title, &self.title_tags)
    }

    pub fn series(&self, series: &Series) -> bool {
        self.title(&series.id, series.category_id.as_deref()) || profiles::tagged(&series.title, &self.title_tags)
    }

    /// For SQL's `NOT IN (SELECT value FROM json_each(?))`.
    pub fn ids(&self) -> Vec<String> {
        self.ids.iter().cloned().collect()
    }

    pub fn title_tags(&self) -> &[String] {
        &self.title_tags
    }
}

/// What an id names, read from its shape ("<source>|ch|<key>",
/// "<source>|livecat|<key>", "<source>|mv|<key>"); `None` for anything that
/// can't be edited (an episode, "<series>|ep|<key>", among them).
pub(crate) fn edit_kind(item_id: &str) -> Option<EditKind> {
    if ids::series_of_episode(item_id).is_some() {
        return None;
    }
    match item_id.split('|').nth(1)? {
        "ch" => Some(EditKind::Channel),
        "livecat" => Some(EditKind::LiveCategory),
        "moviecat" => Some(EditKind::MovieCategory),
        "seriescat" => Some(EditKind::SeriesCategory),
        "mv" => Some(EditKind::Movie),
        "se" => Some(EditKind::Series),
        _ => None,
    }
}

/// Channels and categories can be renamed and locked; films and series only hidden.
fn editable(item_id: &str) -> Result<EditKind> {
    edit_kind(item_id)
        .filter(|kind| !matches!(kind, EditKind::Movie | EditKind::Series))
        .ok_or_else(|| anyhow::anyhow!("Only channels and categories can be changed this way."))
}

fn hideable(item_id: &str) -> Result<EditKind> {
    edit_kind(item_id).ok_or_else(|| anyhow::anyhow!("Only channels, categories, films and series can be hidden."))
}

fn category_edit_kind(kind: CategoryKind) -> EditKind {
    match kind {
        CategoryKind::Live => EditKind::LiveCategory,
        CategoryKind::Movie => EditKind::MovieCategory,
        CategoryKind::Series => EditKind::SeriesCategory,
    }
}

impl Core {
    // MARK: Names and hiding

    /// Gives a channel or category a name of its own; `None` or a blank name
    /// goes back to the provider's.
    pub fn rename_item(&self, item_id: &str, name: Option<&str>) -> Result<()> {
        editable(item_id)?;
        let name = name.map(str::trim).filter(|name| !name.is_empty()).map(str::to_string);
        self.change_edit(item_id, |row| row.display_name = name)
    }

    /// Hides a channel or category from Live TV, the guide, Home, search,
    /// Films and Series, or a film or series from the library (Films,
    /// Series, Home, search, the watchlist as shown), or shows it again.
    pub fn set_hidden(&self, item_id: &str, hidden: bool) -> Result<()> {
        hideable(item_id)?;
        self.change_edit(item_id, |row| row.hidden = hidden)
    }

    /// Shows every hidden channel, category, film and series again. Names and locks stay.
    pub fn show_all_hidden(&self) -> Result<()> {
        self.library.show_all_hidden()?;
        self.edits_changed();
        Ok(())
    }

    // MARK: Locks

    /// Locks a channel or category behind the PIN, or unlocks it. Locking
    /// needs a PIN; unlocking needs it entered this session.
    pub fn set_locked(&self, item_id: &str, locked: bool) -> Result<()> {
        let kind = editable(item_id)?;
        if !self.load_edits()?.has_pin {
            anyhow::bail!("Set a parental PIN in Settings first.");
        }
        if !locked && !self.is_unlocked() {
            anyhow::bail!("Enter the parental PIN first.");
        }
        // Categories that look adult lock themselves, so what's stored for
        // them is only a choice against that.
        let automatic = kind != EditKind::Channel && self.category_looks_adult(item_id)?;
        let stored = match (locked, automatic) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            _ => None,
        };
        self.change_edit(item_id, |row| row.locked = stored)
    }

    /// Everything behind the PIN: what someone locked, and categories whose
    /// names look adult unless someone unlocked them.
    pub(crate) fn locked_ids(&self, edits: &Edits) -> Result<HashSet<String>> {
        let mut locked: HashSet<String> =
            edits.rows.values().filter(|row| row.locked == Some(true)).map(|row| row.item_id.clone()).collect();
        for category in self.catalog.all_categories(&self.all_source_ids())? {
            if edits.lock_left_to_name(&category.id) && parental::looks_adult(&category.name) {
                locked.insert(category.id);
            }
        }
        Ok(locked)
    }

    fn category_looks_adult(&self, category_id: &str) -> Result<bool> {
        let source = ids::source_of(category_id).to_string();
        Ok(self
            .catalog
            .all_categories(&[source])?
            .iter()
            .any(|category| category.id == category_id && parental::looks_adult(&category.name)))
    }

    // MARK: Lists

    /// Channels and categories to leave out of lists: hidden ones (unless
    /// `include_hidden`), locked ones until the PIN is entered, and those the
    /// active profile leaves out (whatever `include_hidden` says: they
    /// aren't hidden, the profile shows a different library).
    pub(crate) fn excluded(&self, include_hidden: bool) -> Result<Excluded> {
        let edits = self.load_edits()?;
        let mut ids: HashSet<String> = if include_hidden {
            HashSet::new()
        } else {
            edits.rows.values().filter(|row| row.hidden).map(|row| row.item_id.clone()).collect()
        };
        if edits.has_pin && !self.is_unlocked() {
            ids.extend(self.locked_ids(&edits)?);
        }
        let profile = self.profile_exclusions()?;
        ids.extend(profile.categories);
        Ok(Excluded { ids, title_tags: profile.title_tags })
    }

    /// What series recordings pass over: hidden channels and categories, and
    /// locked ones whether or not the PIN has been entered, but nothing the
    /// profile leaves out. A series shouldn't record differently because
    /// someone switched profile or unlocked for a while.
    pub(crate) fn excluded_from_recording(&self) -> Result<Excluded> {
        let edits = self.load_edits()?;
        let mut ids: HashSet<String> = edits.rows.values().filter(|row| row.hidden).map(|row| row.item_id.clone()).collect();
        if edits.has_pin {
            ids.extend(self.locked_ids(&edits)?);
        }
        Ok(Excluded { ids, title_tags: Vec::new() })
    }

    /// Every channel and category renamed, hidden or locked (categories
    /// whose names look adult included), and every film and series hidden,
    /// for Settings and the interface's menus. Locked ones are left out
    /// until the PIN is entered, and so are items no longer in any source
    /// and locks kept while there's no PIN.
    pub fn item_edits(&self) -> Result<Vec<ItemEdit>> {
        let edits = self.load_edits()?;
        let unlocked = self.is_unlocked();
        let mut result = Vec::new();
        let mut add = |edit: ItemEdit| {
            let showing = edit.display_name.is_some() || edit.hidden || edit.locked;
            if showing && (unlocked || !edit.locked) {
                result.push(edit);
            }
        };

        for category in self.catalog.all_categories(&self.all_source_ids())? {
            let row = edits.rows.get(&category.id);
            let automatic = edits.has_pin && edits.lock_left_to_name(&category.id) && parental::looks_adult(&category.name);
            if row.is_none() && !automatic {
                continue;
            }
            add(ItemEdit {
                item_id: category.id.clone(),
                kind: category_edit_kind(category.kind),
                source_id: category.source_id.clone(),
                original_name: self.provider_name(&category.name),
                display_name: row.and_then(|row| row.display_name.clone()),
                hidden: row.is_some_and(|row| row.hidden),
                locked: automatic || (edits.has_pin && row.is_some_and(|row| row.locked == Some(true))),
                locked_automatically: automatic,
            });
        }

        let channel_ids: Vec<String> =
            edits.rows.keys().filter(|id| edit_kind(id) == Some(EditKind::Channel)).cloned().collect();
        for channel in self.catalog.channels_by_ids(&channel_ids)? {
            let Some(row) = edits.rows.get(&channel.id) else { continue };
            add(ItemEdit {
                item_id: channel.id.clone(),
                kind: EditKind::Channel,
                source_id: channel.source_id.clone(),
                original_name: self.provider_name(&channel.name),
                display_name: row.display_name.clone(),
                hidden: row.hidden,
                locked: edits.has_pin && row.locked == Some(true),
                locked_automatically: false,
            });
        }

        let hidden_titles = |kind: EditKind| -> Vec<String> {
            edits.rows.values().filter(|row| row.hidden && edit_kind(&row.item_id) == Some(kind)).map(|row| row.item_id.clone()).collect()
        };
        let titles = self
            .catalog
            .movies_by_ids(&hidden_titles(EditKind::Movie))?
            .into_iter()
            .map(|movie| (EditKind::Movie, movie.id, movie.source_id, movie.title, movie.year))
            .chain(
                self.catalog
                    .series_by_ids(&hidden_titles(EditKind::Series))?
                    .into_iter()
                    .map(|show| (EditKind::Series, show.id, show.source_id, show.title, show.year)),
            );
        for (kind, item_id, source_id, title, year) in titles {
            let title = names::media_title(&title);
            add(ItemEdit {
                item_id,
                kind,
                source_id,
                // The year tells copies of a remake apart.
                original_name: year.map_or_else(|| title.clone(), |year| format!("{title} ({year})")),
                display_name: None,
                hidden: true,
                locked: false,
                locked_automatically: false,
            });
        }
        Ok(result)
    }

    // MARK: The cache

    /// Edits as last read. Should the library be unreadable, names are left
    /// as the provider has them (filtering goes through [`Core::excluded`],
    /// which fails instead).
    pub(crate) fn current_edits(&self) -> Arc<Edits> {
        self.load_edits().unwrap_or_else(|error| {
            tracing::warn!("channel names and hiding couldn't be read: {error}");
            Arc::default()
        })
    }

    pub(crate) fn load_edits(&self) -> Result<Arc<Edits>> {
        // Read while holding the lock, so a change made meanwhile is never
        // overwritten by older rows.
        let mut cached = lock(&self.edits);
        if let Some(edits) = cached.as_ref() {
            return Ok(edits.clone());
        }
        let edits = Arc::new(Edits {
            rows: self.library.item_edits()?.into_iter().map(|row| (row.item_id.clone(), row)).collect(),
            has_pin: self.library.preference(PIN_KEY)?.is_some(),
            catchup_offsets: self.library.catchup_offsets()?.into_iter().collect(),
            preferred_versions: self.library.preferred_versions()?.into_iter().collect(),
        });
        *cached = Some(edits.clone());
        Ok(edits)
    }

    /// Forgets the cached edits after a change.
    pub(crate) fn edits_changed(&self) {
        *lock(&self.edits) = None;
    }

    fn change_edit(&self, item_id: &str, change: impl FnOnce(&mut EditRow)) -> Result<()> {
        self.library.edit_item(item_id, change)?;
        self.edits_changed();
        Ok(())
    }

    /// Every source, paused ones too: a paused source can still be browsed by name.
    fn all_source_ids(&self) -> Vec<String> {
        self.configs().into_iter().map(|config| config.id).collect()
    }
}
