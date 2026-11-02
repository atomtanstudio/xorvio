//! The catalog database: categories, channels, films, series, episodes, the
//! guide, rich details and the search index, for every source.
//!
//! Reads and writes use separate WAL connections, so a long import never
//! blocks browsing and readers keep seeing the last committed catalog until an
//! import finishes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use xorvio_model::{
    Category, CategoryKind, Channel, Episode, Movie, Program, ProgramFlags, Series, SourceStatus, StreamLocator, Timestamp,
};

use super::{GuideOverride, json_list, lock, open_connection, sql_limit};
use crate::details::DetailsRecord;
use crate::{genres, names};
use crate::snapshot::{CatalogSnapshot, KeptKind};
use crate::sources::xmltv::ChannelName;

mod folders;

pub(crate) use folders::{FolderFilter, FolderPage, VideoMove};

const SCHEMA_VERSION: i64 = 6;

/// Changes to tables made by an earlier version, run before [`SCHEMA`] (whose
/// `CREATE … IF NOT EXISTS` leaves existing tables as they are). Version 3
/// indexes films and series by their IMDb and TMDB ids and title keys, which
/// is how titles are matched to Trakt's. Version 4 adds TMDB's tables, and
/// version 5 the index films are grouped into versions by, list posters
/// found on TMDB and filmographies from Wikidata, all of which the schema
/// makes. Version 6 adds what media folders' video files hold (file_facts),
/// which the schema makes too.
const MIGRATIONS: &[(i64, &str)] = &[
    (
        2,
        "ALTER TABLE channels ADD COLUMN guide_source TEXT;
         ALTER TABLE epg_channels ADD COLUMN name TEXT;",
    ),
    (
        3,
        "CREATE INDEX IF NOT EXISTS movies_imdb ON movies(imdb_id);
         CREATE INDEX IF NOT EXISTS movies_tmdb ON movies(tmdb_id);
         CREATE INDEX IF NOT EXISTS movies_title_key ON movies(title_key);
         CREATE INDEX IF NOT EXISTS series_imdb ON series(imdb_id);
         CREATE INDEX IF NOT EXISTS series_tmdb ON series(tmdb_id);
         CREATE INDEX IF NOT EXISTS series_title_key ON series(title_key);",
    ),
];

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS categories (
    id TEXT PRIMARY KEY, source_id TEXT NOT NULL, kind TEXT NOT NULL, name TEXT NOT NULL, sort_index INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS categories_source ON categories(source_id, kind, sort_index);

-- A channel shows guide channel COALESCE(epg_fallback, epg_id) of guide
-- COALESCE(guide_source, source_id): its own id in its source's guide unless
-- resolve_guide_links found it elsewhere or someone chose another.
CREATE TABLE IF NOT EXISTS channels (
    id TEXT PRIMARY KEY, source_id TEXT NOT NULL, name TEXT NOT NULL, name_key TEXT NOT NULL,
    number INTEGER, logo TEXT, category_id TEXT, epg_id TEXT, epg_fallback TEXT,
    stream TEXT NOT NULL, catchup_days INTEGER NOT NULL DEFAULT 0, catchup_source TEXT, sort_index INTEGER NOT NULL,
    guide_source TEXT
);
CREATE INDEX IF NOT EXISTS channels_source ON channels(source_id, sort_index);
CREATE INDEX IF NOT EXISTS channels_category ON channels(category_id, sort_index);
CREATE INDEX IF NOT EXISTS channels_guide ON channels(source_id, COALESCE(epg_fallback, epg_id));
CREATE INDEX IF NOT EXISTS channels_guide_link ON channels(COALESCE(guide_source, source_id), COALESCE(epg_fallback, epg_id));

CREATE TABLE IF NOT EXISTS movies (
    id TEXT PRIMARY KEY, source_id TEXT NOT NULL, title TEXT NOT NULL, title_key TEXT NOT NULL, year INTEGER,
    poster TEXT, backdrop TEXT, category_id TEXT, rating REAL, added_at INTEGER, plot TEXT, genre TEXT,
    tmdb_id TEXT, imdb_id TEXT, duration INTEGER, stream TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS movies_source ON movies(source_id);
CREATE INDEX IF NOT EXISTS movies_category ON movies(category_id);
CREATE INDEX IF NOT EXISTS movies_added ON movies(added_at DESC);
CREATE INDEX IF NOT EXISTS movies_imdb ON movies(imdb_id);
CREATE INDEX IF NOT EXISTS movies_tmdb ON movies(tmdb_id);
CREATE INDEX IF NOT EXISTS movies_title_key ON movies(title_key);
-- The copies of one film, by version_key below (written exactly the same,
-- or SQLite won't use it).
CREATE INDEX IF NOT EXISTS movies_version ON movies(COALESCE('t' || NULLIF(NULLIF(tmdb_id, ''), '0'), 'i' || NULLIF(imdb_id, ''), 'k' || title_key || '|' || IFNULL(year, '')));

CREATE TABLE IF NOT EXISTS series (
    id TEXT PRIMARY KEY, source_id TEXT NOT NULL, title TEXT NOT NULL, title_key TEXT NOT NULL, year INTEGER,
    poster TEXT, backdrop TEXT, category_id TEXT, rating REAL, added_at INTEGER, plot TEXT, genre TEXT,
    tmdb_id TEXT, imdb_id TEXT, remote_id TEXT
);
CREATE INDEX IF NOT EXISTS series_source ON series(source_id);
CREATE INDEX IF NOT EXISTS series_category ON series(category_id);
CREATE INDEX IF NOT EXISTS series_imdb ON series(imdb_id);
CREATE INDEX IF NOT EXISTS series_tmdb ON series(tmdb_id);
CREATE INDEX IF NOT EXISTS series_title_key ON series(title_key);

CREATE TABLE IF NOT EXISTS episodes (
    id TEXT PRIMARY KEY, series_id TEXT NOT NULL, source_id TEXT NOT NULL, season INTEGER NOT NULL,
    number INTEGER NOT NULL, title TEXT NOT NULL, plot TEXT, duration INTEGER, still TEXT, stream TEXT NOT NULL,
    air_date INTEGER
);
CREATE INDEX IF NOT EXISTS episodes_series ON episodes(series_id, season, number);
CREATE INDEX IF NOT EXISTS episodes_source ON episodes(source_id);

CREATE TABLE IF NOT EXISTS programs (
    source_id TEXT NOT NULL, epg_id TEXT NOT NULL, start INTEGER NOT NULL, end INTEGER NOT NULL,
    title TEXT NOT NULL, subtitle TEXT, summary TEXT, category TEXT, image TEXT, episode TEXT,
    flags INTEGER NOT NULL DEFAULT 0, rating TEXT, star REAL, year INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS programs_key ON programs(source_id, epg_id, start);
CREATE INDEX IF NOT EXISTS programs_end ON programs(end);

CREATE TABLE IF NOT EXISTS epg_channels (source_id TEXT NOT NULL, epg_id TEXT NOT NULL, name_key TEXT NOT NULL, name TEXT);
CREATE INDEX IF NOT EXISTS epg_channels_name ON epg_channels(source_id, name_key);
CREATE INDEX IF NOT EXISTS epg_channels_id ON epg_channels(source_id, epg_id);

CREATE TABLE IF NOT EXISTS stream_options (
    source_id TEXT NOT NULL, url TEXT NOT NULL, user_agent TEXT, referrer TEXT, PRIMARY KEY (source_id, url)
);

CREATE TABLE IF NOT EXISTS source_status (source_id TEXT PRIMARY KEY, json TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS media_details (
    item_id TEXT PRIMARY KEY, source_id TEXT NOT NULL, json TEXT NOT NULL, fetched_at INTEGER
);
CREATE INDEX IF NOT EXISTS media_details_source ON media_details(source_id);

-- What TMDB said about a film ('movie'), series ('tv') or person ('person'),
-- by TMDB's id, kept for a while so each is asked for now and then.
CREATE TABLE IF NOT EXISTS tmdb_cache (
    kind TEXT NOT NULL, tmdb_id INTEGER NOT NULL, json TEXT NOT NULL, fetched_at INTEGER NOT NULL,
    PRIMARY KEY (kind, tmdb_id)
);
-- Which TMDB id a title (by IMDb id, or title key and year) or a name turned
-- out to be; no id when TMDB had nothing convincing.
CREATE TABLE IF NOT EXISTS tmdb_matches (
    kind TEXT NOT NULL, lookup TEXT NOT NULL, tmdb_id INTEGER, looked_up_at INTEGER NOT NULL,
    PRIMARY KEY (kind, lookup)
);
-- Posters TMDB has for films and series whose source gave none, by catalog
-- id, so lists show them at once; no poster when TMDB had none.
CREATE TABLE IF NOT EXISTS tmdb_posters (
    item_id TEXT PRIMARY KEY, source_id TEXT NOT NULL, poster TEXT, looked_up_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS tmdb_posters_source ON tmdb_posters(source_id);
-- What a media folder's video file holds, read once with ffprobe, by
-- catalog id: its length, picture size, sound and size, for telling GIFs,
-- animations and videos apart (xorvio_model::FileFacts). A file that
-- couldn't be read has a row with no length, so it isn't read again.
CREATE TABLE IF NOT EXISTS file_facts (
    item_id TEXT PRIMARY KEY, source_id TEXT NOT NULL, duration REAL, width INTEGER, height INTEGER,
    audio INTEGER NOT NULL, size INTEGER, read_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS file_facts_source ON file_facts(source_id);
-- People Wikidata knows by a name (as a match key) with their credits, as
-- JSON: everyone of that name, so the one meant can be chosen as it's read.
CREATE TABLE IF NOT EXISTS wikidata_people (name_key TEXT PRIMARY KEY, json TEXT NOT NULL, fetched_at INTEGER NOT NULL);

CREATE VIRTUAL TABLE IF NOT EXISTS search USING fts5(
    title, item_id UNINDEXED, kind UNINDEXED, source_id UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
);
";

/// Programme flags as stored: one bit each.
mod flag {
    pub const NEW: i64 = 1;
    pub const PREMIERE: i64 = 2;
    pub const LIVE: i64 = 4;
    pub const REPEAT: i64 = 16;
}

fn flags_to_bits(flags: &ProgramFlags) -> i64 {
    [(flags.is_new, flag::NEW), (flags.is_premiere, flag::PREMIERE), (flags.is_live, flag::LIVE), (flags.is_repeat, flag::REPEAT)]
        .iter()
        .filter(|(set, _)| *set)
        .fold(0, |bits, (_, bit)| bits | bit)
}

fn bits_to_flags(bits: i64) -> ProgramFlags {
    ProgramFlags {
        is_new: bits & flag::NEW != 0,
        is_live: bits & flag::LIVE != 0,
        is_premiere: bits & flag::PREMIERE != 0,
        is_repeat: bits & flag::REPEAT != 0,
    }
}

fn encode_stream(locator: &StreamLocator) -> String {
    serde_json::to_string(locator).unwrap_or_default()
}

fn decode_stream(text: String) -> StreamLocator {
    serde_json::from_str(&text).unwrap_or(StreamLocator::Url { url: text })
}

fn kind_from(text: &str) -> CategoryKind {
    match text {
        "movie" => CategoryKind::Movie,
        "series" => CategoryKind::Series,
        _ => CategoryKind::Live,
    }
}

/// What a search index row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchKind {
    Channel,
    Movie,
    Series,
}

impl SearchKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Movie => "movie",
            Self::Series => "series",
        }
    }
}

/// How a browse list is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Order {
    RecentlyAdded,
    Title,
    Year,
    Rating,
    /// Newest year first, and within a year the most recently added (Home's New Releases).
    NewReleases,
}

impl Order {
    /// For `alias`, with the id last so no two rows tie and pages never
    /// overlap.
    fn sql(self, alias: &str) -> String {
        let a = alias;
        match self {
            Self::Title => format!("{a}.title COLLATE NOCASE ASC, {a}.id"),
            Self::RecentlyAdded => format!("{a}.added_at DESC NULLS LAST, {a}.title COLLATE NOCASE, {a}.id"),
            Self::Year => format!("{a}.year DESC NULLS LAST, {a}.title COLLATE NOCASE, {a}.id"),
            Self::Rating => format!("{a}.rating DESC NULLS LAST, {a}.title COLLATE NOCASE, {a}.id"),
            Self::NewReleases => format!("{a}.year DESC NULLS LAST, {a}.added_at DESC NULLS LAST, {a}.title COLLATE NOCASE, {a}.id"),
        }
    }

    /// `o` comes before `t` in this order ([`Order::sql`]), as a condition.
    fn precedes(self, o: &str, t: &str) -> String {
        let title = format!("({o}.title < {t}.title COLLATE NOCASE OR ({o}.title = {t}.title COLLATE NOCASE AND {o}.id < {t}.id))");
        // Missing values sort last, as NULLS LAST does.
        let first = |column: &str| {
            format!(
                "(IFNULL({o}.{column}, -1) > IFNULL({t}.{column}, -1) OR (IFNULL({o}.{column}, -1) = IFNULL({t}.{column}, -1) AND {title}))"
            )
        };
        match self {
            Self::Title => title,
            Self::RecentlyAdded => first("added_at"),
            Self::Year => first("year"),
            Self::Rating => first("rating"),
            Self::NewReleases => {
                let added = first("added_at");
                format!(
                    "(IFNULL({o}.year, -1) > IFNULL({t}.year, -1) OR (IFNULL({o}.year, -1) = IFNULL({t}.year, -1) AND {added}))"
                )
            }
        }
    }
}

/// What makes copies of one film the same film, whichever source carries
/// them: its TMDB id, else its IMDb id, else its title key and year. The
/// `movies_version` index is on exactly this.
fn version_key(alias: &str) -> String {
    let a = alias;
    format!("COALESCE('t' || NULLIF(NULLIF({a}.tmdb_id, ''), '0'), 'i' || NULLIF({a}.imdb_id, ''), 'k' || {a}.title_key || '|' || IFNULL({a}.year, ''))")
}

/// A browse list's conditions on the films or series of `t` (an alias):
/// sources (?1), a category (?2), a genre (?3, ?7, ?8), what's kept out
/// (?6), country tags (?9) and years (?12 to ?13).
fn browse_conditions(t: &str) -> String {
    format!(
        "{t}.source_id IN (SELECT value FROM json_each(?1))
         AND (?2 IS NULL OR {t}.category_id = ?2)
         AND (?12 IS NULL OR {t}.year BETWEEN ?12 AND ?13)
         AND (?3 = 0
              OR EXISTS (SELECT 1 FROM json_each(?7) WHERE {t}.genre LIKE '%' || value || '%' ESCAPE '\\')
              OR {t}.category_id IN (SELECT value FROM json_each(?8)))
         AND ({t}.category_id IS NULL OR {t}.category_id NOT IN (SELECT value FROM json_each(?6)))
         AND {t}.id NOT IN (SELECT value FROM json_each(?6))
         AND {}",
        untagged(t, "?9")
    )
}

/// What a browse list may be narrowed to.
pub(crate) struct Browse<'a> {
    pub source_ids: &'a [String],
    pub category_id: Option<&'a str>,
    /// Titles whose genre field contains one of these words ("Drama" finds
    /// "Crime, Drama"), or that are in one of `genre_categories`. Both
    /// empty: any genre.
    pub genre_words: &'a [String],
    pub genre_categories: &'a [String],
    /// Categories and titles kept out: hidden, locked behind the PIN, or
    /// left out by the profile.
    pub excluded: &'a [String],
    /// Country tags ("JP") whose titles ("Title (JP)") the profile leaves out.
    pub title_tags: &'a [String],
    /// Only these titles (with `group_versions`, any copy of them).
    pub only: Option<&'a [String]>,
    /// Not these titles (with `group_versions`, nor any copy of them).
    pub without: &'a [String],
    /// Copies of a film as one, the first in `order` standing for the rest.
    pub group_versions: bool,
    pub order: Order,
    pub limit: usize,
    pub offset: usize,
    /// Titles from these years only (first and last, inclusive).
    pub years: Option<(i64, i64)>,
}

/// What films or series are looked for by: any of these IMDb ids, TMDB ids
/// (as the catalog writes them, text) or title keys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MatchKeys {
    pub imdb: Vec<String>,
    pub tmdb: Vec<String>,
    pub title_keys: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub channels: i64,
    pub movies: i64,
    pub series: i64,
    pub programs: i64,
}

pub(crate) struct CatalogStore {
    path: PathBuf,
    writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
}

const CHANNEL_COLUMNS: &str =
    "id, source_id, name, number, logo, category_id, COALESCE(epg_fallback, epg_id), stream, catchup_days, catchup_source, sort_index";
const MOVIE_COLUMNS: &str =
    "id, source_id, title, year, poster, backdrop, category_id, rating, added_at, plot, genre, tmdb_id, imdb_id, duration, stream";
const SERIES_COLUMNS: &str =
    "id, source_id, title, year, poster, backdrop, category_id, rating, added_at, plot, genre, tmdb_id, imdb_id, remote_id";
const EPISODE_COLUMNS: &str = "id, series_id, source_id, season, number, title, plot, duration, still, stream, air_date";
const PROGRAM_COLUMNS: &str =
    "p.source_id, p.epg_id, p.start, p.end, p.title, p.subtitle, p.summary, p.category, p.image, p.episode, p.flags, p.rating, p.star, p.year";
const PROGRAM_COLUMN_COUNT: usize = 14;

/// Joins a channel (`c`) to the programmes (`p`) of the guide channel it shows.
const SHOWN_PROGRAMS: &str =
    "JOIN programs p ON p.source_id = COALESCE(c.guide_source, c.source_id) AND p.epg_id = COALESCE(c.epg_fallback, c.epg_id)";

/// Joins each programme to the first channel (by sort order), among the
/// sources bound to `sources` (a JSON list parameter such as `?3`), that shows
/// its guide channel. The mapping is worked out once per query and then
/// joined: a per-programme subquery made SQLite scan every channel for every
/// programme, about 86 s for a twelve-hour window of a large guide.
/// Channels and categories in `excluded` (another JSON list parameter) are
/// passed over, so a programme is shown on a channel that's listed.
fn showing_channel(sources: &str, excluded: &str) -> String {
    format!(
        "JOIN (
            SELECT COALESCE(guide_source, source_id) AS g_source, COALESCE(epg_fallback, epg_id) AS g_key,
                id AS g_channel, MIN(sort_index)
            FROM channels
            WHERE COALESCE(epg_fallback, epg_id) IS NOT NULL AND source_id IN (SELECT value FROM json_each({sources}))
              AND id NOT IN (SELECT value FROM json_each({excluded}))
              AND (category_id IS NULL OR category_id NOT IN (SELECT value FROM json_each({excluded})))
            GROUP BY g_source, g_key
        ) g ON g.g_source = p.source_id AND g.g_key = p.epg_id
        JOIN channels c ON c.id = g.g_channel"
    )
}

/// Leaves out titles of `table` tagged with a country in `tags` (a JSON list
/// parameter such as `?9`): "Title (JP)". Only titles with a two-letter tag
/// in brackets are looked at closely, which keeps large catalogs quick.
fn untagged(table: &str, tags: &str) -> String {
    format!(
        "({tags} = '[]' OR {table}.title NOT GLOB '*([A-Za-z][A-Za-z])*'
          OR NOT EXISTS (SELECT 1 FROM json_each({tags}) WHERE {table}.title LIKE '%(' || value || ')%'))"
    )
}

/// [`CHANNEL_COLUMNS`] for the `c` alias.
fn joined_channel_columns() -> &'static str {
    "c.id, c.source_id, c.name, c.number, c.logo, c.category_id, COALESCE(c.epg_fallback, c.epg_id), c.stream, \
     c.catchup_days, c.catchup_source, c.sort_index"
}

fn category_row(row: &Row<'_>) -> rusqlite::Result<Category> {
    Ok(Category {
        id: row.get(0)?,
        source_id: row.get(1)?,
        kind: kind_from(&row.get::<_, String>(2)?),
        name: row.get(3)?,
        sort_index: row.get(4)?,
    })
}

fn channel_row(row: &Row<'_>) -> rusqlite::Result<Channel> {
    channel_at(row, 0)
}

fn channel_at(row: &Row<'_>, at: usize) -> rusqlite::Result<Channel> {
    Ok(Channel {
        id: row.get(at)?,
        source_id: row.get(at + 1)?,
        name: row.get(at + 2)?,
        number: row.get(at + 3)?,
        logo_url: row.get(at + 4)?,
        category_id: row.get(at + 5)?,
        epg_id: row.get(at + 6)?,
        stream: decode_stream(row.get(at + 7)?),
        catchup_days: row.get(at + 8)?,
        catchup_source: row.get(at + 9)?,
        sort_index: row.get(at + 10)?,
        catchup_offset_minutes: 0,
        versions: Vec::new(),
    })
}

fn movie_row(row: &Row<'_>) -> rusqlite::Result<Movie> {
    movie_at(row, 0)
}

fn movie_at(row: &Row<'_>, at: usize) -> rusqlite::Result<Movie> {
    Ok(Movie {
        id: row.get(at)?,
        source_id: row.get(at + 1)?,
        title: row.get(at + 2)?,
        year: row.get(at + 3)?,
        poster_url: row.get(at + 4)?,
        backdrop_url: row.get(at + 5)?,
        category_id: row.get(at + 6)?,
        rating: row.get(at + 7)?,
        added_at: row.get(at + 8)?,
        plot: row.get(at + 9)?,
        genre: row.get(at + 10)?,
        tmdb_id: row.get(at + 11)?,
        imdb_id: row.get(at + 12)?,
        duration_seconds: row.get(at + 13)?,
        stream: decode_stream(row.get(at + 14)?),
    })
}

fn series_row(row: &Row<'_>) -> rusqlite::Result<Series> {
    Ok(Series {
        id: row.get(0)?,
        source_id: row.get(1)?,
        title: row.get(2)?,
        year: row.get(3)?,
        poster_url: row.get(4)?,
        backdrop_url: row.get(5)?,
        category_id: row.get(6)?,
        rating: row.get(7)?,
        added_at: row.get(8)?,
        plot: row.get(9)?,
        genre: row.get(10)?,
        tmdb_id: row.get(11)?,
        imdb_id: row.get(12)?,
        remote_id: row.get(13)?,
    })
}

fn episode_row(row: &Row<'_>) -> rusqlite::Result<Episode> {
    Ok(Episode {
        id: row.get(0)?,
        series_id: row.get(1)?,
        source_id: row.get(2)?,
        season: row.get(3)?,
        number: row.get(4)?,
        title: row.get(5)?,
        plot: row.get(6)?,
        duration_seconds: row.get(7)?,
        still_url: row.get(8)?,
        stream: decode_stream(row.get(9)?),
        air_date: row.get(10)?,
    })
}

fn program_row(row: &Row<'_>) -> rusqlite::Result<Program> {
    let source_id: String = row.get(0)?;
    let epg_id: String = row.get(1)?;
    let start: Timestamp = row.get(2)?;
    let category: Option<String> = row.get(7)?;
    Ok(Program {
        id: Program::make_id(&source_id, &epg_id, start),
        source_id,
        epg_id,
        start,
        end: row.get(3)?,
        title: row.get(4)?,
        subtitle: row.get(5)?,
        summary: row.get(6)?,
        genre: category.as_deref().and_then(genres::program_genre),
        category,
        image_url: row.get(8)?,
        episode: row.get(9)?,
        flags: bits_to_flags(row.get(10)?),
        age_rating: row.get(11)?,
        star_rating: row.get(12)?,
        year: row.get(13)?,
    })
}

/// The key that groups copies of one title ("EN - Paper Boats 4K" and "Paper Boats").
pub(crate) fn title_key(title: &str) -> String {
    names::match_key(&names::media_title(title))
}

/// An FTS5 prefix query: every word must match the start of a word.
fn fts_query(text: &str) -> Option<String> {
    let tokens: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| format!("\"{token}\"*"))
        .collect();
    (!tokens.is_empty()).then(|| tokens.join(" "))
}

/// `text` for a `LIKE '%…%' ESCAPE '\'` pattern.
fn like_escaped(text: &str) -> String {
    text.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// Rows in the order their ids were given, once each.
fn in_given_order<T>(ids: &[String], rows: Vec<T>, id: impl Fn(&T) -> &str) -> Vec<T> {
    let mut by_id: HashMap<String, T> = rows.into_iter().map(|row| (id(&row).to_string(), row)).collect();
    ids.iter().filter_map(|wanted| by_id.remove(wanted)).collect()
}

impl CatalogStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let writer = open_connection(path)?;
        let version: i64 = writer.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version < SCHEMA_VERSION {
            // A new catalog (version 0) is made whole by the schema alone.
            for (to, change) in MIGRATIONS {
                if version > 0 && version < *to {
                    writer.execute_batch(change)?;
                }
            }
            writer.execute_batch(SCHEMA)?;
            writer.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        let readers = (0..3).map(|_| open_connection(path).map(Mutex::new)).collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self { path: path.to_path_buf(), writer: Mutex::new(writer), readers })
    }

    /// Runs a query on whichever reader connection is free.
    fn read<T>(&self, query: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> anyhow::Result<T> {
        for reader in &self.readers {
            if let Ok(connection) = reader.try_lock() {
                return Ok(query(&connection)?);
            }
        }
        Ok(query(&lock(&self.readers[0]))?)
    }

    fn write<T>(&self, change: impl FnOnce(&mut Connection) -> anyhow::Result<T>) -> anyhow::Result<T> {
        change(&mut lock(&self.writer))
    }

    // MARK: Writing

    /// Atomically replaces a source's catalog. Called only once the complete
    /// snapshot has been fetched and parsed, so a failure leaves the old
    /// catalog intact.
    pub fn replace_catalog(
        &self,
        source_id: &str,
        snapshot: &CatalogSnapshot,
        preserve_remote_episodes: bool,
        links: &GuideLinks,
    ) -> anyhow::Result<()> {
        // Kinds the provider couldn't send this time keep their previous rows.
        let keep_movies = snapshot.kept_kinds.contains(&KeptKind::Movies);
        let keep_series = snapshot.kept_kinds.contains(&KeptKind::Series);
        let keep_channels = snapshot.kept_kinds.contains(&KeptKind::Channels);
        let kept_categories: Vec<CategoryKind> = snapshot.kept_kinds.iter().map(|kind| kind.category_kind()).collect();
        let replaced_categories: Vec<&str> = [CategoryKind::Live, CategoryKind::Movie, CategoryKind::Series]
            .into_iter()
            .filter(|kind| !kept_categories.contains(kind))
            .map(CategoryKind::as_str)
            .collect();
        let mut replaced_search = Vec::new();
        if !keep_channels {
            replaced_search.push(SearchKind::Channel.as_str());
        }
        if !keep_movies {
            replaced_search.push(SearchKind::Movie.as_str());
        }
        if !keep_series {
            replaced_search.push(SearchKind::Series.as_str());
        }

        self.write(|connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                "DELETE FROM categories WHERE source_id = ?1 AND kind IN (SELECT value FROM json_each(?2))",
                params![source_id, json_list(&replaced_categories)],
            )?;
            if !keep_channels {
                tx.execute("DELETE FROM channels WHERE source_id = ?1", [source_id])?;
            }
            tx.execute("DELETE FROM stream_options WHERE source_id = ?1", [source_id])?;
            if !keep_movies {
                tx.execute("DELETE FROM movies WHERE source_id = ?1", [source_id])?;
            }
            if !keep_series {
                tx.execute("DELETE FROM series WHERE source_id = ?1", [source_id])?;
                // Xtream episodes are fetched as series are opened; a catalog
                // refresh doesn't carry them, so they're kept for series that remain.
                if !preserve_remote_episodes || !snapshot.episodes.is_empty() {
                    tx.execute("DELETE FROM episodes WHERE source_id = ?1", [source_id])?;
                }
            }
            tx.execute(
                "DELETE FROM search WHERE source_id = ?1 AND kind IN (SELECT value FROM json_each(?2))",
                params![source_id, json_list(&replaced_search)],
            )?;

            {
                let mut category = tx.prepare_cached(
                    "INSERT OR REPLACE INTO categories (id, source_id, kind, name, sort_index) VALUES (?1, ?2, ?3, ?4, ?5)",
                )?;
                for item in &snapshot.categories {
                    category.execute(params![item.id, source_id, item.kind.as_str(), item.name, item.sort_index])?;
                }
                let mut search =
                    tx.prepare_cached("INSERT INTO search (title, item_id, kind, source_id) VALUES (?1, ?2, ?3, ?4)")?;
                let mut channel = tx.prepare_cached(
                    "INSERT OR REPLACE INTO channels (id, source_id, name, name_key, number, logo, category_id, epg_id,
                        stream, catchup_days, catchup_source, sort_index)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                )?;
                for item in &snapshot.channels {
                    channel.execute(params![
                        item.id,
                        source_id,
                        item.name,
                        names::match_key(&item.name),
                        item.number,
                        item.logo_url,
                        item.category_id,
                        item.epg_id,
                        encode_stream(&item.stream),
                        item.catchup_days,
                        item.catchup_source,
                        item.sort_index
                    ])?;
                    search.execute(params![names::display_name(&item.name), item.id, SearchKind::Channel.as_str(), source_id])?;
                }
                let mut movie = tx.prepare_cached(
                    "INSERT OR REPLACE INTO movies (id, source_id, title, title_key, year, poster, backdrop, category_id,
                        rating, added_at, plot, genre, tmdb_id, imdb_id, duration, stream)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                )?;
                for item in &snapshot.movies {
                    movie.execute(params![
                        item.id,
                        source_id,
                        item.title,
                        title_key(&item.title),
                        item.year,
                        item.poster_url,
                        item.backdrop_url,
                        item.category_id,
                        item.rating,
                        item.added_at,
                        item.plot,
                        item.genre,
                        item.tmdb_id,
                        item.imdb_id,
                        item.duration_seconds,
                        encode_stream(&item.stream)
                    ])?;
                    search.execute(params![names::media_title(&item.title), item.id, SearchKind::Movie.as_str(), source_id])?;
                }
                let mut series = tx.prepare_cached(
                    "INSERT OR REPLACE INTO series (id, source_id, title, title_key, year, poster, backdrop, category_id,
                        rating, added_at, plot, genre, tmdb_id, imdb_id, remote_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                )?;
                for item in &snapshot.series {
                    series.execute(params![
                        item.id,
                        source_id,
                        item.title,
                        title_key(&item.title),
                        item.year,
                        item.poster_url,
                        item.backdrop_url,
                        item.category_id,
                        item.rating,
                        item.added_at,
                        item.plot,
                        item.genre,
                        item.tmdb_id,
                        item.imdb_id,
                        item.remote_id
                    ])?;
                    search.execute(params![names::media_title(&item.title), item.id, SearchKind::Series.as_str(), source_id])?;
                }
                let mut options = tx.prepare_cached(
                    "INSERT OR REPLACE INTO stream_options (source_id, url, user_agent, referrer) VALUES (?1, ?2, ?3, ?4)",
                )?;
                for item in &snapshot.stream_options {
                    options.execute(params![source_id, item.url, item.user_agent, item.referrer])?;
                }
            }
            insert_episodes(&tx, &snapshot.episodes)?;
            if !keep_series {
                // Episodes of series the provider no longer lists.
                tx.execute(
                    "DELETE FROM episodes WHERE source_id = ?1 AND series_id NOT IN (SELECT id FROM series WHERE source_id = ?1)",
                    [source_id],
                )?;
            }
            merge_details(&tx, source_id, &snapshot.details, None)?;
            resolve_guide_links(&tx, Some(source_id), links)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Replaces one series' episodes (fetched when it's opened).
    pub fn replace_episodes(&self, series_id: &str, episodes: &[Episode]) -> anyhow::Result<()> {
        self.write(|connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("DELETE FROM episodes WHERE series_id = ?1", [series_id])?;
            insert_episodes(&tx, episodes)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Merges details into what's stored. Existing values are kept where the
    /// new details have none. `fetched_at` marks details from a provider's
    /// detail endpoint, so each title is asked for once.
    pub fn save_details(
        &self,
        source_id: &str,
        details: &BTreeMap<String, DetailsRecord>,
        fetched_at: Option<Timestamp>,
    ) -> anyhow::Result<()> {
        self.write(|connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            merge_details(&tx, source_id, details, fetched_at)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Removes a source's details that came with its lists rather than from
    /// a detail endpoint, before a catalog that brings them all again.
    pub fn forget_listed_details(&self, source_id: &str) -> anyhow::Result<()> {
        self.write(|connection| {
            connection.execute("DELETE FROM media_details WHERE source_id = ?1 AND fetched_at IS NULL", [source_id])?;
            Ok(())
        })
    }

    /// Fills in what a film's detail record adds, without replacing what the
    /// list already said (except a missing year).
    pub fn update_movie(
        &self,
        id: &str,
        plot: Option<&str>,
        genre: Option<&str>,
        duration_seconds: Option<i64>,
        year: Option<i64>,
    ) -> anyhow::Result<()> {
        self.write(|connection| {
            connection.execute(
                "UPDATE movies SET plot = COALESCE(plot, ?1), genre = COALESCE(genre, ?2), duration = COALESCE(?3, duration),
                    year = COALESCE(year, ?4) WHERE id = ?5",
                params![plot, genre, duration_seconds, year, id],
            )?;
            Ok(())
        })
    }

    pub fn remove_source(&self, source_id: &str) -> anyhow::Result<()> {
        self.write(|connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for table in [
                "categories",
                "channels",
                "movies",
                "series",
                "episodes",
                "programs",
                "epg_channels",
                "stream_options",
                "source_status",
                "search",
                "media_details",
                "tmdb_posters",
                "file_facts",
            ] {
                tx.execute(&format!("DELETE FROM {table} WHERE source_id = ?1"), [source_id])?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn save_status(&self, source_id: &str, status: &SourceStatus) -> anyhow::Result<()> {
        let text = serde_json::to_string(status)?;
        self.write(|connection| {
            connection.execute(
                "INSERT OR REPLACE INTO source_status (source_id, json) VALUES (?1, ?2)",
                params![source_id, text],
            )?;
            Ok(())
        })
    }

    /// Drops guide listings that ended before `before`, to keep the database bounded.
    pub fn prune_guide(&self, before: Timestamp) -> anyhow::Result<()> {
        self.write(|connection| {
            connection.execute("DELETE FROM programs WHERE end < ?1", [before])?;
            Ok(())
        })
    }

    /// Drops every guide listing. Guide channel names stay, so channels keep
    /// the guide channels they show and the next guide load fills them again.
    pub fn clear_guide(&self) -> anyhow::Result<()> {
        self.write(|connection| {
            connection.execute("DELETE FROM programs", [])?;
            Ok(())
        })
    }

    /// Rebuilds the database without its free pages, so space freed by
    /// deleting goes back to the disk (SQLite otherwise keeps it for reuse).
    /// In WAL mode the smaller database only reaches the file at a
    /// checkpoint, which truncates the file and the log. Slow for a big
    /// catalog, and it holds the writer meanwhile.
    pub fn compact(&self) -> anyhow::Result<()> {
        self.write(|connection| {
            connection.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(())
        })
    }

    /// How many guide listings there are, across every guide.
    pub fn program_count(&self) -> anyhow::Result<i64> {
        self.read(|db| db.query_row("SELECT COUNT(*) FROM programs", [], |row| row.get(0)))
    }

    /// Streams a guide import into the catalog inside one transaction on its
    /// own connection. The previous guide stays visible to readers until the
    /// new one is complete; if `fill` fails, nothing changes. `guide_id` is a
    /// source's id or an added guide's. Returns how many listings the guide
    /// now has.
    pub fn import_guide(
        &self,
        guide_id: &str,
        links: &GuideLinks,
        fill: impl FnOnce(&mut GuideImport<'_>) -> anyhow::Result<()>,
    ) -> anyhow::Result<i64> {
        let mut connection = open_connection(&self.path)?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM programs WHERE source_id = ?1", [guide_id])?;
        tx.execute("DELETE FROM epg_channels WHERE source_id = ?1", [guide_id])?;
        let mut import = GuideImport { tx, source_id: guide_id.to_string() };
        fill(&mut import)?;
        let GuideImport { tx, .. } = import;
        // A source's guide serves its own channels; an added guide, every channel.
        let scope = (!links.guides.iter().any(|id| id == guide_id)).then_some(guide_id);
        resolve_guide_links(&tx, scope, links)?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM programs WHERE source_id = ?1", [guide_id], |row| row.get(0))?;
        tx.commit()?;
        Ok(count)
    }

    /// Links channels to guides again after the added guides or the guide
    /// channels chosen by hand change: one source's channels, or all of them.
    pub fn relink_guides(&self, scope: Option<&str>, links: &GuideLinks) -> anyhow::Result<()> {
        self.write(|connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            resolve_guide_links(&tx, scope, links)?;
            tx.commit()?;
            Ok(())
        })
    }

    // MARK: Status

    pub fn statuses(&self) -> anyhow::Result<HashMap<String, SourceStatus>> {
        let rows: Vec<(String, String)> = self.read(|db| {
            let mut statement = db.prepare_cached("SELECT source_id, json FROM source_status")?;
            statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
        })?;
        Ok(rows.into_iter().filter_map(|(id, text)| Some((id, serde_json::from_str(&text).ok()?))).collect())
    }

    pub fn status(&self, source_id: &str) -> anyhow::Result<Option<SourceStatus>> {
        let text: Option<String> = self.read(|db| {
            db.query_row("SELECT json FROM source_status WHERE source_id = ?1", [source_id], |row| row.get(0)).optional()
        })?;
        Ok(text.and_then(|text| serde_json::from_str(&text).ok()))
    }

    pub fn counts(&self, source_id: &str) -> anyhow::Result<Counts> {
        self.read(|db| {
            let count = |table: &str| -> rusqlite::Result<i64> {
                db.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE source_id = ?1"), [source_id], |row| row.get(0))
            };
            Ok(Counts {
                channels: count("channels")?,
                movies: count("movies")?,
                series: count("series")?,
                programs: count("programs")?,
            })
        })
    }

    // MARK: Categories and channels

    /// A source's categories of one kind that hold something, in its order.
    pub fn categories(&self, kind: CategoryKind, source_id: &str) -> anyhow::Result<Vec<Category>> {
        let table = match kind {
            CategoryKind::Live => "channels",
            CategoryKind::Movie => "movies",
            CategoryKind::Series => "series",
        };
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT c.id, c.source_id, c.kind, c.name, c.sort_index FROM categories c
                 WHERE c.source_id = ?1 AND c.kind = ?2 AND EXISTS (SELECT 1 FROM {table} t WHERE t.category_id = c.id)
                 ORDER BY c.sort_index"
            ))?;
            statement.query_map(params![source_id, kind.as_str()], category_row)?.collect()
        })
    }

    /// Every category of the given sources, of every kind, empty ones too.
    pub fn all_categories(&self, source_ids: &[String]) -> anyhow::Result<Vec<Category>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(
                "SELECT id, source_id, kind, name, sort_index FROM categories
                 WHERE source_id IN (SELECT value FROM json_each(?1)) ORDER BY source_id, kind, sort_index",
            )?;
            statement.query_map([json_list(source_ids)], category_row)?.collect()
        })
    }

    /// How many channels, films or series each category of these sources holds.
    pub fn category_counts(&self, source_ids: &[String]) -> anyhow::Result<HashMap<String, i64>> {
        self.read(|db| {
            let mut counts = HashMap::new();
            for table in ["channels", "movies", "series"] {
                let mut statement = db.prepare_cached(&format!(
                    "SELECT category_id, COUNT(*) FROM {table}
                     WHERE category_id IS NOT NULL AND source_id IN (SELECT value FROM json_each(?1)) GROUP BY category_id"
                ))?;
                for row in statement.query_map([json_list(source_ids)], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))? {
                    let (category, count) = row?;
                    *counts.entry(category).or_insert(0) += count;
                }
            }
            Ok(counts)
        })
    }

    /// One source's channels in its own order, leaving out the channels and
    /// categories in `excluded`. No limit by default: large providers carry
    /// tens of thousands, and every one must be reachable.
    pub fn channels(
        &self,
        source_id: &str,
        category_id: Option<&str>,
        limit: Option<usize>,
        excluded: &[String],
    ) -> anyhow::Result<Vec<Channel>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {CHANNEL_COLUMNS} FROM channels WHERE source_id = ?1 AND (?2 IS NULL OR category_id = ?2)
                   AND id NOT IN (SELECT value FROM json_each(?4))
                   AND (category_id IS NULL OR category_id NOT IN (SELECT value FROM json_each(?4)))
                 ORDER BY sort_index LIMIT ?3"
            ))?;
            statement.query_map(params![source_id, category_id, sql_limit(limit), json_list(excluded)], channel_row)?.collect()
        })
    }

    /// Channels by id, in the order asked for; an id listed twice comes back once.
    pub fn channels_by_ids(&self, ids: &[String]) -> anyhow::Result<Vec<Channel>> {
        let rows = self.read(|db| {
            let mut statement =
                db.prepare_cached(&format!("SELECT {CHANNEL_COLUMNS} FROM channels WHERE id IN (SELECT value FROM json_each(?1))"))?;
            statement.query_map([json_list(ids)], channel_row)?.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        Ok(in_given_order(ids, rows, |channel| &channel.id))
    }

    pub fn channel(&self, id: &str) -> anyhow::Result<Option<Channel>> {
        Ok(self.channels_by_ids(&[id.to_string()])?.pop())
    }

    /// The first channel (by sort order) showing a guide channel.
    pub fn channel_for_guide(&self, guide_id: &str, epg_id: &str) -> anyhow::Result<Option<Channel>> {
        self.read(|db| {
            db.query_row(
                &format!(
                    "SELECT {CHANNEL_COLUMNS} FROM channels
                     WHERE COALESCE(guide_source, source_id) = ?1 AND COALESCE(epg_fallback, epg_id) = ?2
                     ORDER BY sort_index LIMIT 1"
                ),
                params![guide_id, epg_id],
                channel_row,
            )
            .optional()
        })
    }

    /// The guide and guide channel a channel shows.
    pub fn guide_link(&self, channel_id: &str) -> anyhow::Result<Option<(String, String)>> {
        let link: Option<(String, Option<String>)> = self.read(|db| {
            db.query_row(
                "SELECT COALESCE(guide_source, source_id), COALESCE(epg_fallback, epg_id) FROM channels WHERE id = ?1",
                [channel_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
        })?;
        Ok(link.and_then(|(guide, epg_id)| Some((guide, epg_id?))))
    }

    /// The request headers a playlist asked for when playing `url`.
    pub fn stream_options(&self, source_id: &str, url: &str) -> anyhow::Result<Option<(Option<String>, Option<String>)>> {
        self.read(|db| {
            db.query_row(
                "SELECT user_agent, referrer FROM stream_options WHERE source_id = ?1 AND url = ?2",
                params![source_id, url],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
        })
    }

    // MARK: Films and series

    pub fn movies(&self, browse: &Browse<'_>) -> anyhow::Result<Vec<Movie>> {
        self.browse("movies", MOVIE_COLUMNS, browse, movie_row)
    }

    pub fn series_list(&self, browse: &Browse<'_>) -> anyhow::Result<Vec<Series>> {
        self.browse("series", SERIES_COLUMNS, browse, series_row)
    }

    fn browse<T>(
        &self,
        table: &str,
        columns: &str,
        browse: &Browse<'_>,
        map: fn(&Row<'_>) -> rusqlite::Result<T>,
    ) -> anyhow::Result<Vec<T>> {
        let genre_words: Vec<String> = browse.genre_words.iter().map(|word| like_escaped(word)).collect();
        let by_genre = !genre_words.is_empty() || !browse.genre_categories.is_empty();
        // Only films have versions.
        let grouped = browse.group_versions && table == "movies";
        let prefixed: Vec<String> = columns.split(", ").map(|column| format!("t.{column}")).collect();
        // What's been watched is asked of a copy, and with versions grouped
        // stands for every copy of the film.
        let watched = if grouped {
            let (t, w) = (version_key("t"), version_key("w"));
            format!(
                "(?10 IS NULL OR {t} IN (SELECT {w} FROM {table} w WHERE w.id IN (SELECT value FROM json_each(?10))))
                 AND {t} NOT IN (SELECT {w} FROM {table} w WHERE w.id IN (SELECT value FROM json_each(?11)))"
            )
        } else {
            "(?10 IS NULL OR t.id IN (SELECT value FROM json_each(?10))) AND t.id NOT IN (SELECT value FROM json_each(?11))".to_string()
        };
        // A copy shows unless another copy of the film that the list would
        // show comes before it. The index on the version key keeps this to
        // a lookup or two per title.
        let first_copy = if grouped {
            format!(
                "AND NOT EXISTS (SELECT 1 FROM {table} o WHERE {} = {} AND o.id <> t.id AND {} AND {})",
                version_key("o"),
                version_key("t"),
                browse.order.precedes("o", "t"),
                browse_conditions("o")
            )
        } else {
            String::new()
        };
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {} FROM {table} t
                 WHERE {} AND {watched} {first_copy}
                 ORDER BY {} LIMIT ?4 OFFSET ?5",
                prefixed.join(", "),
                browse_conditions("t"),
                browse.order.sql("t")
            ))?;
            statement
                .query_map(
                    params![
                        json_list(browse.source_ids),
                        browse.category_id,
                        by_genre,
                        sql_limit(Some(browse.limit)),
                        sql_limit(Some(browse.offset)).max(0),
                        json_list(browse.excluded),
                        json_list(&genre_words),
                        json_list(browse.genre_categories),
                        json_list(browse.title_tags),
                        browse.only.map(json_list),
                        json_list(browse.without),
                        browse.years.map(|(first, _)| first),
                        browse.years.map(|(_, last)| last)
                    ],
                    map,
                )?
                .collect()
        })
    }

    /// Every copy of these films in these sources, as (the film asked about,
    /// a copy), each film among its own copies. Copies kept out (hidden,
    /// locked, left out by the profile) are passed over.
    pub fn movie_copies(
        &self,
        ids: &[String],
        source_ids: &[String],
        excluded: &[String],
        title_tags: &[String],
    ) -> anyhow::Result<Vec<(String, Movie)>> {
        let prefixed: Vec<String> = MOVIE_COLUMNS.split(", ").map(|column| format!("v.{column}")).collect();
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT p.id, {} FROM movies p JOIN movies v ON {} = {}
                 WHERE p.id IN (SELECT value FROM json_each(?1))
                   AND (v.id = p.id
                        OR (v.source_id IN (SELECT value FROM json_each(?2))
                            AND v.id NOT IN (SELECT value FROM json_each(?3))
                            AND (v.category_id IS NULL OR v.category_id NOT IN (SELECT value FROM json_each(?3)))
                            AND {}))",
                prefixed.join(", "),
                version_key("v"),
                version_key("p"),
                untagged("v", "?4")
            ))?;
            statement
                .query_map(params![json_list(ids), json_list(source_ids), json_list(excluded), json_list(title_tags)], |row| {
                    Ok((row.get(0)?, movie_at(row, 1)?))
                })?
                .collect()
        })
    }

    /// Categories by id, in no particular order.
    pub fn categories_by_ids(&self, ids: &[String]) -> anyhow::Result<Vec<Category>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(
                "SELECT id, source_id, kind, name, sort_index FROM categories WHERE id IN (SELECT value FROM json_each(?1))",
            )?;
            statement.query_map([json_list(ids)], category_row)?.collect()
        })
    }

    /// How many episodes, specials aside, each of these series has in the
    /// catalog, and how many of those are in `finished` (episode ids).
    pub fn episodes_finished(&self, series_ids: &[String], finished: &[String]) -> anyhow::Result<HashMap<String, (i64, i64)>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(
                "SELECT series_id, COUNT(*), SUM(id IN (SELECT value FROM json_each(?2))) FROM episodes
                 WHERE series_id IN (SELECT value FROM json_each(?1)) AND season > 0 GROUP BY series_id",
            )?;
            statement
                .query_map([json_list(series_ids), json_list(finished)], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))?
                .collect()
        })
    }

    // MARK: Posters and people from elsewhere

    /// Posters found on TMDB for these titles (found ones only).
    pub fn filled_posters(&self, ids: &[String]) -> anyhow::Result<HashMap<String, String>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(
                "SELECT item_id, poster FROM tmdb_posters WHERE poster IS NOT NULL AND item_id IN (SELECT value FROM json_each(?1))",
            )?;
            statement.query_map([json_list(ids)], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
        })
    }

    /// What looking for a title's poster found (no poster: TMDB had none), and when.
    pub fn poster_lookup(&self, item_id: &str) -> anyhow::Result<Option<(Option<String>, Timestamp)>> {
        self.read(|db| {
            db.query_row("SELECT poster, looked_up_at FROM tmdb_posters WHERE item_id = ?1", [item_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()
        })
    }

    pub fn save_poster_lookup(&self, item_id: &str, source_id: &str, poster: Option<&str>, looked_up_at: Timestamp) -> anyhow::Result<()> {
        self.write(|connection| {
            connection.execute(
                "INSERT OR REPLACE INTO tmdb_posters (item_id, source_id, poster, looked_up_at) VALUES (?1, ?2, ?3, ?4)",
                params![item_id, source_id, poster, looked_up_at],
            )?;
            Ok(())
        })
    }

    /// Forgets the posters TMDB was asked for by name alone for a media
    /// folder's titles with no year and no ids (a music video taken for a
    /// film of that name): frames of their videos are their pictures instead.
    pub fn forget_guessed_posters(&self, source_id: &str) -> anyhow::Result<()> {
        self.write(|connection| {
            connection.execute(
                "DELETE FROM tmdb_posters WHERE source_id = ?1 AND poster LIKE 'https://image.tmdb.org/%' AND item_id IN \
                 (SELECT id FROM movies WHERE source_id = ?1 AND year IS NULL AND COALESCE(tmdb_id, '') = '' AND COALESCE(imdb_id, '') = '')",
                [source_id],
            )?;
            Ok(())
        })
    }

    /// Who Wikidata had by this name, as kept, and when it was asked.
    pub fn wikidata_people(&self, name_key: &str) -> anyhow::Result<Option<(String, Timestamp)>> {
        self.read(|db| {
            db.query_row("SELECT json, fetched_at FROM wikidata_people WHERE name_key = ?1", [name_key], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()
        })
    }

    pub fn save_wikidata_people(&self, name_key: &str, json: &str, fetched_at: Timestamp) -> anyhow::Result<()> {
        self.write(|connection| {
            connection.execute(
                "INSERT OR REPLACE INTO wikidata_people (name_key, json, fetched_at) VALUES (?1, ?2, ?3)",
                params![name_key, json, fetched_at],
            )?;
            Ok(())
        })
    }

    pub fn movies_by_ids(&self, ids: &[String]) -> anyhow::Result<Vec<Movie>> {
        let rows = self.read(|db| {
            let mut statement =
                db.prepare_cached(&format!("SELECT {MOVIE_COLUMNS} FROM movies WHERE id IN (SELECT value FROM json_each(?1))"))?;
            statement.query_map([json_list(ids)], movie_row)?.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        Ok(in_given_order(ids, rows, |movie| &movie.id))
    }

    pub fn series_by_ids(&self, ids: &[String]) -> anyhow::Result<Vec<Series>> {
        let rows = self.read(|db| {
            let mut statement =
                db.prepare_cached(&format!("SELECT {SERIES_COLUMNS} FROM series WHERE id IN (SELECT value FROM json_each(?1))"))?;
            statement.query_map([json_list(ids)], series_row)?.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        Ok(in_given_order(ids, rows, |series| &series.id))
    }

    pub fn episodes(&self, series_id: &str) -> anyhow::Result<Vec<Episode>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {EPISODE_COLUMNS} FROM episodes WHERE series_id = ?1 ORDER BY season, number"
            ))?;
            statement.query_map([series_id], episode_row)?.collect()
        })
    }

    /// Series of these sources with a regular episode that aired between
    /// `from` and `to`, the latest airing first. Only episodes the catalog
    /// holds count: a provider's are there once its series was opened.
    pub fn series_with_episodes_aired(&self, source_ids: &[String], from: Timestamp, to: Timestamp, limit: usize) -> anyhow::Result<Vec<String>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(
                "SELECT series_id FROM episodes
                 WHERE source_id IN (SELECT value FROM json_each(?1)) AND season > 0 AND air_date BETWEEN ?2 AND ?3
                 GROUP BY series_id ORDER BY MAX(air_date) DESC LIMIT ?4",
            )?;
            statement.query_map(params![json_list(source_ids), from, to, sql_limit(Some(limit))], |row| row.get(0))?.collect()
        })
    }

    pub fn episodes_by_ids(&self, ids: &[String]) -> anyhow::Result<Vec<Episode>> {
        let rows = self.read(|db| {
            let mut statement =
                db.prepare_cached(&format!("SELECT {EPISODE_COLUMNS} FROM episodes WHERE id IN (SELECT value FROM json_each(?1))"))?;
            statement.query_map([json_list(ids)], episode_row)?.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        Ok(in_given_order(ids, rows, |episode| &episode.id))
    }

    /// Films in these sources with one of these IMDb or TMDB ids or title
    /// keys ([`title_key`]): the catalog's side of matching titles to Trakt's.
    pub fn movies_matching(&self, source_ids: &[String], keys: &MatchKeys) -> anyhow::Result<Vec<Movie>> {
        self.matching("movies", MOVIE_COLUMNS, source_ids, keys, movie_row)
    }

    /// Series in these sources with one of these ids or title keys.
    pub fn series_matching(&self, source_ids: &[String], keys: &MatchKeys) -> anyhow::Result<Vec<Series>> {
        self.matching("series", SERIES_COLUMNS, source_ids, keys, series_row)
    }

    fn matching<T>(
        &self,
        table: &str,
        columns: &str,
        source_ids: &[String],
        keys: &MatchKeys,
        map: fn(&Row<'_>) -> rusqlite::Result<T>,
    ) -> anyhow::Result<Vec<T>> {
        if keys.imdb.is_empty() && keys.tmdb.is_empty() && keys.title_keys.is_empty() {
            return Ok(Vec::new());
        }
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {columns} FROM {table}
                 WHERE source_id IN (SELECT value FROM json_each(?1))
                   AND (imdb_id IN (SELECT value FROM json_each(?2)) OR tmdb_id IN (SELECT value FROM json_each(?3))
                        OR title_key IN (SELECT value FROM json_each(?4)))"
            ))?;
            statement
                .query_map(
                    params![json_list(source_ids), json_list(&keys.imdb), json_list(&keys.tmdb), json_list(&keys.title_keys)],
                    map,
                )?
                .collect()
        })
    }

    /// Every episode of these series, in season and episode order.
    pub fn episodes_of_series(&self, series_ids: &[String]) -> anyhow::Result<Vec<Episode>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {EPISODE_COLUMNS} FROM episodes WHERE series_id IN (SELECT value FROM json_each(?1))
                 ORDER BY series_id, season, number"
            ))?;
            statement.query_map([json_list(series_ids)], episode_row)?.collect()
        })
    }

    /// Genres as the sources write them, split where they list several
    /// ("Drama, Thriller"), of titles outside the `excluded` categories.
    pub fn genres(&self, kind: CategoryKind, source_ids: &[String], excluded: &[String]) -> anyhow::Result<Vec<String>> {
        let table = match kind {
            CategoryKind::Movie => "movies",
            CategoryKind::Series => "series",
            CategoryKind::Live => return Ok(Vec::new()),
        };
        let raw: Vec<String> = self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT DISTINCT genre FROM {table} WHERE genre IS NOT NULL AND source_id IN (SELECT value FROM json_each(?1))
                   AND (category_id IS NULL OR category_id NOT IN (SELECT value FROM json_each(?2)))"
            ))?;
            statement.query_map([json_list(source_ids), json_list(excluded)], |row| row.get(0))?.collect()
        })?;
        let mut seen = HashSet::new();
        let mut genres: Vec<String> = raw
            .iter()
            .flat_map(|text| text.split([',', '/']))
            .map(|genre| genre.trim().to_string())
            .filter(|genre| !genre.is_empty() && seen.insert(genre.to_lowercase()))
            .collect();
        genres.sort_by_key(|genre| genre.to_lowercase());
        Ok(genres)
    }

    /// Titles ranked by how many copies the sources carry (categories,
    /// qualities, services), then rating. Providers duplicate what people want
    /// to watch, so this is a free, honest stand-in for popularity.
    pub fn popular_movies(
        &self,
        source_ids: &[String],
        minimum_year: Option<i64>,
        limit: usize,
        excluded_categories: &[String],
        title_tags: &[String],
    ) -> anyhow::Result<Vec<Movie>> {
        self.popular("movies", MOVIE_COLUMNS, source_ids, minimum_year, limit, excluded_categories, title_tags, movie_row)
    }

    pub fn popular_series(
        &self,
        source_ids: &[String],
        minimum_year: Option<i64>,
        limit: usize,
        excluded_categories: &[String],
        title_tags: &[String],
    ) -> anyhow::Result<Vec<Series>> {
        self.popular("series", SERIES_COLUMNS, source_ids, minimum_year, limit, excluded_categories, title_tags, series_row)
    }

    #[allow(clippy::too_many_arguments)]
    fn popular<T>(
        &self,
        table: &str,
        columns: &str,
        source_ids: &[String],
        minimum_year: Option<i64>,
        limit: usize,
        excluded_categories: &[String],
        title_tags: &[String],
        map: fn(&Row<'_>) -> rusqlite::Result<T>,
    ) -> anyhow::Result<Vec<T>> {
        let prefixed: Vec<String> = columns.split(", ").map(|column| format!("t.{column}")).collect();
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "WITH ranked AS (
                    SELECT MIN(id) AS id, COUNT(*) AS copies, MAX(COALESCE(rating, 0)) AS best FROM {table}
                    WHERE source_id IN (SELECT value FROM json_each(?1))
                      AND (?2 IS NULL OR year >= ?2)
                      AND poster IS NOT NULL
                      AND (category_id IS NULL OR category_id NOT IN (SELECT value FROM json_each(?4)))
                      AND id NOT IN (SELECT value FROM json_each(?4))
                      AND {}
                    GROUP BY COALESCE(tmdb_id, title_key)
                    ORDER BY copies DESC, best DESC
                    LIMIT ?3
                 )
                 SELECT {} FROM ranked r JOIN {table} t ON t.id = r.id ORDER BY r.copies DESC, r.best DESC",
                untagged(table, "?5"),
                prefixed.join(", ")
            ))?;
            statement
                .query_map(
                    params![
                        json_list(source_ids),
                        minimum_year,
                        sql_limit(Some(limit)),
                        json_list(excluded_categories),
                        json_list(title_tags)
                    ],
                    map,
                )?
                .collect()
        })
    }

    // MARK: Search

    /// Item ids of one kind whose title has words starting with each word of
    /// `text`, leaving out ids in `excluded` and items in categories in it,
    /// and titles tagged with a country in `title_tags`, before the limit
    /// is counted.
    pub fn search(
        &self,
        text: &str,
        kind: SearchKind,
        source_ids: &[String],
        limit: usize,
        excluded: &[String],
        title_tags: &[String],
    ) -> anyhow::Result<Vec<String>> {
        let Some(query) = fts_query(text) else { return Ok(Vec::new()) };
        let table = match kind {
            SearchKind::Channel => "channels",
            SearchKind::Movie => "movies",
            SearchKind::Series => "series",
        };
        // Channels have names, not titles; their tags are quality, not country.
        let tags: &[String] = if kind == SearchKind::Channel { &[] } else { title_tags };
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT item_id FROM search
                 WHERE search MATCH ?1 AND kind = ?2 AND source_id IN (SELECT value FROM json_each(?3))
                   AND item_id NOT IN (SELECT value FROM json_each(?5))
                   AND NOT EXISTS (
                       SELECT 1 FROM {table} t WHERE t.id = search.item_id
                          AND t.category_id IN (SELECT value FROM json_each(?5)))
                   AND {}
                 ORDER BY rank LIMIT ?4",
                untagged("search", "?6")
            ))?;
            statement
                .query_map(
                    params![query, kind.as_str(), json_list(source_ids), sql_limit(Some(limit)), json_list(excluded), json_list(tags)],
                    |row| row.get(0),
                )?
                .collect()
        })
    }

    /// Guide programmes ending after `after` whose title contains `text`,
    /// with the channel of `source_ids` showing each (passing over channels
    /// and categories in `excluded`). Those not over at `now` come first,
    /// soonest first, then those that have ended, latest first.
    pub fn search_programs(
        &self,
        text: &str,
        source_ids: &[String],
        after: Timestamp,
        now: Timestamp,
        limit: usize,
        excluded: &[String],
    ) -> anyhow::Result<Vec<(Program, Channel)>> {
        let trimmed = text.trim();
        if trimmed.chars().count() < 2 {
            return Ok(Vec::new());
        }
        let pattern = like_escaped(trimmed);
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {PROGRAM_COLUMNS}, {} FROM programs p {}
                 WHERE p.title LIKE '%' || ?1 || '%' ESCAPE '\\' AND p.end > ?2
                 ORDER BY p.end <= ?5, CASE WHEN p.end <= ?5 THEN -p.start ELSE p.start END, c.sort_index
                 LIMIT ?4",
                joined_channel_columns(),
                showing_channel("?3", "?6")
            ))?;
            statement
                .query_map(
                    params![pattern, after, json_list(source_ids), sql_limit(Some(limit)), now, json_list(excluded)],
                    |row| Ok((program_row(row)?, channel_at(row, PROGRAM_COLUMN_COUNT)?)),
                )?
                .collect()
        })
    }

    /// Programmes airing at `now` on the channels of `source_ids` (passing
    /// over channels and categories in `excluded`), one per channel, those
    /// with artwork and flagged live, new or premiere first, then in channel
    /// order. Only programmes ending within twelve hours are looked at,
    /// which keeps a large guide quick.
    pub fn airing(&self, source_ids: &[String], now: Timestamp, limit: usize, excluded: &[String]) -> anyhow::Result<Vec<(Program, Channel)>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {PROGRAM_COLUMNS}, {} FROM programs p {}
                 WHERE p.end > ?1 AND p.end <= ?1 + 43200 AND p.start <= ?1
                 GROUP BY c.id
                 ORDER BY p.image IS NULL, (p.flags & {flags}) = 0, c.sort_index
                 LIMIT ?3",
                joined_channel_columns(),
                showing_channel("?2", "?4"),
                flags = flag::LIVE | flag::NEW | flag::PREMIERE,
            ))?;
            statement
                .query_map(params![now, json_list(source_ids), sql_limit(Some(limit)), json_list(excluded)], |row| {
                    Ok((program_row(row)?, channel_at(row, PROGRAM_COLUMN_COUNT)?))
                })?
                .collect()
        })
    }

    /// Programmes starting between `from` and `to` on any channel, the ones
    /// that look like events people plan for first: films, sport, live
    /// broadcasts, premieres and new episodes, and listings with artwork.
    /// News and paid programming are left out; repeats come last. Channels
    /// and categories in `excluded` are passed over.
    pub fn upcoming_events(
        &self,
        source_ids: &[String],
        from: Timestamp,
        to: Timestamp,
        limit: usize,
        excluded: &[String],
    ) -> anyhow::Result<Vec<(Program, Channel)>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {PROGRAM_COLUMNS}, {} FROM programs p {}
                 WHERE p.start >= ?1 AND p.start < ?2
                   AND lower(COALESCE(p.category, '')) NOT LIKE '%news%'
                   AND lower(COALESCE(p.category, '')) NOT LIKE '%paid%'
                   AND lower(COALESCE(p.category, '')) NOT LIKE '%shopping%'
                   AND lower(COALESCE(p.category, '')) NOT LIKE '%infomercial%'
                   AND lower(p.title) NOT LIKE '%paid program%'
                   AND lower(p.title) NOT LIKE '%infomercial%'
                 ORDER BY (
                    CASE WHEN lower(COALESCE(p.category, '')) LIKE '%movie%' OR lower(COALESCE(p.category, '')) LIKE '%film%' THEN 3 ELSE 0 END
                  + CASE WHEN lower(COALESCE(p.category, '')) LIKE '%sport%' THEN 3 ELSE 0 END
                  + CASE WHEN p.flags & {live} != 0 THEN 3 ELSE 0 END
                  + CASE WHEN p.flags & {premiere} != 0 THEN 2 ELSE 0 END
                  + CASE WHEN p.flags & {new} != 0 THEN 1 ELSE 0 END
                  + CASE WHEN p.image IS NOT NULL THEN 1 ELSE 0 END
                  - CASE WHEN p.flags & {repeat} != 0 THEN 3 ELSE 0 END
                 ) DESC, p.start, c.sort_index
                 LIMIT ?4",
                joined_channel_columns(),
                showing_channel("?3", "?5"),
                live = flag::LIVE,
                premiere = flag::PREMIERE,
                new = flag::NEW,
                repeat = flag::REPEAT,
            ))?;
            statement
                .query_map(params![from, to, json_list(source_ids), sql_limit(Some(limit)), json_list(excluded)], |row| {
                    Ok((program_row(row)?, channel_at(row, PROGRAM_COLUMN_COUNT)?))
                })?
                .collect()
        })
    }

    // MARK: Guide

    /// The programmes a channel shows overlapping `from`…`to`, in order.
    pub fn channel_programs(&self, channel_id: &str, from: Timestamp, to: Timestamp) -> anyhow::Result<Vec<Program>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {PROGRAM_COLUMNS} FROM channels c {SHOWN_PROGRAMS}
                 WHERE c.id = ?1 AND p.end > ?2 AND p.start < ?3 ORDER BY p.start"
            ))?;
            statement.query_map(params![channel_id, from, to], program_row)?.collect()
        })
    }

    /// The next `limit` programmes on a channel that haven't ended by `now`.
    pub fn channel_upcoming(&self, channel_id: &str, now: Timestamp, limit: usize) -> anyhow::Result<Vec<Program>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {PROGRAM_COLUMNS} FROM channels c {SHOWN_PROGRAMS}
                 WHERE c.id = ?1 AND p.end > ?2 ORDER BY p.start LIMIT ?3"
            ))?;
            statement.query_map(params![channel_id, now, sql_limit(Some(limit))], program_row)?.collect()
        })
    }

    /// A guide channel's programmes overlapping `from`…`to`, in order.
    pub fn programs(&self, source_id: &str, epg_id: &str, from: Timestamp, to: Timestamp) -> anyhow::Result<Vec<Program>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT {PROGRAM_COLUMNS} FROM programs p
                 WHERE p.source_id = ?1 AND p.epg_id = ?2 AND p.end > ?3 AND p.start < ?4 ORDER BY p.start"
            ))?;
            statement.query_map(params![source_id, epg_id, from, to], program_row)?.collect()
        })
    }

    pub fn program(&self, source_id: &str, epg_id: &str, start: Timestamp) -> anyhow::Result<Option<Program>> {
        self.read(|db| {
            db.query_row(
                &format!("SELECT {PROGRAM_COLUMNS} FROM programs p WHERE p.source_id = ?1 AND p.epg_id = ?2 AND p.start = ?3"),
                params![source_id, epg_id, start],
                program_row,
            )
            .optional()
        })
    }

    /// Channels in `guide_ids`' guides whose id or display name contains
    /// `text` (all of them when it's empty), exact and leading matches first.
    pub fn guide_channels(
        &self,
        text: &str,
        guide_ids: &[String],
        now: Timestamp,
        limit: usize,
    ) -> anyhow::Result<Vec<GuideChannelRow>> {
        let text = text.trim();
        let pattern = like_escaped(text);
        let key = names::match_key(text);
        self.read(|db| {
            let mut statement = db.prepare_cached(
                "SELECT e.source_id, e.epg_id, MIN(CASE
                        WHEN ?1 = '' THEN 2
                        WHEN e.epg_id = ?1 COLLATE NOCASE OR (?2 != '' AND e.name_key = ?2) THEN 0
                        WHEN e.epg_id LIKE ?1 || '%' ESCAPE '\\' OR (?2 != '' AND e.name_key LIKE ?2 || '%') THEN 1
                        ELSE 2 END) AS score
                 FROM epg_channels e
                 WHERE e.source_id IN (SELECT value FROM json_each(?3))
                   AND (?1 = '' OR e.epg_id LIKE '%' || ?1 || '%' ESCAPE '\\' OR e.name LIKE '%' || ?1 || '%' ESCAPE '\\'
                        OR (?2 != '' AND e.name_key LIKE '%' || ?2 || '%'))
                 GROUP BY e.source_id, e.epg_id
                 ORDER BY score, e.epg_id COLLATE NOCASE LIMIT ?4",
            )?;
            let found: Vec<(String, String)> = statement
                .query_map(params![pattern, key, json_list(guide_ids), sql_limit(Some(limit))], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            found.into_iter().map(|(guide_id, epg_id)| guide_channel_row(db, guide_id, epg_id, now)).collect()
        })
    }

    /// A guide channel's name and what it shows at `now`, as far as its guide says.
    pub fn guide_channel(&self, guide_id: &str, epg_id: &str, now: Timestamp) -> anyhow::Result<GuideChannelRow> {
        self.read(|db| guide_channel_row(db, guide_id.to_string(), epg_id.to_string(), now))
    }

    /// Whether a guide lists a channel at all.
    pub fn guide_has_channel(&self, guide_id: &str, epg_id: &str) -> anyhow::Result<bool> {
        self.read(|db| {
            db.query_row(
                "SELECT EXISTS (SELECT 1 FROM epg_channels WHERE source_id = ?1 AND epg_id = ?2)
                     OR EXISTS (SELECT 1 FROM programs WHERE source_id = ?1 AND epg_id = ?2)",
                params![guide_id, epg_id],
                |row| row.get(0),
            )
        })
    }

    /// About when the guides' listings begin and end, if there are any: the
    /// first and last programme endings, which the `end` index answers at once
    /// (the first start would need a scan of every listing).
    pub fn guide_span(&self) -> anyhow::Result<Option<(Timestamp, Timestamp)>> {
        // One aggregate per query: SQLite reads MIN and MAX off the index only alone.
        let (first, last): (Option<Timestamp>, Option<Timestamp>) = self.read(|db| {
            db.query_row("SELECT (SELECT MIN(end) FROM programs), (SELECT MAX(end) FROM programs)", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
        })?;
        Ok(first.zip(last))
    }

    // MARK: Details

    /// Stored details and when they were fetched from the provider's detail
    /// endpoint (`None` when they only came from a list).
    pub fn details(&self, item_id: &str) -> anyhow::Result<Option<(DetailsRecord, Option<Timestamp>)>> {
        let row: Option<(String, Option<Timestamp>)> = self.read(|db| {
            db.query_row("SELECT json, fetched_at FROM media_details WHERE item_id = ?1", [item_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
        })?;
        Ok(row.map(|(text, fetched)| (serde_json::from_str(&text).unwrap_or_default(), fetched)))
    }

    // MARK: TMDB

    /// What TMDB said about something, as stored, and when it was asked.
    pub fn tmdb_cached(&self, kind: &str, tmdb_id: i64) -> anyhow::Result<Option<(String, Timestamp)>> {
        self.read(|db| {
            db.query_row("SELECT json, fetched_at FROM tmdb_cache WHERE kind = ?1 AND tmdb_id = ?2", params![kind, tmdb_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
        })
    }

    pub fn save_tmdb_cached(&self, kind: &str, tmdb_id: i64, json: &str, fetched_at: Timestamp) -> anyhow::Result<()> {
        self.write(|connection| {
            connection.execute(
                "INSERT OR REPLACE INTO tmdb_cache (kind, tmdb_id, json, fetched_at) VALUES (?1, ?2, ?3, ?4)",
                params![kind, tmdb_id, json, fetched_at],
            )?;
            Ok(())
        })
    }

    /// The TMDB id a lookup found before (`Some(None)` when it found
    /// nothing), and when it was looked up.
    pub fn tmdb_match(&self, kind: &str, lookup: &str) -> anyhow::Result<Option<(Option<i64>, Timestamp)>> {
        self.read(|db| {
            db.query_row("SELECT tmdb_id, looked_up_at FROM tmdb_matches WHERE kind = ?1 AND lookup = ?2", params![kind, lookup], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
        })
    }

    pub fn save_tmdb_match(&self, kind: &str, lookup: &str, tmdb_id: Option<i64>, looked_up_at: Timestamp) -> anyhow::Result<()> {
        self.write(|connection| {
            connection.execute(
                "INSERT OR REPLACE INTO tmdb_matches (kind, lookup, tmdb_id, looked_up_at) VALUES (?1, ?2, ?3, ?4)",
                params![kind, lookup, tmdb_id, looked_up_at],
            )?;
            Ok(())
        })
    }

    pub fn details_many(&self, ids: &[String]) -> anyhow::Result<HashMap<String, DetailsRecord>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<(String, String)> = self.read(|db| {
            let mut statement =
                db.prepare_cached("SELECT item_id, json FROM media_details WHERE item_id IN (SELECT value FROM json_each(?1))")?;
            statement.query_map([json_list(ids)], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
        })?;
        Ok(rows.into_iter().filter_map(|(id, text)| Some((id, serde_json::from_str(&text).ok()?))).collect())
    }
}

/// An open guide import: everything goes into one transaction.
pub(crate) struct GuideImport<'c> {
    tx: rusqlite::Transaction<'c>,
    source_id: String,
}

impl GuideImport<'_> {
    pub fn insert_programs(&mut self, programs: &[Program]) -> anyhow::Result<()> {
        let mut statement = self.tx.prepare_cached(
            "INSERT OR IGNORE INTO programs (source_id, epg_id, start, end, title, subtitle, summary, category, image,
                episode, flags, rating, star, year)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        )?;
        for program in programs {
            statement.execute(params![
                self.source_id,
                program.epg_id,
                program.start,
                program.end,
                program.title,
                program.subtitle,
                program.summary,
                program.category,
                program.image_url,
                program.episode,
                flags_to_bits(&program.flags),
                program.age_rating,
                program.star_rating,
                program.year
            ])?;
        }
        Ok(())
    }

    pub fn insert_channel_names(&mut self, names: &[ChannelName]) -> anyhow::Result<()> {
        let mut statement = self
            .tx
            .prepare_cached("INSERT INTO epg_channels (source_id, epg_id, name_key, name) VALUES (?1, ?2, ?3, ?4)")?;
        for name in names {
            statement.execute(params![self.source_id, name.epg_id, names::match_key(&name.display_name), name.display_name])?;
        }
        Ok(())
    }
}

fn insert_episodes(tx: &rusqlite::Transaction<'_>, episodes: &[Episode]) -> anyhow::Result<()> {
    let mut statement = tx.prepare_cached(
        "INSERT OR REPLACE INTO episodes (id, series_id, source_id, season, number, title, plot, duration, still, stream, air_date)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    for episode in episodes {
        statement.execute(params![
            episode.id,
            episode.series_id,
            episode.source_id,
            episode.season,
            episode.number,
            episode.title,
            episode.plot,
            episode.duration_seconds,
            episode.still_url,
            encode_stream(&episode.stream),
            episode.air_date
        ])?;
    }
    Ok(())
}

fn merge_details(
    tx: &rusqlite::Transaction<'_>,
    source_id: &str,
    details: &BTreeMap<String, DetailsRecord>,
    fetched_at: Option<Timestamp>,
) -> anyhow::Result<()> {
    if details.is_empty() {
        return Ok(());
    }
    let mut select = tx.prepare_cached("SELECT json FROM media_details WHERE item_id = ?1")?;
    let mut fetched = tx.prepare_cached(
        "INSERT OR REPLACE INTO media_details (item_id, source_id, json, fetched_at) VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut listed = tx.prepare_cached(
        "INSERT INTO media_details (item_id, source_id, json, fetched_at) VALUES (?1, ?2, ?3, NULL)
         ON CONFLICT(item_id) DO UPDATE SET json = excluded.json",
    )?;
    for (item_id, incoming) in details {
        let existing: Option<DetailsRecord> = select
            .query_row([item_id], |row| row.get::<_, String>(0))
            .optional()?
            .and_then(|text| serde_json::from_str(&text).ok());
        let merged = existing.map_or_else(|| incoming.clone(), |existing| existing.merged(incoming));
        let text = serde_json::to_string(&merged)?;
        match fetched_at {
            Some(at) => fetched.execute(params![item_id, source_id, text, at])?,
            None => listed.execute(params![item_id, source_id, text])?,
        };
    }
    Ok(())
}

/// What decides the guide channel each channel shows: the added guides, in
/// the user's order, and the guide channels chosen by hand.
#[derive(Debug, Clone, Default)]
pub(crate) struct GuideLinks {
    pub guides: Vec<String>,
    pub overrides: Vec<GuideOverride>,
}

/// Works out the guide channel each channel in `scope` (one source, or every
/// source) shows. A guide channel chosen by hand wins. Otherwise the
/// channel's own guide id is looked for in its source's guide, then in the
/// added guides in order; failing that, a guide channel with the same tidy
/// name, in the same order. A channel nothing matches keeps its own id.
fn resolve_guide_links(tx: &rusqlite::Transaction<'_>, scope: Option<&str>, links: &GuideLinks) -> anyhow::Result<()> {
    // Four lookups the epg_channels indexes answer, rather than one with ORs
    // that would scan the whole guide for every channel.
    tx.execute(
        "WITH guides(guide_id, rank) AS (SELECT value, key + 1 FROM json_each(?2))
         UPDATE channels SET (guide_source, epg_fallback) = (
            SELECT NULLIF(found.guide_id, channels.source_id), NULLIF(found.epg_id, channels.epg_id) FROM (
                SELECT e.source_id AS guide_id, e.epg_id AS epg_id, 0 AS by_name, 0 AS rank FROM epg_channels e
                    WHERE e.source_id = channels.source_id AND e.epg_id = channels.epg_id
                UNION ALL
                SELECT e.source_id, e.epg_id, 0, g.rank FROM guides g
                    JOIN epg_channels e ON e.source_id = g.guide_id AND e.epg_id = channels.epg_id
                UNION ALL
                SELECT e.source_id, e.epg_id, 1, 0 FROM epg_channels e
                    WHERE e.source_id = channels.source_id AND e.name_key = channels.name_key
                UNION ALL
                SELECT e.source_id, e.epg_id, 1, g.rank FROM guides g
                    JOIN epg_channels e ON e.source_id = g.guide_id AND e.name_key = channels.name_key
            ) found ORDER BY found.by_name, found.rank LIMIT 1
         )
         WHERE ?1 IS NULL OR source_id = ?1",
        params![scope, json_list(&links.guides)],
    )?;
    let mut chosen = tx.prepare_cached(
        "UPDATE channels SET guide_source = ?2, epg_fallback = ?3 WHERE id = ?1 AND (?4 IS NULL OR source_id = ?4)",
    )?;
    for choice in &links.overrides {
        chosen.execute(params![choice.channel_id, choice.guide_id, choice.epg_id, scope])?;
    }
    Ok(())
}

/// A guide channel as the catalog knows it: its guide, id, first display
/// name, and the title of what it shows at the moment asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuideChannelRow {
    pub guide_id: String,
    pub epg_id: String,
    pub name: Option<String>,
    pub now_title: Option<String>,
}

fn guide_channel_row(db: &Connection, guide_id: String, epg_id: String, now: Timestamp) -> rusqlite::Result<GuideChannelRow> {
    let name: Option<String> = db
        .query_row(
            "SELECT name FROM epg_channels WHERE source_id = ?1 AND epg_id = ?2 AND name IS NOT NULL ORDER BY rowid LIMIT 1",
            params![guide_id, epg_id],
            |row| row.get(0),
        )
        .optional()?;
    let now_title: Option<String> = db
        .query_row(
            "SELECT title FROM programs WHERE source_id = ?1 AND epg_id = ?2 AND start <= ?3 AND end > ?3 LIMIT 1",
            params![guide_id, epg_id, now],
            |row| row.get(0),
        )
        .optional()?;
    Ok(GuideChannelRow { guide_id, epg_id, name, now_title })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_queries_are_safe() {
        assert_eq!(fts_query("glass val").as_deref(), Some("\"glass\"* \"val\"*"));
        // Operators and quotes only ever reach FTS5 as quoted words.
        assert_eq!(fts_query("\" OR *").as_deref(), Some("\"OR\"*"));
        assert_eq!(fts_query("*** \"\""), None);
        assert_eq!(fts_query("café-noir").as_deref(), Some("\"café\"* \"noir\"*"));
    }

    #[test]
    fn flags_round_trip() {
        let flags = ProgramFlags { is_new: true, is_live: true, is_premiere: false, is_repeat: true };
        assert_eq!(bits_to_flags(flags_to_bits(&flags)), flags);
    }

    #[test]
    fn catalogs_from_before_guide_links_are_brought_up_to_date() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("catalog.sqlite");
        {
            // The first schema: no guide links, no guide channel names.
            let first = SCHEMA
                .replace(",\n    guide_source TEXT\n", "\n")
                .replace(", name_key TEXT NOT NULL, name TEXT)", ", name_key TEXT NOT NULL)")
                .replace(
                    "CREATE INDEX IF NOT EXISTS channels_guide_link ON channels(COALESCE(guide_source, source_id), COALESCE(epg_fallback, epg_id));\n",
                    "",
                );
            let db = Connection::open(&path).unwrap();
            db.execute_batch(&first).unwrap();
            db.execute(
                "INSERT INTO channels (id, source_id, name, name_key, epg_id, stream, sort_index)
                 VALUES ('s|ch|1', 's', 'One', 'one', 'one.uk', '{}', 0)",
                [],
            )
            .unwrap();
            db.pragma_update(None, "user_version", 1).unwrap();
            assert!(db.prepare("SELECT guide_source FROM channels").is_err(), "the test starts from the old tables");
        }
        let store = CatalogStore::open(&path).unwrap();
        let links = GuideLinks { guides: vec!["guide-x".into()], overrides: Vec::new() };
        store
            .import_guide("guide-x", &links, |import| {
                import.insert_channel_names(&[ChannelName { epg_id: "one.uk".into(), display_name: "Channel One".into() }])
            })
            .unwrap();
        assert_eq!(store.guide_link("s|ch|1").unwrap(), Some(("guide-x".into(), "one.uk".into())));
        assert_eq!(store.guide_channel("guide-x", "one.uk", 0).unwrap().name.as_deref(), Some("Channel One"));
        assert_eq!(store.channel("s|ch|1").unwrap().map(|c| c.name), Some("One".into()), "existing rows are kept");
    }

    #[test]
    fn catalogs_from_version_2_gain_the_indexes_titles_are_matched_by() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("catalog.sqlite");
        {
            let second = SCHEMA
                .lines()
                .filter(|line| !["_imdb ON", "_tmdb ON", "_title_key ON"].iter().any(|index| line.contains(index)))
                .collect::<Vec<_>>()
                .join("\n");
            let db = Connection::open(&path).unwrap();
            db.execute_batch(&second).unwrap();
            db.execute(
                "INSERT INTO movies (id, source_id, title, title_key, imdb_id, tmdb_id, stream)
                 VALUES ('s|mv|1', 's', 'Paper Boats', 'paperboats', 'tt0000001', NULL, '{}'),
                        ('s|mv|2', 's', 'Glass Valley', 'glassvalley', NULL, '77', '{}'),
                        ('s|mv|3', 's', 'Low Tide', 'lowtide', NULL, NULL, '{}'),
                        ('t|mv|4', 't', 'Paper Boats', 'paperboats', 'tt0000001', NULL, '{}')",
                [],
            )
            .unwrap();
            db.pragma_update(None, "user_version", 2).unwrap();
        }
        let store = CatalogStore::open(&path).unwrap();
        let indexes: Vec<String> = store
            .read(|db| {
                let mut statement = db.prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name IN ('movies', 'series')")?;
                statement.query_map([], |row| row.get(0))?.collect()
            })
            .unwrap();
        for index in ["movies_imdb", "movies_tmdb", "movies_title_key", "series_imdb", "series_tmdb", "series_title_key"] {
            assert!(indexes.iter().any(|name| name == index), "{index} in {indexes:?}");
        }
        let keys = MatchKeys { imdb: vec!["tt0000001".into()], tmdb: vec!["77".into()], title_keys: vec!["lowtide".into()] };
        let mut found: Vec<String> = store.movies_matching(&["s".to_string()], &keys).unwrap().into_iter().map(|m| m.id).collect();
        found.sort();
        assert_eq!(found, ["s|mv|1", "s|mv|2", "s|mv|3"], "only the sources asked about");
        assert!(store.movies_matching(&["s".to_string()], &MatchKeys::default()).unwrap().is_empty());
    }

    #[test]
    fn copies_of_a_film_are_found_through_the_version_index() {
        let folder = tempfile::tempdir().unwrap();
        let store = CatalogStore::open(&folder.path().join("catalog.sqlite")).unwrap();
        let plan: Vec<String> = store
            .read(|db| {
                let mut statement = db.prepare(&format!(
                    "EXPLAIN QUERY PLAN SELECT t.id FROM movies t WHERE NOT EXISTS (SELECT 1 FROM movies o WHERE {} = {} AND o.id <> t.id)",
                    version_key("o"),
                    version_key("t")
                ))?;
                statement.query_map([], |row| row.get(3))?.collect()
            })
            .unwrap();
        assert!(plan.iter().any(|step| step.contains("movies_version")), "{plan:?}");
    }

    /// Movies at the size of a large provider's catalogue: one poster per
    /// film, a page at a time, in each order.
    #[test]
    #[ignore = "timing"]
    fn grouping_a_large_catalogue_stays_quick() {
        use std::time::Instant;
        let folder = tempfile::tempdir().unwrap();
        let store = CatalogStore::open(&folder.path().join("catalog.sqlite")).unwrap();
        store
            .write(|connection| {
                let tx = connection.transaction()?;
                {
                    let mut insert = tx.prepare(
                        "INSERT INTO movies (id, source_id, title, title_key, year, poster, category_id, rating, added_at, tmdb_id, stream)
                         VALUES (?1, 's', ?2, ?3, ?4, 'p', ?5, ?6, ?7, ?8, '{}')",
                    )?;
                    for n in 0..31_000i64 {
                        // About one film in five is carried twice or three times.
                        let film = if n % 5 == 0 { n / 3 } else { n };
                        let title = format!("Film {film}");
                        insert.execute(params![
                            format!("s|mv|{n}"),
                            title,
                            names::match_key(&title),
                            1950 + film % 75,
                            format!("s|moviecat|{}", n % 300),
                            (film % 90) as f64 / 10.0,
                            1_700_000_000 + n * 60,
                            (film % 4 != 0).then(|| (film + 1).to_string())
                        ])?;
                    }
                }
                tx.commit()?;
                Ok(())
            })
            .unwrap();
        let sources = ["s".to_string()];
        for order in [Order::RecentlyAdded, Order::Title, Order::Year, Order::Rating] {
            for offset in [0, 12_000] {
                let browse = Browse {
                    source_ids: &sources,
                    category_id: None,
                    genre_words: &[],
                    genre_categories: &[],
                    excluded: &[],
                    title_tags: &[],
                    only: None,
                    without: &[],
                    group_versions: true,
                    order,
                    limit: 120,
                    offset,
                    years: None,
                };
                let started = Instant::now();
                let page = store.movies(&browse).unwrap();
                println!("{order:?} from {offset}: {} films in {:.1} ms", page.len(), started.elapsed().as_secs_f64() * 1000.0);
            }
        }
        let ids: Vec<String> = (0..120).map(|n| format!("s|mv|{}", n * 7)).collect();
        let started = Instant::now();
        let copies = store.movie_copies(&ids, &sources, &[], &[]).unwrap();
        println!("copies of 120 films: {} in {:.1} ms", copies.len(), started.elapsed().as_secs_f64() * 1000.0);
    }

    #[test]
    fn title_keys_group_copies() {
        assert_eq!(title_key("EN - Paper Boats 4K"), title_key("Paper Boats"));
        assert_eq!(title_key("|UK| Paper Boats (2024)"), "paperboats");
    }
}
