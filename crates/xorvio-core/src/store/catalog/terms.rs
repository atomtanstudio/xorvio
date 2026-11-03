//! The catalog's side of what films and series are found by besides their
//! names (`crate::terms`): each title's row in `title_terms`, written again
//! from what's kept (its list's plot and genre, its details, TMDB's terms)
//! whenever any of that changes, searches over it, and TMDB's terms.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use xorvio_model::Timestamp;

use super::{CatalogStore, SearchKind, fts_query, untagged};
use crate::details::DetailsRecord;
use crate::store::{json_list, sql_limit};
use crate::terms::SearchTerms;

/// Which titles' rows to write again.
pub(super) enum Titles<'a> {
    All,
    Source(&'a str),
    Ids(&'a [String]),
}

/// A column of the index a search looks in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TermColumn {
    People,
    Genres,
    Story,
}

impl TermColumn {
    fn name(self) -> &'static str {
        match self {
            Self::People => "people",
            Self::Genres => "genres",
            Self::Story => "story",
        }
    }
}

/// A film or series TMDB is to be asked about for search.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TitleWantingTerms {
    pub kind: SearchKind,
    pub item_id: String,
    pub source_id: String,
    pub title: String,
    pub year: Option<i64>,
    pub tmdb_id: Option<String>,
    pub imdb_id: Option<String>,
    pub added_at: Option<Timestamp>,
}

const TABLES: [(SearchKind, &str); 2] = [(SearchKind::Movie, "movies"), (SearchKind::Series, "series")];

/// Titles of `?1`'s sources that TMDB is to be asked about: their source
/// names no one in them, and TMDB hasn't been asked (or had nothing before
/// `?3`). Media folders' (`?2`) videos with no year and no ids aren't: they
/// are seldom films TMDB would know.
const WANTING: &str = "t.source_id IN (SELECT value FROM json_each(?1))
    AND NOT EXISTS (SELECT 1 FROM tmdb_terms k WHERE k.item_id = t.id AND (k.json IS NOT NULL OR k.looked_up_at >= ?3))
    AND NOT EXISTS (SELECT 1 FROM title_terms s WHERE s.item_id = t.id AND s.people <> '')
    AND NOT (t.source_id IN (SELECT value FROM json_each(?2)) AND t.year IS NULL
             AND COALESCE(t.tmdb_id, '') = '' AND COALESCE(t.imdb_id, '') = '')";

/// Writes these titles' rows again from what's kept. A title with nothing
/// to be found by has none.
pub(super) fn reindex(db: &Connection, titles: Titles<'_>) -> anyhow::Result<()> {
    let (filter, value) = match &titles {
        Titles::All => ("?1 IS NULL", None),
        Titles::Source(id) => ("t.source_id = ?1", Some(id.to_string())),
        Titles::Ids(ids) => ("t.id IN (SELECT value FROM json_each(?1))", Some(json_list(ids))),
    };
    match &titles {
        Titles::All => db.execute("DELETE FROM title_terms", [])?,
        Titles::Source(id) => db.execute("DELETE FROM title_terms WHERE source_id = ?1", [id])?,
        Titles::Ids(ids) => db.execute("DELETE FROM title_terms WHERE item_id IN (SELECT value FROM json_each(?1))", [json_list(ids)])?,
    };
    let mut insert =
        db.prepare_cached("INSERT INTO title_terms (item_id, kind, source_id, people, genres, story) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
    for (kind, table) in TABLES {
        let mut select = db.prepare(&format!(
            "SELECT t.id, t.plot, t.genre, d.json, k.json, t.source_id FROM {table} t
             LEFT JOIN media_details d ON d.item_id = t.id LEFT JOIN tmdb_terms k ON k.item_id = t.id
             WHERE {filter}"
        ))?;
        let mut rows = select.query(params![value])?;
        while let Some(row) = rows.next()? {
            let (id, terms) = terms_row(row)?;
            if terms.is_empty() {
                continue;
            }
            let source_id: String = row.get(5)?;
            let [people, genres, story] = terms.columns();
            insert.execute(params![id, kind.as_str(), source_id, people, genres, story])?;
        }
    }
    Ok(())
}

/// A title's id and terms from a row of (id, plot, genre, details, TMDB's terms).
fn terms_row(row: &Row<'_>) -> rusqlite::Result<(String, SearchTerms)> {
    let plot: Option<String> = row.get(1)?;
    let genre: Option<String> = row.get(2)?;
    let details: Option<DetailsRecord> = row.get::<_, Option<String>>(3)?.and_then(|text| serde_json::from_str(&text).ok());
    let tmdb: Option<SearchTerms> = row.get::<_, Option<String>>(4)?.and_then(|text| serde_json::from_str(&text).ok());
    Ok((row.get(0)?, SearchTerms::of_title(plot.as_deref(), genre.as_deref(), details.as_ref(), tmdb.as_ref())))
}

impl CatalogStore {
    /// Films or series (`kind`) whose `column` has words starting with each
    /// word of `text`, best first, leaving out what [`CatalogStore::search`]
    /// leaves out; with the words of the story around them when that's
    /// where they are.
    #[allow(clippy::too_many_arguments)]
    pub fn search_terms(
        &self,
        column: TermColumn,
        text: &str,
        kind: SearchKind,
        source_ids: &[String],
        limit: usize,
        excluded: &[String],
        title_tags: &[String],
    ) -> anyhow::Result<Vec<(String, Option<String>)>> {
        let Some(query) = fts_query(text) else { return Ok(Vec::new()) };
        let query = format!("{{{}}} : ({query})", column.name());
        let table = if kind == SearchKind::Series { "series" } else { "movies" };
        let around = if column == TermColumn::Story { "snippet(title_terms_index, 2, '', '', '…', 8)" } else { "NULL" };
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT s.item_id, {around} FROM title_terms_index
                 JOIN title_terms s ON s.id = title_terms_index.rowid
                 JOIN {table} t ON t.id = s.item_id
                 WHERE title_terms_index MATCH ?1 AND s.kind = ?2 AND s.source_id IN (SELECT value FROM json_each(?3))
                   AND s.item_id NOT IN (SELECT value FROM json_each(?5))
                   AND (t.category_id IS NULL OR t.category_id NOT IN (SELECT value FROM json_each(?5)))
                   AND {}
                 ORDER BY title_terms_index.rank LIMIT ?4",
                untagged("t", "?6")
            ))?;
            statement
                .query_map(
                    params![query, kind.as_str(), json_list(source_ids), sql_limit(Some(limit)), json_list(excluded), json_list(title_tags)],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?
                .collect()
        })
    }

    /// What these films and series are found by, by id.
    pub fn title_terms(&self, ids: &[String]) -> anyhow::Result<HashMap<String, SearchTerms>> {
        self.read(|db| {
            let mut found = HashMap::new();
            for (_, table) in TABLES {
                let mut statement = db.prepare_cached(&format!(
                    "SELECT t.id, t.plot, t.genre, d.json, k.json FROM {table} t
                     LEFT JOIN media_details d ON d.item_id = t.id LEFT JOIN tmdb_terms k ON k.item_id = t.id
                     WHERE t.id IN (SELECT value FROM json_each(?1))"
                ))?;
                for row in statement.query_map([json_list(ids)], terms_row)? {
                    let (id, terms) = row?;
                    found.insert(id, terms);
                }
            }
            Ok(found)
        })
    }

    /// Films and series TMDB is to be asked about for search (see
    /// [`WANTING`]), the most recently added first.
    pub fn titles_wanting_terms(
        &self,
        source_ids: &[String],
        folder_ids: &[String],
        retry_before: Timestamp,
        limit: usize,
    ) -> anyhow::Result<Vec<TitleWantingTerms>> {
        let mut found = self.read(|db| {
            let mut found = Vec::new();
            for (kind, table) in TABLES {
                let mut statement = db.prepare_cached(&format!(
                    "SELECT t.id, t.source_id, t.title, t.year, t.tmdb_id, t.imdb_id, t.added_at FROM {table} t
                     WHERE {WANTING}
                     ORDER BY t.added_at IS NULL, t.added_at DESC, t.id LIMIT ?4"
                ))?;
                let rows = statement.query_map(
                    params![json_list(source_ids), json_list(folder_ids), retry_before, sql_limit(Some(limit))],
                    |row| {
                        Ok(TitleWantingTerms {
                            kind,
                            item_id: row.get(0)?,
                            source_id: row.get(1)?,
                            title: row.get(2)?,
                            year: row.get(3)?,
                            tmdb_id: row.get(4)?,
                            imdb_id: row.get(5)?,
                            added_at: row.get(6)?,
                        })
                    },
                )?;
                for row in rows {
                    found.push(row?);
                }
            }
            Ok(found)
        })?;
        found.sort_by_key(|title| (title.added_at.is_none(), std::cmp::Reverse(title.added_at)));
        found.truncate(limit);
        Ok(found)
    }

    /// How many films and series TMDB is still to be asked about for search.
    pub fn count_titles_wanting_terms(&self, source_ids: &[String], folder_ids: &[String], retry_before: Timestamp) -> anyhow::Result<usize> {
        self.read(|db| {
            let mut count = 0;
            for (_, table) in TABLES {
                count += db.query_row(
                    &format!("SELECT COUNT(*) FROM {table} t WHERE {WANTING}"),
                    params![json_list(source_ids), json_list(folder_ids), retry_before],
                    |row| row.get::<_, i64>(0),
                )?;
            }
            Ok(count.max(0) as usize)
        })
    }

    /// TMDB's answer for copies of one title, as (id, source id), with its
    /// id there and its terms (none when TMDB had nothing), and their rows
    /// written again.
    pub fn save_tmdb_terms(
        &self,
        items: &[(String, String)],
        kind: SearchKind,
        found: Option<(i64, &SearchTerms)>,
        looked_up_at: Timestamp,
    ) -> anyhow::Result<()> {
        let json = found.map(|(_, terms)| serde_json::to_string(terms)).transpose()?;
        let tmdb_id = found.map(|(id, _)| id);
        self.write(|connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            {
                let mut insert = tx.prepare_cached(
                    "INSERT OR REPLACE INTO tmdb_terms (item_id, source_id, kind, tmdb_id, json, looked_up_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?;
                for (item_id, source_id) in items {
                    insert.execute(params![item_id, source_id, kind.as_str(), tmdb_id, json, looked_up_at])?;
                }
            }
            let ids: Vec<String> = items.iter().map(|(id, _)| id.clone()).collect();
            reindex(&tx, Titles::Ids(&ids))?;
            tx.commit()?;
            Ok(())
        })
    }

    /// TMDB's terms kept for a title from any copy of it.
    pub fn tmdb_terms_of(&self, kind: SearchKind, tmdb_id: i64) -> anyhow::Result<Option<SearchTerms>> {
        let text: Option<String> = self.read(|db| {
            db.query_row(
                "SELECT json FROM tmdb_terms WHERE kind = ?1 AND tmdb_id = ?2 AND json IS NOT NULL LIMIT 1",
                params![kind.as_str(), tmdb_id],
                |row| row.get(0),
            )
            .optional()
        })?;
        Ok(text.and_then(|text| serde_json::from_str(&text).ok()))
    }
}
