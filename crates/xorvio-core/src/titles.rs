//! What lists of films and series show besides the titles themselves: which
//! films are copies of one another (the versions a provider and the other
//! sources carry), what's been watched of each, and posters found on TMDB
//! for titles whose source gave none.
//!
//! Films are the same film when they share a TMDB id, else an IMDb id, else
//! a title and year (the catalog's `version_key`). Lists keep the work in
//! SQL: the library's progress goes in as lists of ids, never row by row.

use std::collections::{HashMap, HashSet};

use xorvio_model::{CategoryKind, MediaKind, Movie, PlaybackProgress, Series, TitleMark, TitleVersion};

use crate::names;
use crate::store::library::{fraction, is_resumable};
use crate::{Core, Result, WatchedFilter};

impl Core {
    /// What a list's watched filter keeps, as (only these titles, not these).
    /// Films go by their own progress; series by their episodes': one started
    /// is under way, and it's done once every episode the catalog lists for
    /// it (specials aside) is finished.
    pub(crate) fn watched_filter(&self, kind: CategoryKind, filter: WatchedFilter) -> Result<(Option<Vec<String>>, Vec<String>)> {
        if filter == WatchedFilter::All {
            return Ok((None, Vec::new()));
        }
        if kind == CategoryKind::Movie {
            let progress = self.library.progress_of_kind(MediaKind::Movie)?;
            return Ok(match filter {
                WatchedFilter::Unwatched => (None, progress.into_iter().filter(|p| p.finished).map(|p| p.item_id).collect()),
                _ => (Some(progress.into_iter().filter(is_resumable).map(|p| p.item_id).collect()), Vec::new()),
            });
        }
        let episodes = self.library.progress_of_kind(MediaKind::Episode)?;
        let started: HashSet<String> = episodes.iter().filter_map(|p| p.series_id.clone()).collect();
        let started: Vec<String> = started.into_iter().collect();
        if filter == WatchedFilter::Unwatched {
            return Ok((None, started));
        }
        let finished: Vec<String> = episodes.iter().filter(|p| p.finished).map(|p| p.item_id.clone()).collect();
        let counts = self.catalog.episodes_finished(&started, &finished)?;
        let done = |series_id: &String| counts.get(series_id).is_some_and(|(all, watched)| *all > 0 && watched >= all);
        Ok((Some(started.iter().filter(|id| !done(id)).cloned().collect()), Vec::new()))
    }

    /// Versions and what's been watched, for posters in a list. A film
    /// counts every copy of it in the active sources, and is watched when
    /// any copy was finished; its progress is the copy last played part-way.
    /// A series is watched once every episode the catalog lists for it is.
    pub fn title_marks(&self, kind: CategoryKind, ids: &[String]) -> Result<Vec<TitleMark>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        if kind == CategoryKind::Series {
            return self.series_marks(ids);
        }
        let excluded = self.excluded(false)?;
        let copies = self.catalog.movie_copies(ids, &self.active_source_ids(), &excluded.ids(), excluded.title_tags())?;
        let all: Vec<String> = copies.iter().map(|(_, copy)| copy.id.clone()).collect();
        let progress = self.library.progress_many(&all)?;
        let mut by_film: HashMap<&str, Vec<&str>> = HashMap::new();
        for (film, copy) in &copies {
            by_film.entry(film.as_str()).or_default().push(copy.id.as_str());
        }
        Ok(ids
            .iter()
            .map(|id| {
                let copies = by_film.get(id.as_str()).cloned().unwrap_or_else(|| vec![id.as_str()]);
                let played: Vec<&PlaybackProgress> = copies.iter().filter_map(|copy| progress.get(*copy)).collect();
                let latest = played.iter().filter(|p| is_resumable(p)).max_by_key(|p| p.updated_at);
                TitleMark {
                    id: id.clone(),
                    versions: copies.iter().collect::<HashSet<_>>().len() as i64,
                    watched: played.iter().any(|p| p.finished),
                    progress: latest.map(|p| fraction(p)),
                }
            })
            .collect())
    }

    fn series_marks(&self, ids: &[String]) -> Result<Vec<TitleMark>> {
        let progress = self.library.progress_of_series(ids)?;
        let finished: Vec<String> = progress.iter().filter(|p| p.finished).map(|p| p.item_id.clone()).collect();
        let counts = self.catalog.episodes_finished(ids, &finished)?;
        Ok(ids
            .iter()
            .map(|id| {
                let (all, watched) = counts.get(id).copied().unwrap_or_default();
                let done = all > 0 && watched >= all;
                TitleMark {
                    id: id.clone(),
                    versions: 1,
                    watched: done,
                    progress: (!done && all > 0 && watched > 0).then(|| watched as f64 / all as f64),
                }
            })
            .collect())
    }

    /// Every copy of a film the active sources carry, for its page's
    /// Versions picker: the film itself first, then by source in the
    /// person's order, the best picture first within each. Copies kept out
    /// of lists are left out, though the film asked for is always there.
    pub fn movie_versions(&self, movie_id: &str) -> Result<Vec<TitleVersion>> {
        let excluded = self.excluded(false)?;
        let sources = self.active_source_ids();
        let copies: Vec<Movie> =
            self.catalog.movie_copies(&[movie_id.to_string()], &sources, &excluded.ids(), excluded.title_tags())?.into_iter().map(|(_, copy)| copy).collect();
        let category_ids: Vec<String> = copies.iter().filter_map(|copy| copy.category_id.clone()).collect();
        let edits = self.current_edits();
        let categories: HashMap<String, String> = self
            .catalog
            .categories_by_ids(&category_ids)?
            .into_iter()
            .map(|category| (category.id.clone(), self.present_category_with(&edits, category).name))
            .collect();
        let names: HashMap<String, String> = self.configs().into_iter().map(|config| (config.id, config.name)).collect();
        let mut versions: Vec<(usize, names::Quality, TitleVersion)> = copies
            .into_iter()
            .map(|copy| {
                let quality = names::version_name(&copy.title).quality;
                let place = sources.iter().position(|source| *source == copy.source_id).unwrap_or(usize::MAX);
                let version = TitleVersion {
                    source_name: names.get(&copy.source_id).cloned().unwrap_or_default(),
                    category_name: copy.category_id.as_ref().and_then(|id| categories.get(id)).cloned(),
                    quality: quality.label().map(str::to_string),
                    extension: copy.stream.extension().filter(|extension| !extension.is_empty()),
                    movie: copy,
                };
                (place, quality, version)
            })
            .collect();
        versions.sort_by(|(a_place, a_quality, a), (b_place, b_quality, b)| {
            (b.movie.id == movie_id).cmp(&(a.movie.id == movie_id)).then(a_place.cmp(b_place)).then(b_quality.cmp(a_quality))
        });
        let movies = self.present_movies(versions.iter().map(|(_, _, version)| version.movie.clone()).collect())?;
        Ok(versions.into_iter().zip(movies).map(|((_, _, version), movie)| TitleVersion { movie, ..version }).collect())
    }

    // MARK: Posters

    /// Films as lists show them: tidy titles, and a poster TMDB had for
    /// those whose source gave none.
    pub(crate) fn present_movies(&self, movies: Vec<Movie>) -> Result<Vec<Movie>> {
        let missing: Vec<String> = movies.iter().filter(|movie| movie.poster_url.is_none()).map(|movie| movie.id.clone()).collect();
        let posters = if missing.is_empty() { HashMap::new() } else { self.catalog.filled_posters(&missing)? };
        Ok(movies
            .into_iter()
            .map(|mut movie| {
                if movie.poster_url.is_none() {
                    movie.poster_url = posters.get(&movie.id).cloned();
                }
                Self::present_movie(movie)
            })
            .collect())
    }

    pub(crate) fn present_series_list(&self, series: Vec<Series>) -> Result<Vec<Series>> {
        let missing: Vec<String> = series.iter().filter(|show| show.poster_url.is_none()).map(|show| show.id.clone()).collect();
        let posters = if missing.is_empty() { HashMap::new() } else { self.catalog.filled_posters(&missing)? };
        Ok(series
            .into_iter()
            .map(|mut show| {
                if show.poster_url.is_none() {
                    show.poster_url = posters.get(&show.id).cloned();
                }
                Self::present_series(show)
            })
            .collect())
    }
}
