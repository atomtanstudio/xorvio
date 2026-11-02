//! A complete, parsed copy of one source's catalog.
//!
//! A refresh builds the whole snapshot before anything is written, so a
//! download that fails half-way never damages the last usable catalog.

use std::collections::{BTreeMap, BTreeSet};
use xorvio_model::{Category, CategoryKind, Channel, Episode, Movie, Series, SourceCapabilities, Timestamp};

use crate::details::DetailsRecord;

/// Request headers a playlist asks for when playing one of its links
/// (`#EXTVLCOPT:http-user-agent=…`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StreamOptions {
    pub url: String,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CatalogSnapshot {
    pub categories: Vec<Category>,
    pub channels: Vec<Channel>,
    pub movies: Vec<Movie>,
    pub series: Vec<Series>,
    pub episodes: Vec<Episode>,
    pub stream_options: Vec<StreamOptions>,
    /// Guide link found inside the source (M3U `url-tvg`, Xtream `xmltv.php`).
    pub discovered_epg_url: Option<String>,
    pub capabilities: SourceCapabilities,
    pub max_connections: Option<i64>,
    pub account_expires: Option<Timestamp>,
    pub server_time_zone: Option<String>,
    /// Extra metadata the source's lists already carry, by item id.
    pub details: BTreeMap<String, DetailsRecord>,
    /// Kinds whose lists couldn't be downloaded this time. The writer keeps the
    /// previous rows for them instead of replacing them with nothing.
    pub kept_kinds: BTreeSet<KeptKind>,
    /// Plain-language notes about parts of the catalog that couldn't be refreshed.
    pub warnings: Vec<String>,
    /// What a media server remembers watching, merged into Xorvio's own
    /// progress once the catalog is stored.
    pub progress: Vec<crate::sources::server::ServerProgress>,
}

/// The parts of a catalog that can be kept from an earlier refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum KeptKind {
    Movies,
    Series,
}

impl KeptKind {
    pub fn category_kind(self) -> CategoryKind {
        match self {
            Self::Movies => CategoryKind::Movie,
            Self::Series => CategoryKind::Series,
        }
    }
}

/// The default threshold for [`CatalogSnapshot::keep_lists_that_came_back_empty`].
pub(crate) const EMPTY_LIST_THRESHOLD: i64 = 50;

impl CatalogSnapshot {
    /// A provider that answers with an empty film or series list, where there
    /// were plenty before, has almost certainly had a hiccup (overloaded or
    /// rate-limited panels do this) rather than deleted its whole library. The
    /// previous list is kept and the problem reported, instead of emptying the
    /// library until the next refresh.
    pub fn keep_lists_that_came_back_empty(&mut self, previous_movies: i64, previous_series: i64, threshold: i64) {
        if self.movies.is_empty() && previous_movies >= threshold && self.kept_kinds.insert(KeptKind::Movies) {
            self.capabilities.movies = true;
            self.warnings.push(format!(
                "The provider sent an empty film list, so the previous {} films are still shown.",
                grouped(previous_movies)
            ));
        }
        if self.series.is_empty() && previous_series >= threshold && self.kept_kinds.insert(KeptKind::Series) {
            self.capabilities.series = true;
            self.warnings.push(format!(
                "The provider sent an empty series list, so the previous {} series are still shown.",
                grouped(previous_series)
            ));
        }
    }

    /// Capabilities read from what the snapshot holds.
    pub fn derive_capabilities(&mut self) {
        self.capabilities.live = !self.channels.is_empty();
        self.capabilities.movies = !self.movies.is_empty();
        self.capabilities.series = !self.series.is_empty();
        self.capabilities.replay = self.channels.iter().any(|channel| channel.catchup_days > 0);
        self.capabilities.guide = self.discovered_epg_url.is_some();
    }
}

/// 1234567 → "1,234,567".
pub(crate) fn grouped(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    if value < 0 { format!("-{out}") } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_lists_keep_the_previous_ones() {
        let mut snapshot = CatalogSnapshot::default();
        snapshot.keep_lists_that_came_back_empty(1234, 12, EMPTY_LIST_THRESHOLD);
        assert!(snapshot.kept_kinds.contains(&KeptKind::Movies));
        assert!(!snapshot.kept_kinds.contains(&KeptKind::Series), "a handful before isn't enough to assume a hiccup");
        assert!(snapshot.capabilities.movies);
        assert_eq!(snapshot.warnings, ["The provider sent an empty film list, so the previous 1,234 films are still shown."]);
        // Running it twice doesn't repeat the warning.
        snapshot.keep_lists_that_came_back_empty(1234, 12, EMPTY_LIST_THRESHOLD);
        assert_eq!(snapshot.warnings.len(), 1);
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1234567), "1,234,567");
    }
}
