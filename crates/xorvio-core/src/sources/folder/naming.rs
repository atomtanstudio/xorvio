//! What a video file is, from where it sits in a folder and how it's named,
//! the way people (and media managers) usually name them:
//!
//! ```text
//! Heat (1995).mkv                           a film
//! Heat (1995)/Heat.mkv                      a film, its year from the folder
//! Arrival.2016.1080p.BluRay.x264.mkv        a film, release details dropped
//! Severance/Season 2/Severance S02E03.mkv   an episode
//! Severance/Season 2/03 - Who Is Alive.mkv  an episode, numbered in its season folder
//! TV/The.Bear.S01E01.System.mkv             an episode, its series from the name
//! Fargo 3x05.avi                            an episode, numbered the other way
//! Show/Season 1/Show S01E01E02.mkv          two episodes in one file
//! ```
//!
//! Samples and extras (trailers, featurettes, deleted scenes) are left out.
//! The `regex` crate has no lookaround, so the "not part of a longer word"
//! conditions are checked in code next to the patterns.

use std::sync::LazyLock;

use chrono::Datelike;
use regex::{Match, Regex};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("built-in pattern")
}

pub(crate) const VIDEO_EXTENSIONS: &[&str] = &["mkv", "mp4", "m4v", "mov", "avi", "ts", "m2ts", "mts", "webm", "wmv", "mpg", "mpeg"];
pub(crate) const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];
pub(crate) const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "vtt", "ass", "ssa"];

/// Folders of extras beside a film or series, left out wherever they are.
const EXTRAS_FOLDERS: &[&str] = &[
    "extras", "extra", "featurettes", "featurette", "behind the scenes", "deleted scenes", "interviews", "trailers", "trailer",
    "sample", "samples", "bonus", "bonus features",
];
/// Extras inside a film's or series' folder, but at the top of a folder more
/// likely a shelf someone made ("Shorts", "Other").
const EXTRAS_INSIDE: &[&str] = &["shorts", "scenes", "other"];

/// File names that say nothing about the film, so its folder names it.
const GENERIC_NAMES: &[&str] = &["movie", "film", "video", "feature", "main", "title", "title t00", "title_t00"];

/// What a video file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Item {
    Movie {
        title: String,
        year: Option<i64>,
    },
    Episode {
        series: String,
        /// The series' year, when its folder or the file name gives one.
        year: Option<i64>,
        season: i64,
        number: i64,
        /// The last episode in a file that holds several (S01E01E02 → 2).
        last: Option<i64>,
        title: Option<String>,
    },
}

/// The extension of a file name, lowercased.
pub(crate) fn extension(name: &str) -> Option<String> {
    let file = name.rsplit('/').next().unwrap_or(name);
    file.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase())
}

pub(crate) fn has_extension(name: &str, extensions: &[&str]) -> bool {
    extension(name).is_some_and(|found| extensions.contains(&found.as_str()))
}

/// A file name without its extension.
pub(crate) fn base_name(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.rsplit_once('.').map_or(file, |(base, _)| base)
}

/// The folder part of a path below the folder ("" for one at the top).
pub(crate) fn parent_path(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// Whether a folder `depth` levels below the chosen one (0 at the top) holds extras.
pub(crate) fn is_extras_folder(name: &str, depth: usize) -> bool {
    let lower = name.trim().to_lowercase();
    EXTRAS_FOLDERS.contains(&lower.as_str()) || (depth > 0 && EXTRAS_INSIDE.contains(&lower.as_str()))
}

/// What a file is, from its path below the folder ("/" between parts) and
/// the chosen folder's own name, which stands above what's in it (a folder
/// chosen for one film or one series). `None` for files to leave out.
pub(crate) fn classify(relative: &str, root_name: &str) -> Option<Item> {
    let parts: Vec<&str> = relative.split('/').filter(|part| !part.is_empty()).collect();
    let (file, folders) = parts.split_last()?;
    if folders.iter().enumerate().any(|(depth, folder)| is_extras_folder(folder, depth)) {
        return None;
    }
    let base = base_name(file);
    if is_sample(base) {
        return None;
    }
    // The names above the file, the chosen folder's first.
    let chain: Vec<&str> = std::iter::once(root_name).filter(|name| !name.is_empty()).chain(folders.iter().copied()).collect();
    let parent = chain.last().copied();
    let season_from_folder = parent.and_then(season_number);
    // The series' own folder: above a season folder, or the folder a file sits in.
    let series_folder = if season_from_folder.is_some() { chain.len().checked_sub(2).map(|at| chain[at]) } else { parent };

    if let Some(found) = episode_match(base) {
        let from_name = clean_title(found.before);
        let from_folder = series_folder.map(clean_title).filter(|(title, _)| !title.is_empty());
        // A season folder's parent names the series better than a release name does.
        let (series, year) = if season_from_folder.is_some() {
            from_folder.unwrap_or(from_name)
        } else if from_name.0.is_empty() {
            from_folder.unwrap_or_default()
        } else {
            from_name
        };
        if series.is_empty() {
            return None;
        }
        let title = clean_title(found.after).0;
        return Some(Item::Episode {
            series,
            year,
            season: found.season,
            number: found.number,
            last: found.last,
            title: (!title.is_empty()).then_some(title),
        });
    }
    if let (Some(season), Some(folder)) = (season_from_folder, series_folder) {
        let (series, year) = clean_title(folder);
        if !series.is_empty()
            && let Some((number, rest)) = leading_episode_number(base)
        {
            let title = clean_title(rest).0;
            return Some(Item::Episode { series, year, season, number, last: None, title: (!title.is_empty()).then_some(title) });
        }
    }

    let (mut title, mut year) = clean_title(base);
    if let Some(parent) = parent {
        let (folder_title, folder_year) = clean_title(parent);
        // "Heat (1995)/Heat.mkv", or a file with a meaningless name in a film's
        // folder (but not in a folder whose name means no more than the file's).
        if title.is_empty() || GENERIC_NAMES.contains(&title.to_lowercase().as_str()) || (looks_like_hash(base) && !looks_like_hash(parent)) {
            (title, year) = (folder_title, folder_year);
        } else if year.is_none() && folder_year.is_some() && fold_key(&folder_title) == fold_key(&title) {
            year = folder_year;
        }
    }
    (!title.is_empty()).then_some(Item::Movie { title, year })
}

fn is_sample(base: &str) -> bool {
    let lower = base.to_lowercase();
    lower == "sample"
        || ["-sample", ".sample", "_sample"].iter().any(|end| lower.ends_with(end))
        || ["sample-", "sample.", "sample_"].iter().any(|start| lower.starts_with(start))
}

static CAMERA_NAME: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"(?i)^(?:(?:video|vid|img|dsc|dscn|dscf|mvi|mov|gopr|gp|gx|dji|pxl|clip|recording|untitled)[\s._-]*\d[\d\s._-]*|\d{8}[\s._-]\d{6}\d*|screen ?recording\b.*|.+?[\s._-]0\d{3,}[\s._-]?)$",
    )
});

/// A name a camera, phone, recorder or video tool gives every file, telling
/// one apart only by its number: "video 00001", "IMG_1234", "MVI_0042",
/// "GOPR0042", "PXL_20240512_123456", "20240512_123456", or any name ending
/// in a zero-padded counter ("FusionX 00001", "ComfyUI_00001_").
pub(crate) fn camera_name(base: &str) -> bool {
    CAMERA_NAME.is_match(base.trim())
}

/// A file's own name, tidied but kept whole ("VJ_LOOPS_2019-1" → "VJ LOOPS
/// 2019-1"), for a video that isn't a film to be named by.
pub(crate) fn own_name(base: &str) -> String {
    let spaced = if base.contains(' ') { base.replace('_', " ") } else { base.replace(['_', '.'], " ") };
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn looks_like_hash(base: &str) -> bool {
    base.len() >= 16 && base.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

static SEASON_FOLDER: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^(?:season|series|staffel|saison|temporada|s)[\s._-]*(\d{1,3})(?:\s*[(\[].*)?$"));

/// "Season 2", "S02", "Series 3", "Staffel 1", "Saison 4", "Temporada 5",
/// "Season 1 (2008)"; "Specials" is season 0.
pub(crate) fn season_number(folder: &str) -> Option<i64> {
    let lower = folder.trim().to_lowercase();
    if lower == "specials" || lower == "special" {
        return Some(0);
    }
    SEASON_FOLDER.captures(&lower)?[1].parse().ok()
}

/// An episode code found in a file name, and the text either side of it.
#[derive(Debug, PartialEq, Eq)]
struct EpisodeMatch<'t> {
    before: &'t str,
    season: i64,
    number: i64,
    last: Option<i64>,
    after: &'t str,
}

static LETTERED_CODE: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)s(\d{1,2})[\s._-]*e(\d{1,3})"));
static CROSSED_CODE: LazyLock<Regex> = LazyLock::new(|| regex(r"(\d{1,2})x(\d{2,3})"));

/// "S01E02", "s01 e02", "S01E01E02" and "1x02", standing on their own
/// rather than inside a longer word or number.
fn episode_match(base: &str) -> Option<EpisodeMatch<'_>> {
    for (pattern, letters_before) in [(&*LETTERED_CODE, true), (&*CROSSED_CODE, false)] {
        let mut from = 0;
        while let Some(captures) = pattern.captures_at(base, from) {
            let whole = captures.get(0)?;
            let before_ok = base[..whole.start()]
                .chars()
                .next_back()
                .is_none_or(|c| if letters_before { !c.is_ascii_alphanumeric() } else { !c.is_ascii_digit() });
            let after_ok = base[whole.end()..].chars().next().is_none_or(|c| !c.is_ascii_digit());
            if before_ok
                && after_ok
                && let (Ok(season), Ok(number)) = (captures[1].parse(), captures[2].parse())
            {
                let (last, end) = more_episodes(base, whole.end(), number);
                return Some(EpisodeMatch { before: &base[..whole.start()], season, number, last, after: &base[end..] });
            }
            from = whole.start() + base[whole.start()..].chars().next().map_or(1, char::len_utf8);
        }
    }
    None
}

/// Further episodes in the same file after its first code: "E02" in
/// "S01E01E02", "S01E01-E02" or "S01E01 E02", or "-02" in "S01E01-02" (but
/// not "-720p"). Returns the last one and where the codes end.
fn more_episodes(base: &str, from: usize, first: i64) -> (Option<i64>, usize) {
    let bytes = base.as_bytes();
    let (mut position, mut previous, mut last) = (from, first, None);
    loop {
        let mut at = position;
        let mut dashed = false;
        while at < bytes.len() && matches!(bytes[at], b' ' | b'.' | b'_' | b'-') {
            dashed |= bytes[at] == b'-';
            at += 1;
        }
        let lettered = at < bytes.len() && matches!(bytes[at], b'e' | b'E');
        if lettered {
            at += 1;
        } else if !dashed {
            break;
        }
        let digits = at;
        while at < bytes.len() && bytes[at].is_ascii_digit() && at - digits < 3 {
            at += 1;
        }
        let next = bytes.get(at);
        if at == digits || next.is_some_and(u8::is_ascii_digit) || (!lettered && next.is_some_and(u8::is_ascii_alphanumeric)) {
            break;
        }
        let Ok(number) = base[digits..at].parse::<i64>() else { break };
        if number <= previous {
            break;
        }
        (previous, last, position) = (number, Some(number), at);
    }
    (last, position)
}

static LEADING_NUMBER: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)^(?:episode|ep|e)?[\s._-]*(\d{1,3})"));

/// "03 - Who Is Alive", "E03 Who Is Alive", "Episode 3", in a season folder.
fn leading_episode_number(base: &str) -> Option<(i64, &str)> {
    let captures = LEADING_NUMBER.captures(base)?;
    let digits = captures.get(1)?;
    if base[digits.end()..].chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((digits.as_str().parse().ok()?, &base[digits.end()..]))
}

static BRACKET_TAG: LazyLock<Regex> = LazyLock::new(|| regex(r"\[([^\]]*)\]"));
static YEAR_ONLY: LazyLock<Regex> = LazyLock::new(|| regex(r"^(?:19|20)\d{2}$"));
static YEAR: LazyLock<Regex> = LazyLock::new(|| regex(r"(?:19|20)\d{2}"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| regex(r"\s+"));
/// Release details that never belong to a title: resolutions, sources, codecs, audio.
static RELEASE_DETAIL: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"(?i)2160p|1080p|1080i|720p|576p|480p|4k|uhd|blu-?ray|bdrip|brrip|bdremux|remux|web-?dl|webrip|hdtv|hdrip|dvdrip|dvdscr|dvd|hdr10\+?|dolby[\s._-]?vision|x26[45]|h[\s._]?26[45]|hevc|avc|xvid|divx|e?ac3|ddp?5[\s._]?1|dts-hd|dts|truehd|atmos|10bit|8bit",
    )
});
/// Release words that can also be ordinary words, dropped only right before
/// a release detail ("Aliens Extended 1080p", but "The Extended Family").
const RELEASE_WORDS: &[&str] =
    &["proper", "repack", "extended", "unrated", "uncut", "remastered", "imax", "internal", "multi", "subbed", "dubbed", "hdr", "dv", "web"];

/// Matches of `pattern` that stand on their own: the characters either side
/// aren't ASCII letters or digits (`letters`), or aren't digits.
fn standalone<'t>(pattern: &Regex, text: &'t str, letters: bool) -> Vec<Match<'t>> {
    let inside = |c: char| if letters { c.is_ascii_alphanumeric() } else { c.is_ascii_digit() };
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(candidate) = pattern.find_at(text, from) {
        let before = text[..candidate.start()].chars().next_back();
        let after = text[candidate.end()..].chars().next();
        if !before.is_some_and(inside) && !after.is_some_and(inside) {
            found.push(candidate);
            from = candidate.end();
        } else {
            from = candidate.start() + text[candidate.start()..].chars().next().map_or(1, char::len_utf8);
        }
        if from >= text.len() {
            break;
        }
    }
    found
}

/// A display title and year from a file or folder name: separators become
/// spaces, bracketed tags go, and everything from the year or the first
/// release detail ("1080p", "BluRay") on is dropped.
pub(crate) fn clean_title(raw: &str) -> (String, Option<i64>) {
    // [Group] and [Site] tags, but a year in brackets stays.
    let mut text = BRACKET_TAG
        .replace_all(raw, |captures: &regex::Captures<'_>| {
            if YEAR_ONLY.is_match(captures[1].trim()) { captures[0].to_string() } else { " ".to_string() }
        })
        .into_owned();
    // Scene names use dots or underscores for spaces; names with real spaces keep their dots ("Mr. Robot").
    if !text.contains(' ') || text.contains('_') {
        text = text.replace(['.', '_'], " ");
    }
    // The last year that isn't the whole title ("1917 (2019)" is 1917 from 2019),
    // and not one still to come: "Blade Runner 2049" keeps its number.
    let latest = i64::from(chrono::Utc::now().year()) + 2;
    let year = standalone(&YEAR, &text, false)
        .into_iter()
        .rfind(|found| found.start() > 0 && found.as_str().parse::<i64>().is_ok_and(|year| year <= latest))
        .map(|found| (found.start(), found.as_str().parse::<i64>().unwrap_or_default()));
    if let Some((start, _)) = year {
        text.truncate(start);
    }
    if let Some(cut) = release_cut(&text) {
        text.truncate(cut);
    }
    let text = SPACES.replace_all(&text, " ");
    let title = text.trim_matches(|c: char| c.is_whitespace() || " -–—([{".contains(c));
    (title.to_string(), year.map(|(_, year)| year))
}

/// Where a title's release details start, if it has any.
fn release_cut(text: &str) -> Option<usize> {
    let detail = standalone(&RELEASE_DETAIL, text, true).into_iter().find(|found| found.start() > 0)?;
    let mut cut = detail.start();
    loop {
        let head = text[..cut].trim_end_matches(|c: char| c.is_whitespace() || "-._[(".contains(c));
        let start = head.char_indices().rev().find(|(_, c)| !c.is_ascii_alphanumeric()).map_or(0, |(at, c)| at + c.len_utf8());
        if start > 0 && RELEASE_WORDS.contains(&head[start..].to_lowercase().as_str()) {
            cut = start;
        } else {
            break;
        }
    }
    Some(cut)
}

/// A key for telling titles apart: accents, case and punctuation set aside.
pub(crate) fn fold_key(text: &str) -> String {
    text.nfkd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase).filter(|c| c.is_alphanumeric()).collect()
}

/// Where a folder is, for the Sources list: "~/Movies", "Films on NAS", or
/// "On NAS" for a whole drive or share.
pub(crate) fn place(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("/Volumes/") {
        let mut parts = rest.split('/').filter(|part| !part.is_empty());
        let volume = parts.next().unwrap_or(rest);
        return match parts.next_back() {
            Some(folder) => format!("{folder} on {volume}"),
            None => format!("On {volume}"),
        };
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && let Some(rest) = path.strip_prefix(home.trim_end_matches('/'))
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return format!("~{rest}");
    }
    path.to_string()
}

/// A media folder source's folders, described for the Sources list.
pub(crate) fn describe(folders: &[String]) -> Option<String> {
    let first = place(folders.first()?);
    Some(match folders.len() {
        1 => first,
        count => format!("{first} and {} more", count - 1),
    })
}

/// The last part of a folder's path ("/Volumes/NAS/Films" → "Films").
pub(crate) fn folder_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().filter(|name| !name.is_empty()).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie(title: &str, year: Option<i64>) -> Option<Item> {
        Some(Item::Movie { title: title.into(), year })
    }

    fn episode(series: &str, season: i64, number: i64, title: Option<&str>) -> Option<Item> {
        Some(Item::Episode { series: series.into(), year: None, season, number, last: None, title: title.map(str::to_string) })
    }

    #[test]
    fn a_meaningless_name_takes_its_folders_unless_that_means_no_more() {
        assert_eq!(classify("Heat (1995)/0a1b2c3d4e5f60718293a4b5.mkv", "Films"), movie("Heat", Some(1995)));
        let hashed = classify("3174fb7a-9884-4147-986c-736748bf7691/01800d3d-f0ca-4d66-b9f7-f887f0c45ab0.mp4", "videos");
        assert_eq!(hashed, movie("01800d3d-f0ca-4d66-b9f7-f887f0c45ab0", None), "the file's own name, not its folder's");
    }

    #[test]
    fn cameras_and_phones_name_files_by_number() {
        for name in [
            "video 00001",
            "vid 00002",
            "IMG_1234",
            "MVI_0042",
            "GOPR0042",
            "GX010042",
            "DJI_0001",
            "DSC01234",
            "PXL_20240512_123456",
            "20240512_123456",
            "Screen Recording 2024-05-12 at 10.31.22",
            "Untitled 3",
            "FusionX 00001",
            "interpolated 00001",
            "ComfyUI_00001_",
        ] {
            assert!(camera_name(name), "{name}");
        }
        for name in ["Heat", "Video Killed the Radio Star", "VJ LOOPS 2019-1", "Movie 43", "Imgur Compilation", "2001 A Space Odyssey", "Blade Runner 2049", "Agent 007", "Apollo 13"] {
            assert!(!camera_name(name), "{name}");
        }
    }

    #[test]
    fn own_names_are_tidied_but_whole() {
        assert_eq!(own_name("VJ LOOPS 2019-1"), "VJ LOOPS 2019-1");
        assert_eq!(own_name("VJ_LOOPS_2019-1"), "VJ LOOPS 2019-1");
        assert_eq!(own_name("Talk.At.The.Library.2024"), "Talk At The Library 2024");
        assert_eq!(own_name("Mr. Robot  clip"), "Mr. Robot clip");
    }

    #[test]
    fn films_from_file_names() {
        for (path, title, year) in [
            ("Heat (1995).mkv", "Heat", Some(1995)),
            ("Arrival.2016.1080p.BluRay.x264-GROUP.mkv", "Arrival", Some(2016)),
            ("The Grand Budapest Hotel (2014) [1080p] [YTS.MX].mp4", "The Grand Budapest Hotel", Some(2014)),
            ("1917 (2019).mkv", "1917", Some(2019)),
            ("Mr. Robot Documentary.mp4", "Mr. Robot Documentary", None),
            ("Blade_Runner_2049_2160p_HDR.mkv", "Blade Runner 2049", None),
            ("Blade.Runner.2049.2017.2160p.UHD.BluRay.mkv", "Blade Runner 2049", Some(2017)),
            ("2001 A Space Odyssey (1968).mkv", "2001 A Space Odyssey", Some(1968)),
            ("Aliens.Extended.1080p.BluRay.mkv", "Aliens", None),
            ("Charlottes.Web.mkv", "Charlottes Web", None),
            ("Internal.Affairs.1990.720p.mkv", "Internal Affairs", Some(1990)),
            ("Amélie [2001].mkv", "Amélie", Some(2001)),
        ] {
            assert_eq!(classify(path, ""), movie(title, year), "{path}");
        }
    }

    #[test]
    fn films_take_what_their_folder_knows() {
        assert_eq!(classify("Films/Heat (1995)/Heat.mkv", "Media"), movie("Heat", Some(1995)));
        assert_eq!(classify("Heat (1995)/movie.mkv", ""), movie("Heat", Some(1995)));
        assert_eq!(classify("Arrival (2016)/3f2a9c4be1d8a7f6.mkv", ""), movie("Arrival", Some(2016)));
        // A folder chosen for one film names it too.
        assert_eq!(classify("title_t00.mkv", "Heat (1995)"), movie("Heat", Some(1995)));
        assert_eq!(classify("Heat.mkv", "Movies"), movie("Heat", None), "an unrelated folder's name isn't the title");
    }

    #[test]
    fn episodes_in_season_folders() {
        assert_eq!(
            classify("TV/Severance/Season 2/Severance.S02E03.Who.Is.Alive.1080p.mkv", ""),
            episode("Severance", 2, 3, Some("Who Is Alive"))
        );
        assert_eq!(classify("Severance/S02/S02E04 - Woe's Hollow.mkv", ""), episode("Severance", 2, 4, Some("Woe's Hollow")));
        assert_eq!(classify("Severance/Season 1/01 - Good News About Hell.mkv", ""), episode("Severance", 1, 1, Some("Good News About Hell")));
        assert_eq!(classify("Severance/Specials/E01 Making Of.mkv", ""), episode("Severance", 0, 1, Some("Making Of")));
        assert_eq!(classify("Severance/Season 1 (2022)/Episode 3.mkv", ""), episode("Severance", 1, 3, None));
        // The chosen folder is the series' own.
        assert_eq!(classify("Season 1/S01E02.mkv", "Shogun"), episode("Shogun", 1, 2, None));
    }

    #[test]
    fn episodes_named_on_their_own() {
        // In a folder of mixed shows, the file name names the series, not the folder.
        assert_eq!(classify("TV/The.Bear.S01E01.System.720p.WEB.mkv", ""), episode("The Bear", 1, 1, Some("System")));
        assert_eq!(classify("Fargo 3x05.avi", ""), episode("Fargo", 3, 5, None));
        // With no series in the name, the folder names it.
        assert_eq!(classify("Shogun/S01E02.mkv", ""), episode("Shogun", 1, 2, None));
    }

    #[test]
    fn series_keep_their_year() {
        let Some(Item::Episode { series, year, .. }) = classify("Doctor Who (2005)/Season 1/Doctor Who S01E01.mkv", "") else {
            panic!("not an episode")
        };
        assert_eq!((series.as_str(), year), ("Doctor Who", Some(2005)));
    }

    #[test]
    fn several_episodes_in_one_file() {
        for (path, last) in [
            ("Show/Season 1/Show S01E01E02.mkv", Some(2)),
            ("Show/Season 1/Show.S01E01-E02.mkv", Some(2)),
            ("Show/Season 1/Show S01E01-02.mkv", Some(2)),
            ("Show/Season 1/Show S01E01E02E03 Title.mkv", Some(3)),
            ("Show/Season 1/Show S01E01-720p.mkv", None),
            ("Show/Season 1/Show S01E01 Episode Name.mkv", None),
        ] {
            let Some(Item::Episode { number, last: found, .. }) = classify(path, "") else { panic!("{path} is not an episode") };
            assert_eq!((number, found), (1, last), "{path}");
        }
        let Some(Item::Episode { title, .. }) = classify("Show/Season 1/Show S01E01E02E03 Title.mkv", "") else { panic!() };
        assert_eq!(title.as_deref(), Some("Title"));
    }

    #[test]
    fn samples_and_extras_are_left_out() {
        assert_eq!(classify("Heat (1995)/Extras/Making of Heat.mkv", ""), None);
        assert_eq!(classify("Heat (1995)/Featurettes/Michael Mann.mp4", ""), None);
        assert_eq!(classify("Arrival (2016)/arrival-sample.mkv", ""), None);
        assert_eq!(classify("Arrival (2016)/Sample/arrival.mkv", ""), None);
        assert_eq!(classify("Severance/Season 1/Trailers/Teaser.mkv", ""), None);
        // At the top, "Shorts" is a shelf, not a film's extras.
        assert_eq!(classify("Shorts/Paperman (2012).mp4", ""), movie("Paperman", Some(2012)));
        assert_eq!(classify("Heat (1995)/Shorts/Something.mp4", ""), None);
    }

    #[test]
    fn season_folders() {
        for (name, season) in [("Season 2", Some(2)), ("S02", Some(2)), ("Series 3", Some(3)), ("Staffel 1", Some(1)), ("Saison 4", Some(4)), ("Specials", Some(0)), ("Season 1 (2008)", Some(1)), ("Seasoning", None), ("Heat", None)] {
            assert_eq!(season_number(name), season, "{name}");
        }
    }

    #[test]
    fn places_for_the_sources_list() {
        assert_eq!(place("/Volumes/NAS"), "On NAS");
        assert_eq!(place("/Volumes/NAS/Media/Films"), "Films on NAS");
        assert_eq!(place("/srv/films"), "/srv/films");
        assert_eq!(describe(&["/srv/a".into(), "/srv/b".into(), "/srv/c".into()]).as_deref(), Some("/srv/a and 2 more"));
        assert_eq!(folder_name("/Volumes/NAS/Films/"), "Films");
    }
}
