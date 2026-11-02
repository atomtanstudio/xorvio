//! Readers for each kind of source. They know nothing about storage or the
//! interface: each one turns what a provider sends into a [`CatalogSnapshot`]
//! (or guide programmes), and provider-specific rules stay here.
//!
//! [`CatalogSnapshot`]: crate::snapshot::CatalogSnapshot

pub mod demo;
pub mod folder;
pub mod jellyfin;
pub mod m3u;
pub mod plex;
pub mod server;
pub mod stream;
pub mod tvmaze;
pub mod xmltv;
pub mod xtream;
pub mod xtream_details;

/// Catalog ids. They're built from the provider's own keys, never from links,
/// so editing a source's address keeps favorites, progress and the watchlist.
pub mod ids {
    use xorvio_model::CategoryKind;

    pub fn category(source_id: &str, kind: CategoryKind, key: &str) -> String {
        format!("{source_id}|{}cat|{key}", kind.as_str())
    }

    pub fn channel(source_id: &str, key: &str) -> String {
        format!("{source_id}|ch|{key}")
    }

    pub fn movie(source_id: &str, key: &str) -> String {
        format!("{source_id}|mv|{key}")
    }

    pub fn series(source_id: &str, key: &str) -> String {
        format!("{source_id}|se|{key}")
    }

    pub fn episode(series_id: &str, key: &str) -> String {
        format!("{series_id}|ep|{key}")
    }

    /// The series an episode id belongs to; `None` for anything else.
    pub fn series_of_episode(item_id: &str) -> Option<&str> {
        item_id.split_once("|ep|").map(|(series_id, _)| series_id)
    }

    /// The source an id belongs to: everything before the first `|`.
    pub fn source_of(item_id: &str) -> &str {
        item_id.split('|').next().unwrap_or(item_id)
    }
}

/// The steps a refresh reports, in the words the interface shows.
pub mod phase {
    pub const CONNECTING: &str = "Connecting";
    pub const READING_CATALOG: &str = "Reading catalog";
    pub const ORGANIZING: &str = "Organizing content";
    pub const LOADING_GUIDE: &str = "Loading guide";
}
