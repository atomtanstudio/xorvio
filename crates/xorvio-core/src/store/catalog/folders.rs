//! A media folder's videos as its own library page lists them: by the
//! folder their file is in, filtered by what the file holds (file_facts),
//! and the facts themselves.

use std::collections::HashMap;

use rusqlite::params;
use xorvio_model::{ClipKind, ClipLength, FileFacts, FolderSort, GIF_BITRATE, LARGEST_GIF_SIDE, LONGEST_CLIP_SECONDS, Movie};

use super::{CatalogStore, like_escaped};
use xorvio_model::Timestamp;

/// A media folder's film's file, as its stream says.
const FILE_PATH: &str = "json_extract(m.stream, '$.path')";

/// One page of a media folder's videos, with what their files hold.
pub(crate) struct FolderPage {
    pub videos: Vec<(Movie, Option<FileFacts>)>,
    /// How many match, on every page.
    pub total: i64,
    /// How many in the folder haven't been read yet.
    pub unread: i64,
}

/// A media folder's video moved or renamed: its id before and after, and its file now.
pub(crate) struct VideoMove {
    pub from: String,
    pub to: String,
    pub path: String,
}

/// What to list of a media folder: the videos whose files are in `prefix`
/// (a folder's path ending in "/"), or below it too.
pub(crate) struct FolderFilter<'a> {
    pub source_id: &'a str,
    pub prefix: &'a str,
    pub below: bool,
    pub kind: Option<ClipKind>,
    pub length: Option<ClipLength>,
    pub sort: FolderSort,
    pub limit: i64,
    pub offset: i64,
}

/// The same rules as `FileFacts::kind`, in SQL over file_facts as `f`.
fn kind_condition(kind: ClipKind) -> String {
    let clip = format!("f.audio = 0 AND f.duration > 0 AND f.duration <= {LONGEST_CLIP_SECONDS}");
    let gif = format!(
        "(MAX(IFNULL(f.width, 1000000), IFNULL(f.height, 1000000)) <= {LARGEST_GIF_SIDE} OR (f.size IS NOT NULL AND f.size * 8.0 / f.duration < {GIF_BITRATE}))"
    );
    match kind {
        ClipKind::Gif => format!("({clip} AND {gif})"),
        ClipKind::Animation => format!("({clip} AND NOT {gif})"),
        ClipKind::Video => format!("(f.duration > 0 AND (f.audio = 1 OR f.duration > {LONGEST_CLIP_SECONDS}))"),
    }
}

const FROM: &str = "FROM movies m LEFT JOIN file_facts f ON f.item_id = m.id";

/// A listing's conditions and order in SQL, over [`FROM`]. Its parameters:
/// ?1 the source, ?2 the folder's path as a pattern, ?3 where its files' own
/// names start, ?4 whether to go below it.
struct Chosen {
    /// The folder's videos, whatever their files hold.
    place: String,
    /// Those the filters choose.
    chosen: String,
    order: &'static str,
    pattern: String,
    after: i64,
    below: i64,
}

impl Chosen {
    fn new(filter: &FolderFilter<'_>) -> Self {
        let place = format!(
            "m.source_id = ?1 AND json_extract(m.stream, '$.type') = 'file' AND {FILE_PATH} LIKE ?2 ESCAPE '\\' \
             AND (?4 = 1 OR instr(substr({FILE_PATH}, ?3), '/') = 0)"
        );
        let mut chosen = vec![place.clone()];
        if let Some(kind) = filter.kind {
            chosen.push(kind_condition(kind));
        }
        match filter.length {
            Some(ClipLength::Short) => chosen.push("f.duration > 0 AND f.duration < 60".into()),
            Some(ClipLength::Long) => chosen.push("f.duration >= 60".into()),
            None => {}
        }
        let order = match filter.sort {
            FolderSort::Added => "m.added_at DESC, m.title COLLATE NOCASE",
            FolderSort::Name => "m.title COLLATE NOCASE, m.id",
            FolderSort::Length => "f.duration IS NULL, f.duration DESC, m.title COLLATE NOCASE",
            FolderSort::Size => "f.size IS NULL, f.size DESC, m.title COLLATE NOCASE",
        };
        Self {
            place,
            chosen: chosen.join(" AND "),
            order,
            pattern: format!("{}%", like_escaped(filter.prefix)),
            after: filter.prefix.chars().count() as i64 + 1,
            below: filter.below as i64,
        }
    }
}

impl CatalogStore {
    /// One page of a media folder's videos, with what their files hold, and
    /// how many match on every page and how many here haven't been read.
    pub(crate) fn folder_videos(&self, filter: &FolderFilter<'_>) -> anyhow::Result<FolderPage> {
        let Chosen { place, chosen, order, pattern, after, below } = Chosen::new(filter);
        let (rows, total, unread) = self.read(|db| {
            let mut page = db.prepare(&format!(
                "SELECT m.id, f.duration, f.width, f.height, f.audio, f.size {FROM} WHERE {chosen} ORDER BY {order} LIMIT ?5 OFFSET ?6"
            ))?;
            let rows = page
                .query_map(params![filter.source_id, pattern, after, below, filter.limit, filter.offset], |row| {
                    let id: String = row.get(0)?;
                    let audio: Option<i64> = row.get(4)?;
                    let facts = match audio {
                        Some(audio) => Some(FileFacts {
                            duration_seconds: row.get(1)?,
                            width: row.get(2)?,
                            height: row.get(3)?,
                            audio: audio != 0,
                            size_bytes: row.get(5)?,
                        }),
                        None => None,
                    };
                    Ok((id, facts))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let counted = |condition: &str| -> rusqlite::Result<i64> {
                db.query_row(&format!("SELECT COUNT(*) {FROM} WHERE {condition}"), params![filter.source_id, pattern, after, below], |row| row.get(0))
            };
            let total = counted(&chosen)?;
            let unread = counted(&format!("{place} AND f.item_id IS NULL"))?;
            Ok((rows, total, unread))
        })?;
        let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
        let mut facts: HashMap<String, Option<FileFacts>> = rows.into_iter().collect();
        let videos = self
            .movies_by_ids(&ids)?
            .into_iter()
            .map(|movie| {
                let known = facts.remove(&movie.id).flatten();
                (movie, known)
            })
            .collect();
        Ok(FolderPage { videos, total, unread })
    }

    /// Every video's id a listing would show on all its pages, in its order.
    pub(crate) fn folder_video_ids(&self, filter: &FolderFilter<'_>) -> anyhow::Result<Vec<String>> {
        let Chosen { chosen, order, pattern, after, below, .. } = Chosen::new(filter);
        self.read(|db| {
            let mut statement = db.prepare(&format!("SELECT m.id {FROM} WHERE {chosen} ORDER BY {order}"))?;
            statement.query_map(params![filter.source_id, pattern, after, below], |row| row.get(0))?.collect()
        })
    }

    /// How many of a media folder's videos are in each folder directly below
    /// `prefix` (a folder's path ending in "/"), and below that, by name.
    pub(crate) fn folder_counts(&self, source_id: &str, prefix: &str) -> anyhow::Result<HashMap<String, i64>> {
        let pattern = format!("{}%", like_escaped(prefix));
        let after = prefix.chars().count() as i64 + 1;
        self.read(|db| {
            let mut statement = db.prepare(&format!(
                "SELECT substr(rest, 1, instr(rest, '/') - 1) AS name, COUNT(*) FROM \
                 (SELECT substr({FILE_PATH}, ?3) AS rest FROM movies m WHERE m.source_id = ?1 \
                  AND json_extract(m.stream, '$.type') = 'file' AND {FILE_PATH} LIKE ?2 ESCAPE '\\') \
                 WHERE instr(rest, '/') > 0 GROUP BY name"
            ))?;
            statement.query_map(params![source_id, pattern, after], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
        })
    }

    /// Up to `limit` of these sources' videos whose files haven't been read, with their files.
    pub(crate) fn videos_without_facts(&self, source_ids: &[String], limit: i64) -> anyhow::Result<Vec<(String, String, String)>> {
        self.read(|db| {
            let mut statement = db.prepare_cached(&format!(
                "SELECT m.id, m.source_id, {FILE_PATH} FROM movies m LEFT JOIN file_facts f ON f.item_id = m.id \
                 WHERE m.source_id IN (SELECT value FROM json_each(?1)) AND json_extract(m.stream, '$.type') = 'file' \
                 AND f.item_id IS NULL ORDER BY m.added_at DESC LIMIT ?2"
            ))?;
            statement.query_map(params![super::json_list(source_ids), limit], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect()
        })
    }

    /// What a video's file holds (`None`: it couldn't be read), kept so it's read once.
    pub(crate) fn save_file_facts(&self, item_id: &str, source_id: &str, facts: Option<&FileFacts>, read_at: Timestamp) -> anyhow::Result<()> {
        self.write(|connection| {
            let facts = facts.copied().unwrap_or_default();
            connection.execute(
                "INSERT OR REPLACE INTO file_facts (item_id, source_id, duration, width, height, audio, size, read_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![item_id, source_id, facts.duration_seconds, facts.width, facts.height, facts.audio as i64, facts.size_bytes, read_at],
            )?;
            Ok(())
        })
    }

    /// Videos deleted from a media folder, gone from the catalog before it's read again.
    pub(crate) fn forget_videos(&self, ids: &[String]) -> anyhow::Result<()> {
        self.write(|connection| {
            let tx = connection.transaction()?;
            for id in ids {
                tx.execute("DELETE FROM movies WHERE id = ?1", [id])?;
                tx.execute("DELETE FROM search WHERE item_id = ?1", [id])?;
                tx.execute("DELETE FROM title_terms WHERE item_id = ?1", [id])?;
                tx.execute("DELETE FROM tmdb_terms WHERE item_id = ?1", [id])?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// Videos moved or renamed in a media folder: each under its new id and
    /// file at once, keeping what was found for it (its file's facts, its
    /// frame or poster, its details).
    pub(crate) fn move_videos(&self, moves: &[VideoMove]) -> anyhow::Result<()> {
        self.write(|connection| {
            let tx = connection.transaction()?;
            for video in moves {
                let ids = params![video.from, video.to];
                tx.execute("UPDATE OR REPLACE movies SET id = ?2, stream = json_set(stream, '$.path', ?3) WHERE id = ?1", params![video.from, video.to, video.path])?;
                tx.execute("UPDATE search SET item_id = ?2 WHERE item_id = ?1", ids)?;
                for table in ["file_facts", "tmdb_posters", "media_details", "tmdb_terms"] {
                    tx.execute(&format!("UPDATE OR REPLACE {table} SET item_id = ?2 WHERE item_id = ?1"), ids)?;
                }
                // A row replaced by OR REPLACE skips the triggers that keep the index in step.
                tx.execute("DELETE FROM title_terms WHERE item_id = ?2", ids)?;
                tx.execute("UPDATE title_terms SET item_id = ?2 WHERE item_id = ?1", ids)?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// Forgets facts kept for a source's videos that are gone.
    pub(crate) fn forget_stale_facts(&self, source_id: &str) -> anyhow::Result<()> {
        self.write(|connection| {
            connection.execute(
                "DELETE FROM file_facts WHERE source_id = ?1 AND item_id NOT IN (SELECT id FROM movies WHERE source_id = ?1)",
                [source_id],
            )?;
            Ok(())
        })
    }
}
