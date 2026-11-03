//! Home's rows that aren't a plain browse list, as Xorvio for Mac's
//! HomeContent has them: New Releases here, and New Episodes with the check
//! that finds them (new_episodes.rs). Both follow the active profile, hiding
//! and the parental PIN through [`Core::excluded`].

use std::collections::HashSet;

use chrono::{Datelike, TimeZone};
use xorvio_model::Movie;

use crate::store::catalog::{Browse, Order, title_key};
use crate::{Core, Result, now};

/// One card per film: providers list a film in several categories and
/// qualities, and two sources can both carry it.
fn film_key(movie: &Movie) -> String {
    match movie.tmdb_id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => format!("tmdb-{id}"),
        None => format!("{}-{}", title_key(&movie.title), movie.year.unwrap_or(0)),
    }
}

impl Core {
    /// New Releases: films from this year and last, this year's first and
    /// the most recently added first within a year, one card per film.
    pub fn new_releases(&self, limit: usize) -> Result<Vec<Movie>> {
        let sources = self.active_source_ids();
        let excluded = self.excluded(false)?;
        let ids = excluded.ids();
        let this_year = chrono::Local.timestamp_opt(now(), 0).single().map_or(2026, |date| i64::from(date.year()));
        let movies = self.catalog.movies(&Browse {
            source_ids: &sources,
            category_id: None,
            genre_words: &[],
            genre_categories: &[],
            excluded: &ids,
            title_tags: excluded.title_tags(),
            only: None,
            without: &[],
            // Copies of one film are one card.
            group_versions: true,
            order: Order::NewReleases,
            // Copies take places too; enough to fill the row once they're folded.
            limit: limit.saturating_mul(4),
            offset: 0,
            // A year ahead too: a film out in December can be dated the next.
            years: Some((this_year - 1, this_year + 1)),
        })?;
        let mut seen = HashSet::new();
        Ok(movies.into_iter().filter(|movie| seen.insert(film_key(movie))).take(limit).map(Self::present_movie).collect())
    }
}
