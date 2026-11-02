//! Subtitle files beside a video, found when it plays rather than when the
//! folder is scanned: "Heat.srt", "Heat.en.srt", "Heat.English.forced.srt",
//! and the same in a "Subs" folder, or any in "Subs/Heat/" (as some release
//! groups lay them out). Their names give their language.

use std::path::{Path, PathBuf};

use super::naming::{self, SUBTITLE_EXTENSIONS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sidecar {
    pub path: PathBuf,
    pub file_name: String,
    /// An ISO 639-2 code ("eng", "fre"), as [`xorvio_model::language_code`] gives them.
    pub language: Option<String>,
    /// Only the parts in another language, or signs.
    pub forced: bool,
}

/// Subtitle folders beside a video.
const SUBTITLE_FOLDERS: &[&str] = &["subs", "subtitles", "sub"];

/// The subtitle files for `video`: full ones before forced ones, and those
/// whose language is known first.
pub(crate) fn sidecars(video: &Path) -> Vec<Sidecar> {
    let (Some(folder), Some(stem)) = (video.parent(), video.file_stem().and_then(|stem| stem.to_str())) else { return Vec::new() };
    let stem = stem.to_lowercase();
    let mut found = Vec::new();
    for (path, name, is_dir) in entries(folder) {
        if is_dir && SUBTITLE_FOLDERS.contains(&name.to_lowercase().as_str()) {
            for (inner, inner_name, inner_dir) in entries(&path) {
                if inner_dir && inner_name.to_lowercase() == stem {
                    // Subs/Heat/2_English.srt: every one in there is Heat's, named by language.
                    found.extend(entries(&inner).into_iter().filter(|(_, _, dir)| !dir).filter_map(|(file, file_name, _)| {
                        let tags = naming::base_name(&file_name).to_string();
                        sidecar(file, file_name, &tags)
                    }));
                } else if !inner_dir {
                    found.extend(matching(inner, inner_name, &stem));
                }
            }
        } else if !is_dir {
            found.extend(matching(path, name, &stem));
        }
    }
    found.sort_by(|a, b| {
        (a.forced, a.language.is_none(), &a.language, &a.file_name).cmp(&(b.forced, b.language.is_none(), &b.language, &b.file_name))
    });
    found
}

/// What's in a folder (path, name, whether it's a folder), hidden files left out.
fn entries(folder: &Path) -> Vec<(PathBuf, String, bool)> {
    let Ok(listing) = std::fs::read_dir(folder) else { return Vec::new() };
    listing
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            if name.starts_with('.') {
                return None;
            }
            let is_dir = std::fs::metadata(entry.path()).ok()?.is_dir();
            Some((entry.path(), name, is_dir))
        })
        .collect()
}

/// A subtitle file named after the video: its name, then tags ("Heat.en.forced.srt").
fn matching(path: PathBuf, name: String, stem: &str) -> Option<Sidecar> {
    let lower = name.to_lowercase();
    let rest = lower.strip_prefix(stem)?;
    if !rest.starts_with('.') {
        return None;
    }
    let tags = naming::base_name(&name).get(stem.len()..).unwrap_or_default().to_string();
    sidecar(path, name, &tags)
}

fn sidecar(path: PathBuf, file_name: String, tags: &str) -> Option<Sidecar> {
    if !naming::has_extension(&file_name, SUBTITLE_EXTENSIONS) {
        return None;
    }
    let (language, forced) = read_tags(tags);
    Some(Sidecar { path, file_name, language, forced })
}

/// The language and the forced mark in a subtitle's tags (".en.forced",
/// "_English", ".pt-BR.sdh").
pub(crate) fn read_tags(tags: &str) -> (Option<String>, bool) {
    let words: Vec<String> =
        tags.split(|c: char| ".-_ ()[]".contains(c)).filter(|word| !word.is_empty()).map(str::to_lowercase).collect();
    let forced = words.iter().any(|word| word == "forced" || word == "foreign");
    let language = words.iter().find_map(|word| language(word));
    // "hi" usually marks hearing-impaired subtitles; it's Hindi only on its own.
    let language = language.or_else(|| words.iter().any(|word| word == "hi").then(|| "hin".to_string()));
    (language, forced)
}

/// Three-letter codes taken as languages; others ("sdh", "dts") aren't.
const CODES: &[&str] = &[
    "eng", "fre", "fra", "spa", "ger", "deu", "ita", "por", "pob", "dut", "nld", "swe", "nor", "nob", "nno", "dan", "fin", "pol", "rus",
    "ukr", "tur", "gre", "ell", "ara", "heb", "hin", "jpn", "kor", "chi", "zho", "cze", "ces", "slo", "slk", "hun", "rum", "ron", "bul",
    "hrv", "srp", "slv", "per", "fas", "tha", "vie", "ind", "may", "msa", "ice", "isl", "est", "lav", "lit", "cat", "baq", "eus", "gle",
    "wel", "cym", "alb", "sqi", "mac", "mkd", "arm", "hye", "geo", "kat", "tam", "tel", "ben", "urd", "fil", "tgl",
];
/// Two-letter codes taken as languages ("hi" is read above).
const SHORT_CODES: &[&str] = &[
    "en", "fr", "es", "de", "it", "pt", "pb", "nl", "sv", "no", "nb", "da", "fi", "pl", "ru", "uk", "tr", "el", "ar", "he", "ja", "ko",
    "zh", "cs", "sk", "hu", "ro", "bg", "hr", "sr", "sl", "fa", "th", "vi", "id", "ms", "is", "et", "lv", "lt", "ca", "eu", "ga", "cy",
    "sq", "mk", "hy", "ka", "ta", "te", "bn", "ur",
];
const NAMES: &[(&str, &str)] = &[
    ("english", "eng"), ("french", "fre"), ("francais", "fre"), ("français", "fre"), ("spanish", "spa"), ("espanol", "spa"),
    ("español", "spa"), ("castellano", "spa"), ("latino", "spa"), ("german", "ger"), ("deutsch", "ger"), ("italian", "ita"),
    ("italiano", "ita"), ("portuguese", "por"), ("portugues", "por"), ("português", "por"), ("brazilian", "por"), ("dutch", "dut"),
    ("nederlands", "dut"), ("swedish", "swe"), ("svenska", "swe"), ("norwegian", "nor"), ("norsk", "nor"), ("danish", "dan"),
    ("dansk", "dan"), ("finnish", "fin"), ("suomi", "fin"), ("polish", "pol"), ("polski", "pol"), ("russian", "rus"), ("ukrainian", "ukr"),
    ("turkish", "tur"), ("greek", "gre"), ("arabic", "ara"), ("hebrew", "heb"), ("hindi", "hin"), ("japanese", "jpn"), ("korean", "kor"),
    ("chinese", "chi"), ("mandarin", "chi"), ("cantonese", "chi"), ("czech", "cze"), ("slovak", "slo"), ("hungarian", "hun"),
    ("romanian", "rum"), ("bulgarian", "bul"), ("croatian", "hrv"), ("serbian", "srp"), ("slovenian", "slv"), ("persian", "per"),
    ("farsi", "per"), ("thai", "tha"), ("vietnamese", "vie"), ("indonesian", "ind"), ("malay", "may"), ("icelandic", "ice"),
    ("estonian", "est"), ("latvian", "lav"), ("lithuanian", "lit"), ("catalan", "cat"),
];

fn language(word: &str) -> Option<String> {
    if let Some((_, code)) = NAMES.iter().find(|(name, _)| *name == word) {
        return Some(code.to_string());
    }
    let known = match word.len() {
        2 => SHORT_CODES.contains(&word),
        3 => CODES.contains(&word),
        _ => false,
    };
    known.then(|| xorvio_model::language_code(word)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_from_names() {
        for (tags, language, forced) in [
            (".en", Some("eng"), false),
            (".eng", Some("eng"), false),
            (".English", Some("eng"), false),
            (".fr.forced", Some("fre"), true),
            (".pt-BR.sdh", Some("por"), false),
            (".en.hi", Some("eng"), false),
            (".hi", Some("hin"), false),
            ("2_English", Some("eng"), false),
            ("", None, false),
            (".sdh", None, false),
            (".final", None, false),
        ] {
            assert_eq!(read_tags(tags), (language.map(str::to_string), forced), "{tags}");
        }
    }

    #[test]
    fn subtitles_beside_a_video() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        for file in [
            "Heat.mkv",
            "Heat.srt",
            "Heat.en.forced.srt",
            "Heat.fr.ass",
            "Heat 2.srt",
            "Heat.nfo",
            ".Heat.de.srt",
            "Subs/Heat.es.vtt",
            "Subs/Heat/2_German.srt",
            "Subs/Other.it.srt",
        ] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        let found = sidecars(&root.join("Heat.mkv"));
        let summary: Vec<(&str, Option<&str>, bool)> =
            found.iter().map(|s| (s.file_name.as_str(), s.language.as_deref(), s.forced)).collect();
        assert_eq!(
            summary,
            [
                ("Heat.fr.ass", Some("fre"), false),
                ("2_German.srt", Some("ger"), false),
                ("Heat.es.vtt", Some("spa"), false),
                ("Heat.srt", None, false),
                ("Heat.en.forced.srt", Some("eng"), true),
            ]
        );
    }
}
