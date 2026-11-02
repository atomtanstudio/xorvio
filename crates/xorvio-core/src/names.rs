//! Smart name cleanup. Provider names often carry country prefixes and quality
//! tags ("UK | Channel One FHD"). Cleanup is a display transformation only: the
//! catalog always keeps the provider's original name, so a rule that tidies
//! too much can be fixed without a refresh.

use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("built-in pattern")
}

const QUALITY_TAGS: &[&str] = &[
    "HD", "FHD", "UHD", "SD", "4K", "8K", "HEVC", "H265", "H.265", "H264", "1080P", "720P", "2160P", "HDR", "50FPS",
    "60FPS", "RAW", "BACKUP", "VIP", "MULTI",
];

fn is_quality_tag(word: &str, strip: &[char]) -> bool {
    let upper = word.to_uppercase();
    QUALITY_TAGS.contains(&upper.trim_matches(|c| strip.contains(&c)))
}

/// Drops quality tags from the end, always keeping at least one word.
fn drop_trailing_quality_tags(name: &str, strip: &[char]) -> String {
    let mut words: Vec<&str> = name.split(' ').filter(|word| !word.is_empty()).collect();
    while words.len() > 1 && words.last().is_some_and(|last| is_quality_tag(last, strip)) {
        words.pop();
    }
    words.join(" ")
}

fn trim_separators(text: &str) -> &str {
    text.trim_matches(|c| " |-:".contains(c))
}

static CHANNEL_PREFIXES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [r"^\|[A-Za-z]{2,4}\|\s*", r"^\[[A-Za-z]{2,4}\]\s*", r"^[A-Za-z]{2,4}\s*[:|]\s+", r"^[A-Za-z]{2,4}\s*-\s+"]
        .into_iter()
        .map(regex)
        .collect()
});
static BRACKETED_QUALITY: LazyLock<Regex> =
    LazyLock::new(|| regex(r"\s*[\[\(](?i:hd|fhd|uhd|sd|4k|hevc|hdr|raw|vip)[\]\)]"));

/// A tidied channel or category name. Never returns an empty string.
pub fn display_name(raw: &str) -> String {
    let original = raw.trim();
    let mut name = original.to_string();

    // A leading "XX:", "XX |", "|XX|" or "[XX]" country or group prefix (2–4 letters).
    for pattern in CHANNEL_PREFIXES.iter() {
        if let Some(found) = pattern.find(&name) {
            let candidate = name[found.end()..].to_string();
            if !candidate.is_empty() {
                name = candidate;
            }
            break;
        }
    }

    // Bracketed or trailing quality tags.
    name = BRACKETED_QUALITY.replace_all(&name, "").into_owned();
    name = drop_trailing_quality_tags(&name, &['|', '-']);
    let name = trim_separators(&name);
    if name.is_empty() { original.to_string() } else { name.to_string() }
}

static MEDIA_BRACKET_TAGS: LazyLock<[Regex; 2]> =
    LazyLock::new(|| [regex(r"^\|[A-Z0-9]{2,4}\|\s*"), regex(r"^\[[A-Z0-9]{2,4}\]\s*")]);
// The `regex` crate has no lookahead; the conditions Swift wrote as lookaheads
// are checked in code next to these patterns.
static MEDIA_TAG: LazyLock<Regex> = LazyLock::new(|| regex(r"^[A-Z0-9+]{1,6}(?:-[A-Z0-9+]{1,6}){0,2}\s*[-:|]\s+"));
static MEDIA_TAG_BEFORE_NUMBER: LazyLock<Regex> = LazyLock::new(|| regex(r"^[A-Z0-9+]{1,6}(?:-[A-Z0-9+]{1,6}){0,2}\s+-"));
static LIST_NUMBER: LazyLock<Regex> = LazyLock::new(|| regex(r"^\d{1,3}\."));
static LEADING_NUMBER_DOT: LazyLock<Regex> = LazyLock::new(|| regex(r"^\d{1,3}\."));
static TRAILING_YEAR_OR_COUNTRY: LazyLock<Regex> = LazyLock::new(|| regex(r"\s*\((?:(?:19|20)\d{2}|[A-Z]{2})\)\s*$"));

/// Tags need a letter, so "1917 - Extended Cut" keeps its year-like title.
fn tag_has_letter(text: &str) -> bool {
    text.chars()
        .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '+' || *c == '-')
        .any(|c| c.is_ascii_uppercase())
}

/// Where a leading provider tag ends, if `name` starts with one.
fn media_tag_end(name: &str, step: usize) -> Option<usize> {
    match step {
        0 | 1 => MEDIA_BRACKET_TAGS[step].find(name).map(|found| found.end()),
        2 => MEDIA_TAG.find(name).filter(|_| tag_has_letter(name)).map(|found| found.end()),
        // The same tag with no space after the dash, when a list number follows ("EN-TOP -141.Top Gun").
        3 => MEDIA_TAG_BEFORE_NUMBER
            .find(name)
            .filter(|found| tag_has_letter(name) && LIST_NUMBER.is_match(&name[found.end()..]))
            .map(|found| found.end()),
        // A provider's list position ("212. Logan"), but not a title like "2.0".
        _ => {
            let found = LEADING_NUMBER_DOT.find(name)?;
            let next = name[found.end()..].chars().next()?;
            (next.is_whitespace() || next.is_ascii_alphabetic())
                .then(|| found.end() + (name[found.end()..].len() - name[found.end()..].trim_start().len()))
        }
    }
}

/// Film and series titles: removes uppercase provider tags in front of the
/// title ("4K-TOP - ", "NF - ", "EN - ", "|EN| ") and trailing quality tags.
/// Only all-caps tags are removed, so "X-Men - Apocalypse" is left alone.
pub fn media_title(raw: &str) -> String {
    let original = raw.trim();
    let mut name = original.to_string();
    for _ in 0..2 {
        for step in 0..5 {
            if let Some(end) = media_tag_end(&name, step) {
                let candidate = &name[end..];
                if candidate.chars().count() >= 2 {
                    name = candidate.to_string();
                }
            }
        }
    }
    // Dots for spaces ("Top.Gun.Maverick."), from file names.
    if !name.contains(' ') && name.matches('.').count() >= 2 {
        name = name.replace('.', " ").trim().to_string();
    }
    // A stray full stop at the end ("Braveheart."), unless the title uses them
    // throughout ("Oh. What. Fun.") or it ends an abbreviation.
    if name.ends_with('.')
        && name.matches('.').count() == 1
        && !["Jr.", "Dr.", "St."].iter().any(|suffix| name.ends_with(suffix))
    {
        name.pop();
    }
    // Trailing "(1999)" and "(US)" tags: the year is shown separately and the
    // country is used for filtering, not display.
    while let Some(found) = TRAILING_YEAR_OR_COUNTRY.find(&name) {
        if found.start() == 0 {
            break;
        }
        name.truncate(found.start());
    }
    let name = drop_trailing_quality_tags(&name, &['|', '-', '(', ')', '[', ']']);
    let cleaned = trim_separators(&name);
    if cleaned.is_empty() { original.to_string() } else { cleaned.to_string() }
}

static COUNTRY_TAG: LazyLock<Regex> = LazyLock::new(|| regex(r"\(([A-Z]{2})\)\s*(?:\((?:19|20)\d{2}\)\s*)?$"));

/// The two-letter country tag at the end of a title ("Below (2026) (GB)" → "GB").
pub fn country_tag(raw: &str) -> Option<String> {
    COUNTRY_TAG.captures(raw).map(|captures| captures[1].to_string())
}

/// A key for matching names (guide fallback by name, stable playlist ids):
/// tidied, lowercased, accents and width folded, punctuation dropped.
pub fn match_key(raw: &str) -> String {
    display_name(raw)
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

// MARK: Versions of one channel

/// Picture quality as a channel's name gives it. A name with no tag ranks
/// just above "SD": providers usually leave their main version untagged and
/// mark the lesser ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Quality {
    Sd,
    Untagged,
    Hd,
    FullHd,
    Uhd,
}

impl Quality {
    /// What the Versions list shows.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::Sd => Some("SD"),
            Self::Untagged => None,
            Self::Hd => Some("HD"),
            Self::FullHd => Some("FHD"),
            Self::Uhd => Some("4K"),
        }
    }

    /// The quality an upper-case word names, if it names one.
    fn of_word(word: &str) -> Option<Self> {
        match word {
            "4K" | "8K" | "UHD" | "2160P" => Some(Self::Uhd),
            "FHD" | "FULLHD" | "1080P" | "1080I" => Some(Self::FullHd),
            "HD" | "720P" => Some(Self::Hd),
            "SD" | "576P" | "576I" | "480P" => Some(Self::Sd),
            _ => None,
        }
    }
}

/// Tags that mark a version rather than name a channel: quality and codec
/// tags beyond [`QUALITY_TAGS`], and backup feeds.
const VERSION_TAGS: &[&str] = &[
    "FULLHD", "1080I", "576P", "576I", "480P", "HQ", "H.264", "AVC", "X265", "10BIT", "HDR10", "HFR", "25FPS", "SDR",
];
const BACKUP_TAGS: &[&str] = &["BACKUP", "BKP", "ALT", "ALTERNATIVE", "ALTERNATE", "SECOURS"];

fn is_version_tag(upper: &str) -> bool {
    QUALITY_TAGS.contains(&upper) || VERSION_TAGS.contains(&upper) || BACKUP_TAGS.contains(&upper)
}

/// A channel's name read as one version of a channel the provider lists
/// several times ("News 24 HD", "News 24 FHD", "US: News 24", "News 24 (backup)").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionName {
    /// The name the versions share ("News 24").
    pub name: String,
    /// `name` for comparing: case, accents and punctuation set aside, but
    /// "+1" (a timeshift channel) kept apart from "1".
    pub key: String,
    pub quality: Quality,
    /// Marked as a backup or alternative feed.
    pub backup: bool,
}

static VERSION_PREFIXES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    // display_name's prefixes, then upper-case ones written without a space
    // ("US:News 24") and quality prefixes ("4K | News 24").
    let mut patterns: Vec<Regex> = CHANNEL_PREFIXES.iter().cloned().collect();
    patterns.push(regex(r"^(?:[A-Z]{2,4}|[48]K)\s*[:|]\s*"));
    patterns
});
static FULL_HD: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)\bfull[\s-]?hd\b"));

/// Superscript tags written onto the name ("News 24ᴴᴰ") get a space before
/// them, so that once normalised they read as a word of their own.
fn superscripts_apart(raw: &str) -> String {
    let is_superscript =
        |c: char| matches!(c, '\u{1D2C}'..='\u{1D6A}' | '\u{1D9B}'..='\u{1DBF}' | '\u{2070}'..='\u{209F}' | '\u{02B0}'..='\u{02FF}' | '²' | '³' | '¹');
    let mut text = String::with_capacity(raw.len() + 2);
    let mut previous: Option<char> = None;
    for c in raw.chars() {
        if is_superscript(c) && previous.is_some_and(|p| !p.is_whitespace() && !is_superscript(p)) {
            text.push(' ');
        }
        text.push(c);
        previous = Some(c);
    }
    text
}

fn is_separator(word: &str) -> bool {
    word.chars().all(|c| "|-:/·•".contains(c))
}

/// The words inside a bracket mark a version ("(FHD)", "[4K HEVC]",
/// "(backup 2)"), so the bracket can go.
fn brackets_a_version(inner: &str) -> bool {
    let words: Vec<String> = inner
        .split(|c: char| c.is_whitespace() || "/,|+-".contains(c))
        .filter(|word| !word.is_empty())
        .map(str::to_uppercase)
        .collect();
    let tagged = words.iter().filter(|word| is_version_tag(word)).count();
    tagged > 0 && words.iter().all(|word| is_version_tag(word) || word.chars().all(|c| c.is_ascii_digit()))
}

/// Removes brackets that only hold version tags.
fn remove_version_brackets(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(open) = rest.find(['(', '[', '{']) {
        let closer = match rest.as_bytes()[open] {
            b'(' => ')',
            b'[' => ']',
            _ => '}',
        };
        let Some(length) = rest[open + 1..].find(closer) else { break };
        let inner = &rest[open + 1..open + 1 + length];
        result.push_str(&rest[..open]);
        if !brackets_a_version(inner) {
            result.push_str(&rest[open..open + length + 2]);
        }
        rest = &rest[open + length + 2..];
    }
    result.push_str(rest);
    result
}

/// Reads a channel's name as a version of a channel: what the versions
/// share, its quality and whether it's a backup. Like [`display_name`] it
/// never returns an empty name.
pub fn version_name(raw: &str) -> VersionName {
    let text: String = superscripts_apart(raw.trim()).nfkc().collect();
    let text = FULL_HD.replace_all(&text, "FHD");

    // Quality and backup marks count wherever they are.
    let mut quality = None;
    let mut backup = false;
    for word in text.split(|c: char| !(c.is_alphanumeric() || c == '.')).filter(|word| !word.is_empty()) {
        let upper = word.to_uppercase();
        quality = quality.max(Quality::of_word(&upper));
        backup |= BACKUP_TAGS.contains(&upper.as_str());
    }

    // The name without a country or quality prefix, bracketed tags and
    // trailing tags ("Backup 2" included).
    let mut name: &str = &text;
    for _ in 0..2 {
        if let Some(found) = VERSION_PREFIXES.iter().find_map(|pattern| pattern.find(name))
            && found.end() < name.len()
        {
            name = &name[found.end()..];
        }
    }
    let name = remove_version_brackets(name);
    let mut words: Vec<&str> = name.split_whitespace().collect();
    while words.len() > 1 {
        let last = words[words.len() - 1].trim_matches(|c| "|-:([])".contains(c)).to_uppercase();
        let numbered_backup = last.chars().all(|c| c.is_ascii_digit())
            && BACKUP_TAGS.contains(&words[words.len() - 2].trim_matches(|c| "|-:([])".contains(c)).to_uppercase().as_str());
        if is_separator(words[words.len() - 1]) || is_version_tag(&last) {
            words.pop();
        } else if numbered_backup {
            words.truncate(words.len() - 2);
        } else {
            break;
        }
    }
    let joined = words.join(" ");
    let name = match trim_separators(&joined) {
        "" => display_name(raw),
        name => name.to_string(),
    };
    let key = name
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric() || *c == '+')
        .collect();
    VersionName { name, key, quality: quality.unwrap_or(Quality::Untagged), backup }
}

static EPISODE_PATTERNS: LazyLock<[Regex; 2]> = LazyLock::new(|| {
    [regex(r"^(.*?)[\s._-]+[Ss](\d{1,2})\s*[Ee](\d{1,3})\b"), regex(r"^(.*?)[\s._-]+(\d{1,2})x(\d{1,3})\b")]
});

/// Reads "Show Name S01E02", "Show Name S01 E02" and "Show Name 1x02" as
/// (series, season, episode).
pub fn episode_info(raw: &str) -> Option<(String, i64, i64)> {
    for pattern in EPISODE_PATTERNS.iter() {
        let Some(captures) = pattern.captures(raw) else { continue };
        let series = captures[1].trim_matches(|c| " -_.".contains(c));
        let (Ok(season), Ok(episode)) = (captures[2].parse(), captures[3].parse()) else { continue };
        if !series.is_empty() {
            return Some((series.to_string(), season, episode));
        }
    }
    None
}

static TRAILING_YEAR: LazyLock<Regex> = LazyLock::new(|| regex(r"^(.*?)[\s\-]*[\(\[]?((?:19|20)\d{2})[\)\]]?\s*$"));

/// Splits a trailing "(1999)" or " - 1999" year from a title.
pub fn split_year(raw: &str) -> (String, Option<i64>) {
    let Some(captures) = TRAILING_YEAR.captures(raw) else { return (raw.to_string(), None) };
    let title = captures[1].trim();
    if title.is_empty() {
        return (raw.to_string(), None);
    }
    (title.to_string(), captures[2].parse().ok())
}

/// Some guides mark live broadcasts in the title ("Live: NBA Basketball").
/// The title keeps the name; the marker becomes the programme's live flag.
pub fn live_marker(title: &str) -> (String, bool) {
    const PREFIX: &str = "live: ";
    let is_marked = title.chars().count() > PREFIX.len()
        && title.get(..PREFIX.len()).is_some_and(|start| start.eq_ignore_ascii_case(PREFIX));
    if is_marked { (title[PREFIX.len()..].trim().to_string(), true) } else { (title.to_string(), false) }
}

/// "S01 E03" for an episode's season and number.
pub fn episode_code(season: i64, number: i64) -> String {
    format!("S{season:02} E{number:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_display_names() {
        for (raw, expected) in [
            ("UK | Channel One FHD", "Channel One"),
            ("|US| News 24 HD", "News 24"),
            ("[FR] Canal Info", "Canal Info"),
            ("Sports 2 (HD)", "Sports 2"),
            ("HD", "HD"),
            ("  ", ""),
        ] {
            assert_eq!(display_name(raw), expected, "{raw}");
        }
    }

    #[test]
    fn cleans_media_titles() {
        for (raw, expected) in [
            ("4K-TOP - The Glass Valley", "The Glass Valley"),
            ("NF - Tidewater", "Tidewater"),
            ("|EN| Paper Boats", "Paper Boats"),
            ("EN-TOP -141.Top Gun", "Top Gun"),
            ("212. Logan", "Logan"),
            ("2.0", "2.0"),
            ("1917 - Extended Cut", "1917 - Extended Cut"),
            ("X-Men - Apocalypse", "X-Men - Apocalypse"),
            ("Top.Gun.Maverick.", "Top Gun Maverick"),
            ("Braveheart.", "Braveheart"),
            ("Oh. What. Fun.", "Oh. What. Fun."),
            ("Below (2026) (GB)", "Below"),
            ("Marrow Street 4K", "Marrow Street"),
            ("(2020)", "(2020)"),
        ] {
            assert_eq!(media_title(raw), expected, "{raw}");
        }
    }

    #[test]
    fn country_tags() {
        assert_eq!(country_tag("Below (2026) (GB)").as_deref(), Some("GB"));
        assert_eq!(country_tag("Below (GB) (2026)").as_deref(), Some("GB"));
        assert_eq!(country_tag("Below"), None);
    }

    #[test]
    fn match_keys_fold_case_accents_and_tags() {
        assert_eq!(match_key("UK | Café Télé HD"), "cafetele");
        assert_eq!(match_key("Ｃｈａｎｎｅｌ　One"), match_key("channel one"));
    }

    #[test]
    fn versions_of_a_channel_share_a_name() {
        for raw in [
            "Harbor News",
            "Harbor News HD",
            "Harbor News FHD",
            "Harbor News 4K",
            "Harbor News UHD HEVC",
            "Harbor News Full HD",
            "Harbor News 1080p 50FPS",
            "US: Harbor News",
            "US:Harbor News",
            "UK | Harbor News FHD",
            "|UK| Harbor News",
            "[FR] Harbor News",
            "4K | Harbor News",
            "Harbor News (backup)",
            "Harbor News [FHD]",
            "Harbor News (HD/SD)",
            "Harbor News | HD | Backup",
            "Harbor News Backup 2",
            "Harbor News ALT",
            "Harbor News ᴴᴰ",
            "Harbor Newsᴴᴰ",
            "Harbor News ᵁᴴᴰ",
            "harbor news  hd",
            "Ｈａｒｂｏｒ Ｎｅｗｓ",
        ] {
            let version = version_name(raw);
            assert_eq!(version.key, "harbornews", "{raw}");
            assert!(version.name.eq_ignore_ascii_case("Harbor News"), "{raw} → {}", version.name);
        }
    }

    #[test]
    fn versions_carry_their_quality_and_backup_marks() {
        for (raw, quality, backup) in [
            ("Summit Sports 4K", Quality::Uhd, false),
            ("Summit Sports UHD", Quality::Uhd, false),
            ("Summit Sports ᵁᴴᴰ", Quality::Uhd, false),
            ("Summit Sports 2160p", Quality::Uhd, false),
            ("Summit Sports FHD", Quality::FullHd, false),
            ("Summit Sports Full HD", Quality::FullHd, false),
            ("Summit Sports 1080p", Quality::FullHd, false),
            ("Summit Sports HD", Quality::Hd, false),
            ("Summit Sports ᴴᴰ", Quality::Hd, false),
            ("Summit Sports 720p", Quality::Hd, false),
            ("Summit Sports SD", Quality::Sd, false),
            ("Summit Sports", Quality::Untagged, false),
            ("4K | Summit Sports", Quality::Uhd, false),
            ("Summit Sports HD (backup)", Quality::Hd, true),
            ("Summit Sports BKP", Quality::Untagged, true),
        ] {
            let version = version_name(raw);
            assert_eq!((version.quality, version.backup), (quality, backup), "{raw}");
        }
        assert!(Quality::Uhd > Quality::FullHd && Quality::FullHd > Quality::Hd && Quality::Hd > Quality::Untagged);
        assert!(Quality::Untagged > Quality::Sd, "an untagged main version beats one marked SD");
        assert_eq!(Quality::FullHd.label(), Some("FHD"));
    }

    #[test]
    fn different_channels_stay_apart() {
        let key = |raw: &str| version_name(raw).key;
        assert_ne!(key("Summit Sports"), key("Summit Sports 2"));
        assert_ne!(key("Summit Sports +1"), key("Summit Sports 1"), "a timeshift channel isn't a version");
        assert_ne!(key("Summit Sports"), key("Summit Sports +1"));
        assert_ne!(key("Harbor News (US)"), key("Harbor News"), "a country at the end may be a different feed");
        assert_eq!(version_name("HD").name, "HD", "a name is never emptied");
        assert_eq!(version_name("Studio (Live)").name, "Studio (Live)", "brackets that aren't tags stay");
    }

    #[test]
    fn parses_episode_numbers() {
        assert_eq!(episode_info("Tidewater S02E05"), Some(("Tidewater".into(), 2, 5)));
        assert_eq!(episode_info("Tidewater S01 E01").map(|e| e.2), Some(1));
        assert_eq!(episode_info("Tidewater 2x05").map(|e| e.1), Some(2));
        assert_eq!(episode_info("A Film About Nothing"), None);
        assert_eq!(episode_info("S01E01"), None);
    }

    #[test]
    fn splits_years() {
        assert_eq!(split_year("The Glass Valley (2018)"), ("The Glass Valley".into(), Some(2018)));
        assert_eq!(split_year("Copper Skies - 2021"), ("Copper Skies".into(), Some(2021)));
        assert_eq!(split_year("2012"), ("2012".into(), None));
        assert_eq!(split_year("Plain"), ("Plain".into(), None));
    }

    #[test]
    fn guide_titles_lose_their_live_marker() {
        assert_eq!(live_marker("Live: NBA Basketball"), ("NBA Basketball".into(), true));
        assert_eq!(live_marker("LIVE: Premier League"), ("Premier League".into(), true));
        assert_eq!(live_marker("Live and Kicking"), ("Live and Kicking".into(), false));
        assert_eq!(live_marker("Live: "), ("Live: ".into(), false));
        assert_eq!(live_marker("Liv€"), ("Liv€".into(), false));
    }

    #[test]
    fn episode_codes() {
        assert_eq!(episode_code(1, 3), "S01 E03");
        assert_eq!(episode_code(12, 104), "S12 E104");
    }
}
