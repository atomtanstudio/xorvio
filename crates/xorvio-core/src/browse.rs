//! Browsing the catalog: categories, channels, films, series, search and the
//! guide. Lists cover every source that isn't paused, in the user's order,
//! and leave out what's hidden, locked behind the parental PIN or left out
//! by the active library profile, all through [`Core::excluded`]. Things
//! asked for by id (a guide row, a channel to play) are given whatever
//! their state: the lists that lead to them already left them out.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{Datelike, TimeZone};
use xorvio_model::{
    Category, CategoryKind, Channel, Episode, GuideRow, Movie, ProgramHit, SearchResults, Series, Timestamp,
};

use crate::classify;
use crate::store::catalog::{Browse, Order, SearchKind};
use crate::store::library::DEFAULT_LIST;
use crate::terms::{Person, TermMatches, people_found};
use crate::{BrowseQuery, BrowseSort, ChannelFilter, Core, Result};

/// The most people a search lists.
const MOST_PEOPLE_FOUND: usize = 12;

impl BrowseSort {
    fn order(self) -> Order {
        match self {
            Self::RecentlyAdded => Order::RecentlyAdded,
            Self::Title => Order::Title,
            Self::Year => Order::Year,
            Self::Rating => Order::Rating,
        }
    }
}

impl Core {
    /// Categories of one kind that hold something, source by source.
    pub fn categories(&self, kind: CategoryKind) -> Result<Vec<Category>> {
        self.list_categories(kind, false)
    }

    /// Categories with hidden ones too, so they can be shown again. Locked
    /// ones still need the PIN.
    pub fn categories_with_hidden(&self, kind: CategoryKind) -> Result<Vec<Category>> {
        self.list_categories(kind, true)
    }

    fn list_categories(&self, kind: CategoryKind, include_hidden: bool) -> Result<Vec<Category>> {
        let excluded = self.excluded(include_hidden)?;
        let edits = self.current_edits();
        let mut result = Vec::new();
        for source in self.active_source_ids() {
            result.extend(
                self.catalog
                    .categories(kind, &source)?
                    .into_iter()
                    .filter(|category| !excluded.category(Some(&category.id)))
                    .map(|category| self.present_category_with(&edits, category)),
            );
        }
        Ok(result)
    }

    /// Channels in each source's own order, or a list's (Favorites or a
    /// group) in its order, similar ones as one row when that's on.
    pub fn channels(&self, filter: &ChannelFilter) -> Result<Vec<Channel>> {
        let sources = match &filter.source_id {
            Some(source) => vec![source.clone()],
            None => self.active_source_ids(),
        };
        let limit = filter.limit.unwrap_or(usize::MAX);
        let category = filter.category_id.as_deref();
        let excluded = self.excluded(filter.include_hidden)?;
        let list = filter.list_id.as_deref().or(filter.favorites_only.then_some(DEFAULT_LIST));
        let channels = if let Some(list) = list {
            let members = self.library.list_items(list)?;
            self.catalog
                .channels_by_ids(&members)?
                .into_iter()
                .filter(|channel| sources.contains(&channel.source_id))
                .filter(|channel| category.is_none() || channel.category_id.as_deref() == category)
                .filter(|channel| !excluded.channel(channel))
                .take(limit)
                .collect()
        } else {
            let excluded = excluded.ids();
            let mut found = Vec::new();
            for source in &sources {
                let remaining = limit - found.len();
                if remaining == 0 {
                    break;
                }
                found.extend(self.catalog.channels(source, category, filter.limit.map(|_| remaining), &excluded)?);
            }
            found
        };
        if filter.group_similar.unwrap_or_else(|| self.groups_similar_channels()) {
            Ok(self.group_similar(channels, &self.current_edits()))
        } else {
            Ok(self.present_channels(channels))
        }
    }

    pub fn channel(&self, id: &str) -> Result<Option<Channel>> {
        Ok(self.catalog.channel(id)?.map(|channel| self.present_channel(channel)))
    }

    pub fn movies(&self, query: &BrowseQuery) -> Result<Vec<Movie>> {
        let sources = self.browse_sources(query);
        let excluded = self.excluded(false)?;
        let genre = self.genre_filter(query, CategoryKind::Movie)?;
        let watched = self.watched_filter(CategoryKind::Movie, query.watched)?;
        let movies = self.catalog.movies(&browse(query, &sources, &excluded.ids(), excluded.title_tags(), &genre, &watched))?;
        self.present_movies(movies)
    }

    pub fn movie(&self, id: &str) -> Result<Option<Movie>> {
        Ok(self.present_movies(self.catalog.movies_by_ids(&[id.to_string()])?)?.pop())
    }

    pub fn series_list(&self, query: &BrowseQuery) -> Result<Vec<Series>> {
        let sources = self.browse_sources(query);
        let excluded = self.excluded(false)?;
        let genre = self.genre_filter(query, CategoryKind::Series)?;
        let watched = self.watched_filter(CategoryKind::Series, query.watched)?;
        let series = self.catalog.series_list(&browse(query, &sources, &excluded.ids(), excluded.title_tags(), &genre, &watched))?;
        self.present_series_list(series)
    }

    /// The sources a browse list covers: the one asked for, if it's active.
    fn browse_sources(&self, query: &BrowseQuery) -> Vec<String> {
        let active = self.active_source_ids();
        match &query.source_id {
            Some(source) => active.into_iter().filter(|id| id == source).collect(),
            None => active,
        }
    }

    /// What a query's genre is matched by: words in the genre field, and for
    /// a [`BrowseQuery::content_genre`], the categories named for it too.
    fn genre_filter(&self, query: &BrowseQuery, kind: CategoryKind) -> Result<(Vec<String>, Vec<String>)> {
        if let Some(genre) = query.content_genre {
            let words = classify::genre_keywords(genre).iter().map(|word| (*word).to_string()).collect();
            return Ok((words, self.categories_of_genre(kind, genre)?));
        }
        let words = query.genre.as_deref().map(str::trim).filter(|genre| !genre.is_empty()).map(str::to_string);
        Ok((words.into_iter().collect(), Vec::new()))
    }

    pub fn series(&self, id: &str) -> Result<Option<Series>> {
        Ok(self.present_series_list(self.catalog.series_by_ids(&[id.to_string()])?)?.pop())
    }

    pub fn episode(&self, id: &str) -> Result<Option<Episode>> {
        Ok(self.catalog.episodes_by_ids(&[id.to_string()])?.pop())
    }

    /// Genres the films (or series) carry, alphabetically. Live channels have none.
    pub fn genres(&self, kind: CategoryKind) -> Result<Vec<String>> {
        self.catalog.genres(kind, &self.active_source_ids(), &self.excluded(false)?.ids())
    }

    /// This year's and last year's titles the sources carry most copies of,
    /// with artwork: a free, honest stand-in for "Top 10". Films for
    /// [`CategoryKind::Movie`], series for [`CategoryKind::Series`], both for
    /// [`CategoryKind::Live`].
    pub fn top_titles(&self, kind: CategoryKind, limit: usize) -> Result<(Vec<Movie>, Vec<Series>)> {
        let sources = self.active_source_ids();
        let excluded = self.excluded(false)?;
        let (tags, excluded) = (excluded.title_tags().to_vec(), excluded.ids());
        let this_year = chrono::Local.timestamp_opt(crate::now(), 0).single().map_or(2026, |date| i64::from(date.year()));
        let minimum_year = Some(this_year - 1);
        let movies = if kind == CategoryKind::Series {
            Vec::new()
        } else {
            self.catalog
                .popular_movies(&sources, minimum_year, limit, &excluded, &tags)?
                .into_iter()
                .map(Self::present_movie)
                .collect()
        };
        let series = if kind == CategoryKind::Movie {
            Vec::new()
        } else {
            self.catalog
                .popular_series(&sources, minimum_year, limit, &excluded, &tags)?
                .into_iter()
                .map(Self::present_series)
                .collect()
        };
        Ok((movies, series))
    }

    /// Prefix search over channels, films and series (every word must start a
    /// word of the title), and guide programmes airing now or later whose
    /// title contains the text. Up to `limit` of each. Channels are found by
    /// the names people gave them too. Films and series are then found by
    /// the people in them, their genres and their story (`crate::terms`),
    /// after those found by name, each saying why, and the people found are
    /// listed too.
    pub fn search(&self, text: &str, limit: usize) -> Result<SearchResults> {
        self.search_with(text, limit, limit)
    }

    /// [`Core::search`] with up to `program_limit` programmes: a title
    /// shown daily fills a page of listings.
    pub fn search_with(&self, text: &str, limit: usize, program_limit: usize) -> Result<SearchResults> {
        let sources = self.active_source_ids();
        let excluded = self.excluded(false)?;
        let (ids, tags) = (excluded.ids(), excluded.title_tags());
        let mut channel_ids = self.catalog.search(text, SearchKind::Channel, &sources, limit, &ids, tags)?;
        let wanted = text.trim().to_lowercase();
        if !wanted.is_empty() {
            for (id, name) in self.current_edits().channel_names() {
                if name.to_lowercase().contains(&wanted) && !channel_ids.iter().any(|found| found == id) {
                    channel_ids.push(id.to_string());
                }
            }
        }
        let mut movie_ids = self.catalog.search(text, SearchKind::Movie, &sources, limit, &ids, tags)?;
        let mut series_ids = self.catalog.search(text, SearchKind::Series, &sources, limit, &ids, tags)?;
        // Then those found by the people in them, their genres or story.
        let movie_terms = self.term_matches(text, SearchKind::Movie, &sources, limit, &ids, tags)?;
        let series_terms = self.term_matches(text, SearchKind::Series, &sources, limit, &ids, tags)?;
        let mut reasons = BTreeMap::new();
        for (found, matches) in [(&mut movie_ids, &movie_terms), (&mut series_ids, &series_terms)] {
            for (id, reason) in &matches.found {
                if found.len() >= limit {
                    break;
                }
                if !found.contains(id) {
                    found.push(id.clone());
                    reasons.insert(id.clone(), reason.clone());
                }
            }
        }
        let now = crate::now();
        let programs = self.catalog.search_programs(text, &sources, now, now, program_limit, &ids)?;
        let channels = self
            .catalog
            .channels_by_ids(&channel_ids)?
            .into_iter()
            .filter(|channel| sources.contains(&channel.source_id) && !excluded.channel(channel))
            .take(limit)
            .collect();
        let movies: Vec<Movie> = self.catalog.movies_by_ids(&movie_ids)?.into_iter().filter(|movie| !excluded.movie(movie)).collect();
        let series: Vec<Series> = self.catalog.series_by_ids(&series_ids)?.into_iter().filter(|series| !excluded.series(series)).collect();
        // People only from titles that may be shown, found or not.
        let in_people = |matches: &TermMatches| matches.people.iter().map(|(_, id)| id.clone()).collect::<Vec<_>>();
        let shown: HashSet<String> = self
            .catalog
            .movies_by_ids(&in_people(&movie_terms))?
            .into_iter()
            .filter(|movie| !excluded.movie(movie))
            .map(|movie| movie.id)
            .chain(self.catalog.series_by_ids(&in_people(&series_terms))?.into_iter().filter(|series| !excluded.series(series)).map(|series| series.id))
            .collect();
        let found_people: Vec<(Person, String)> =
            movie_terms.people.into_iter().chain(series_terms.people).filter(|(_, id)| shown.contains(id)).collect();
        reasons.retain(|id, _| movies.iter().any(|movie| movie.id == *id) || series.iter().any(|series| series.id == *id));
        Ok(SearchResults {
            people: people_found(&found_people, MOST_PEOPLE_FOUND),
            reasons,
            channels: self.present_channels(channels),
            movies: self.present_movies(movies)?,
            series: self.present_series_list(series)?,
            programs: programs
                .into_iter()
                .filter(|(_, channel)| !excluded.channel(channel))
                .map(|(program, channel)| ProgramHit { program, channel: self.present_channel(channel) })
                .collect(),
        })
    }

    // MARK: Guide

    /// Each channel's programmes overlapping `from`…`to`, in the order the
    /// channels were asked for. A channel without a guide has an empty row.
    pub fn guide(&self, channel_ids: &[String], from: Timestamp, to: Timestamp) -> Result<Vec<GuideRow>> {
        self.catalog
            .channels_by_ids(channel_ids)?
            .into_iter()
            .map(|channel| {
                let programs = self.catalog.channel_programs(&channel.id, from, to)?;
                Ok(GuideRow { channel: self.present_channel(channel), programs })
            })
            .collect()
    }

    /// What's on each channel now and next. When nothing is airing (a gap in
    /// the listings), the row holds only the next programme.
    pub fn now_next(&self, channel_ids: &[String], now: Timestamp) -> Result<Vec<GuideRow>> {
        self.catalog
            .channels_by_ids(channel_ids)?
            .into_iter()
            .map(|channel| {
                let mut programs = self.catalog.channel_upcoming(&channel.id, now, 2)?;
                if programs.first().is_some_and(|first| !first.is_airing(now)) {
                    programs.truncate(1);
                }
                Ok(GuideRow { channel: self.present_channel(channel), programs })
            })
            .collect()
    }

    /// Guide programmes starting between `from` and `to` across all channels,
    /// for a "Tonight on TV" row. Events people plan for come first: films,
    /// sport, and programmes flagged live, premiere or new, and those with
    /// artwork. News and paid programming are skipped and repeats used only
    /// to fill. At most two per channel, a programme simulcast on several
    /// channels once, ordered by start time.
    pub fn upcoming_programs(&self, from: Timestamp, to: Timestamp, limit: usize) -> Result<Vec<ProgramHit>> {
        if limit == 0 || to <= from {
            return Ok(Vec::new());
        }
        let excluded = self.excluded(false)?;
        let candidates =
            self.catalog.upcoming_events(&self.active_source_ids(), from, to, limit.saturating_mul(20).max(200), &excluded.ids())?;
        let mut per_channel: HashMap<String, usize> = HashMap::new();
        let mut seen = HashSet::new();
        let mut picked: Vec<ProgramHit> = Vec::new();
        for (program, channel) in candidates {
            if picked.len() == limit {
                break;
            }
            if excluded.channel(&channel) {
                continue;
            }
            let shown = per_channel.entry(channel.id.clone()).or_default();
            if *shown >= 2 || !seen.insert((crate::watchlist::normalized(&program.title), program.start)) {
                continue;
            }
            *shown += 1;
            picked.push(ProgramHit { program, channel: self.present_channel(channel) });
        }
        picked.sort_by(|a, b| a.program.start.cmp(&b.program.start).then(a.channel.sort_index.cmp(&b.channel.sort_index)));
        Ok(picked)
    }

    /// What's airing now, for Home's On Now row: favourite channels first,
    /// then recently watched ones, then others with artwork first, one
    /// programme per channel.
    pub fn on_now(&self, limit: usize) -> Result<Vec<ProgramHit>> {
        let now = crate::now();
        let sources = self.active_source_ids();
        let excluded = self.excluded(false)?;
        let mut own = self.library.list_items(DEFAULT_LIST)?;
        own.extend(self.library.recent_channel_ids(30)?);
        let mut seen = HashSet::new();
        let mut picked = Vec::new();
        for channel in self.catalog.channels_by_ids(&own)? {
            if picked.len() == limit {
                return Ok(picked);
            }
            if !sources.contains(&channel.source_id) || excluded.channel(&channel) || !seen.insert(channel.id.clone()) {
                continue;
            }
            let airing = self.catalog.channel_upcoming(&channel.id, now, 1)?.into_iter().find(|program| program.is_airing(now));
            if let Some(program) = airing {
                picked.push(ProgramHit { program, channel: self.present_channel(channel) });
            }
        }
        let others = self.catalog.airing(&sources, now, limit.saturating_mul(2), &excluded.ids())?;
        picked.extend(
            others
                .into_iter()
                .filter(|(_, channel)| !excluded.channel(channel) && seen.insert(channel.id.clone()))
                .map(|(program, channel)| ProgramHit { program, channel: self.present_channel(channel) }),
        );
        picked.truncate(limit);
        Ok(picked)
    }

    /// A programme by id (`<source>|<guide channel>|<start>`) with the channel
    /// that shows it.
    pub fn program(&self, program_id: &str) -> Result<Option<ProgramHit>> {
        let Some((source_id, rest)) = program_id.split_once('|') else { return Ok(None) };
        let Some((epg_id, start)) = rest.rsplit_once('|') else { return Ok(None) };
        let Ok(start) = start.parse::<Timestamp>() else { return Ok(None) };
        let Some(program) = self.catalog.program(source_id, epg_id, start)? else { return Ok(None) };
        let Some(channel) = self.catalog.channel_for_guide(source_id, epg_id)? else { return Ok(None) };
        Ok(Some(ProgramHit { program, channel: self.present_channel(channel) }))
    }
}

fn browse<'a>(
    query: &'a BrowseQuery,
    sources: &'a [String],
    excluded: &'a [String],
    title_tags: &'a [String],
    (genre_words, genre_categories): &'a (Vec<String>, Vec<String>),
    (only, without): &'a (Option<Vec<String>>, Vec<String>),
) -> Browse<'a> {
    Browse {
        source_ids: sources,
        category_id: query.category_id.as_deref(),
        genre_words,
        genre_categories,
        excluded,
        title_tags,
        only: only.as_deref(),
        without,
        group_versions: query.group_versions,
        order: query.sort.order(),
        limit: query.limit,
        offset: query.offset,
        years: None,
    }
}
