//! What a provider's category name says about what's in it: languages and
//! regions ("US| NEWS", "QC - FILMS", "TURKISH SERIES"), genres ("EN -
//! ACTION", "24/7 COMEDY") and adult content. Library profiles (profiles.rs)
//! filter by these. Names are read as the provider sends them, before any
//! tidying, since the tags tidying removes are what say the language.

use std::collections::{BTreeSet, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;
use xorvio_model::{ContentGenre, ContentLanguage};

use crate::parental;

/// What a category's name says about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CategoryTraits {
    pub languages: BTreeSet<ContentLanguage>,
    pub genres: BTreeSet<ContentGenre>,
    pub adult: bool,
}

/// Country and language codes as providers write them before a name ("US|",
/// "EN -", "(UK)", "|FR|").
pub(crate) const CODES: &[(&str, ContentLanguage)] = {
    use ContentLanguage::*;
    &[
        ("US", English), ("USA", English), ("UK", English), ("GB", English), ("EN", English), ("ENG", English),
        ("CA", English), ("AU", English), ("AUS", English), ("NZ", English), ("IE", English), ("IRL", English),
        ("ZA", English), ("SCO", English),
        ("FR", French), ("FRA", French), ("QC", French), ("BE", French), ("CH", German),
        ("ES", Spanish), ("ESP", Spanish), ("MX", Spanish), ("LAT", Spanish), ("CO", Spanish), ("CL", Spanish),
        ("PE", Spanish), ("VE", Spanish),
        ("PT", Portuguese), ("BR", Portuguese), ("DE", German), ("GER", German), ("AT", German), ("IT", Italian),
        ("ITA", Italian), ("NL", Dutch), ("SE", Nordic), ("SW", Nordic), ("NO", Nordic), ("DK", Nordic),
        ("FI", Nordic), ("IS", Nordic), ("PL", Polish), ("TR", Turkish), ("AR", Arabic), ("ARB", Arabic),
        ("IN", SouthAsian), ("PK", SouthAsian), ("BD", SouthAsian), ("RU", Russian), ("UA", Ukrainian), ("GR", Greek),
        ("AL", Albanian), ("RS", Balkan), ("HR", Balkan), ("BA", Balkan), ("SI", Balkan), ("MK", Balkan),
        ("EXYU", Balkan), ("HU", Hungarian), ("RO", Romanian), ("CZ", CzechSlovak), ("SK", CzechSlovak),
        ("BG", Bulgarian), ("IR", Persian), ("IL", Hebrew), ("KR", Korean), ("JP", Japanese), ("CN", Chinese),
        ("HK", Chinese), ("TW", Chinese), ("PH", Filipino), ("VN", Vietnamese), ("TH", Thai), ("NG", African),
        ("KE", African), ("GH", African),
    ]
};

/// A language code at the end overrides a regional one at the start: "CA| GENERAL FR".
const TRAILING_CODES: &[(&str, ContentLanguage)] = {
    use ContentLanguage::*;
    &[("EN", English), ("FR", French), ("ES", Spanish), ("DE", German), ("IT", Italian), ("PT", Portuguese), ("AR", Arabic), ("TR", Turkish)]
};

/// Languages spelled out anywhere in a name, misspellings providers use included.
const WORDS: &[(&str, ContentLanguage)] = {
    use ContentLanguage::*;
    &[
        ("ENGLISH", English), ("FRENCH", French), ("FRANCAIS", French), ("QUEBEC", French), ("SPANISH", Spanish),
        ("ESPANOL", Spanish), ("LATINO", Spanish), ("PORTUGUESE", Portuguese), ("BRAZIL", Portuguese),
        ("GERMAN", German), ("DEUTSCH", German), ("ITALIAN", Italian), ("ITALIANO", Italian), ("DUTCH", Dutch),
        ("SWEDISH", Nordic), ("NORWEGIAN", Nordic), ("DANISH", Nordic), ("FINNISH", Nordic), ("NORDIC", Nordic),
        ("POLISH", Polish), ("POLSKA", Polish), ("TURKISH", Turkish), ("TURKSIH", Turkish), ("TURK", Turkish),
        ("ARABIC", Arabic), ("HINDI", SouthAsian), ("PUNJABI", SouthAsian), ("TAMIL", SouthAsian),
        ("TELUGU", SouthAsian), ("URDU", SouthAsian), ("BANGLA", SouthAsian), ("RUSSIAN", Russian),
        ("UKRAINIAN", Ukrainian), ("GREEK", Greek), ("ALBANIAN", Albanian), ("SERBIAN", Balkan),
        ("CROATIAN", Balkan), ("BOSNIAN", Balkan), ("HUNGARIAN", Hungarian), ("MAGYAR", Hungarian),
        ("ROMANIAN", Romanian), ("CZECH", CzechSlovak), ("SLOVAK", CzechSlovak), ("BULGARIAN", Bulgarian),
        ("PERSIAN", Persian), ("FARSI", Persian), ("HEBREW", Hebrew), ("KOREAN", Korean), ("KDRAMA", Korean),
        ("JAPANESE", Japanese), ("CHINESE", Chinese), ("FILIPINO", Filipino), ("PINOY", Filipino),
        ("VIETNAMESE", Vietnamese), ("THAI", Thai), ("AFRICAN", African), ("NOLLYWOOD", African),
    ]
};

/// Words that mark a genre in a category name or a title's genre field.
/// Ones with a space or hyphen are looked for anywhere in the name.
pub(crate) fn genre_keywords(genre: ContentGenre) -> &'static [&'static str] {
    use ContentGenre::*;
    match genre {
        Action => &["ACTION"],
        Adventure => &["ADVENTURE"],
        Animation => &["ANIMATION", "ANIMATED", "CARTOON", "CARTOONS", "DREAMWORKS", "PIXAR"],
        Comedy => &["COMEDY", "COMEDIE", "STAND-UP", "STANDUP"],
        Crime => &["CRIME"],
        Documentary => &["DOCUMENTARY", "DOCUMENTARIES", "DOCU", "DOCUMENTAIRE", "DOCS"],
        Drama => &["DRAMA"],
        Family => &["FAMILY"],
        Fantasy => &["FANTASY"],
        Horror => &["HORROR"],
        Kids => &["KIDS", "KID", "CHILDREN", "JEUNESSE", "NICKELODEON"],
        Music => &["MUSIC", "CONCERT", "CONCERTS", "MUSICAL", "MUSIQUE"],
        Mystery => &["MYSTERY"],
        News => &["NEWS"],
        Reality => &["REALITY"],
        Romance => &["ROMANCE", "ROMANTIC"],
        ScienceFiction => &["SCIENCE FICTION", "SCI-FI", "SCIFI"],
        Sports => &[
            "SPORT", "SPORTS", "PPV", "NFL", "NBA", "NHL", "MLB", "MLS", "UFC", "WWE", "BOXING", "SOCCER", "FOOTBALL", "F1",
            "GOLF", "TENNIS", "RUGBY", "CRICKET", "DAZN", "ESPN",
        ],
        Thriller => &["THRILLER", "THRILLERS", "SUSPENSE"],
        Western => &["WESTERN", "WESTERNS"],
        Faith => &["CHRISTIAN", "BIBLICAL", "RELIGIOUS", "FAITH"],
        Holiday => &["CHRISTMAS", "HOLIDAY", "NOEL"],
        Anime => &["ANIME", "MANGA", "CRUNCHYROLL"],
    }
}

pub(crate) const ALL_GENRES: [ContentGenre; 23] = {
    use ContentGenre::*;
    [
        Action, Adventure, Animation, Comedy, Crime, Documentary, Drama, Family, Fantasy, Horror, Kids, Music, Mystery, News,
        Reality, Romance, ScienceFiction, Sports, Thriller, Western, Faith, Holiday, Anime,
    ]
};

/// Adult studios and channels whose names say nothing else.
const ADULT_BRANDS: &[&str] = &["PLAYBOY", "HUSTLER", "BRAZZERS"];

/// Superscript quality tags ("ᴴᴰ", "⁴ᴷ", "⁶⁰ᶠᵖˢ") are modifier letters and
/// other numbers: decoration, not words.
static DECORATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{Lm}\p{No}]").expect("built-in pattern"));
/// Everything but letters, digits and the separators codes are written with.
static SYMBOLS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{Nd} |\-:()\[\]/+&']").expect("built-in pattern"));

/// What a category's name says about it.
pub(crate) fn traits(name: &str) -> CategoryTraits {
    let text = normalized(name);
    let mut traits = CategoryTraits { adult: parental::looks_adult(name) || ADULT_BRANDS.iter().any(|brand| text.contains(brand)), ..Default::default() };
    // Words keep their pluses and hyphens ("18+", "SCI-FI"); parts split at them too ("DOCU-MOVIES").
    let tokens: HashSet<&str> = text.split(|c: char| !c.is_alphanumeric() && c != '+' && c != '-').filter(|token| !token.is_empty()).collect();
    let parts: HashSet<&str> = tokens.iter().copied().chain(text.split(|c: char| !c.is_alphanumeric()).filter(|part| !part.is_empty())).collect();

    // A leading code ("US|", "EN -", "(UK)", "|FR|") is the strongest signal.
    if let Some(language) = leading_code(&text).and_then(|code| find(CODES, code)) {
        traits.languages.insert(language);
    }
    // A trailing language code overrides a regional prefix: "CA| GENERAL FR".
    if let Some(language) = text.rsplit(' ').next().filter(|last| last.len() == 2).and_then(|last| find(TRAILING_CODES, last)) {
        traits.languages = BTreeSet::from([language]);
    }
    // Spelled-out languages anywhere ("ENGLISH SERIES", "QUÉBEC SERIES").
    for (word, language) in WORDS {
        if tokens.contains(word) {
            traits.languages.insert(*language);
        }
    }
    // "TURKISH SERIES (SUB EN)" is Turkish with English subtitles, not English.
    if (text.contains("SUB EN") || text.contains("SUB ENG")) && traits.languages.len() > 1 {
        traits.languages.remove(&ContentLanguage::English);
    }
    traits.genres = genres_in_text(&text, &parts);
    traits
}

/// Genres a title's free-text genre field names ("Action, Thriller").
pub(crate) fn genres_in(field: &str) -> BTreeSet<ContentGenre> {
    let text = normalized(&field.replace(',', " "));
    let parts: HashSet<&str> = text.split(|c: char| !c.is_alphanumeric() && c != '-').chain(text.split(|c: char| !c.is_alphanumeric())).filter(|part| !part.is_empty()).collect();
    genres_in_text(&text, &parts)
}

fn genres_in_text(text: &str, parts: &HashSet<&str>) -> BTreeSet<ContentGenre> {
    ALL_GENRES
        .into_iter()
        .filter(|genre| {
            genre_keywords(*genre).iter().any(|keyword| if keyword.contains([' ', '-']) { text.contains(keyword) } else { parts.contains(keyword) })
        })
        .collect()
}

fn find(table: &[(&str, ContentLanguage)], code: &str) -> Option<ContentLanguage> {
    table.iter().find(|(key, _)| *key == code).map(|(_, language)| *language)
}

/// Upper case without accents or decoration, keeping letters, digits and
/// the separators codes are written with.
fn normalized(name: &str) -> String {
    let undecorated = DECORATION.replace_all(name, "");
    let plain = SYMBOLS.replace_all(&undecorated, "");
    let folded: String = plain.nfkd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_uppercase).collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A code of two to four letters at the start, followed by a separator
/// ("US|", "EN -", "UK :"), so a first word like "IN THEATERS" never counts.
fn leading_code(text: &str) -> Option<&str> {
    let trimmed = text.trim_start_matches([' ', '|', '(', '[', '-']);
    let length = trimmed.find(|c: char| !c.is_alphabetic()).unwrap_or(trimmed.len());
    let code = &trimmed[..length];
    if !(2..=4).contains(&code.chars().count()) {
        return None;
    }
    let next = trimmed[length..].trim_start_matches(' ').chars().next()?;
    "|-:)]".contains(next).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ContentLanguage::*;

    fn languages(name: &str) -> Vec<ContentLanguage> {
        traits(name).languages.into_iter().collect()
    }

    #[test]
    fn reads_languages_from_category_names() {
        for (name, expected) in [
            ("US| NEWS ᴴᴰ/ᴿᴬᵂ ⁶⁰ᶠᵖˢ", vec![English]),
            ("EN - ACTION", vec![English]),
            ("QC - FILMS NOUVELLE VERSION", vec![French]),
            ("CA| GENERAL FR", vec![French]),
            ("CA| GENERAL EN", vec![English]),
            ("TURKSIH SERIES (SUB EN)", vec![Turkish]),
            ("EN - ITALIAN SUB ENG", vec![Italian]),
            ("QUÉBEC SERIES", vec![French]),
            ("NETFLIX MOVIES ⁴ᴷ ³⁸⁴⁰ᴾ", vec![]),
            ("IN THEATERS NOW", vec![]),
            ("NO LIMITS", vec![]),
            ("|HU| SPORT", vec![Hungarian]),
            ("TR | Yerli Diziler", vec![Turkish]),
            ("(UK) Documentaries", vec![English]),
        ] {
            assert_eq!(languages(name), expected, "{name}");
        }
    }

    #[test]
    fn reads_genres_and_adult_content() {
        assert!(traits("EN - SCIENCE FICTION").genres.contains(&ContentGenre::ScienceFiction));
        assert!(traits("AMAZON DOCU-MOVIES").genres.contains(&ContentGenre::Documentary));
        assert!(traits("US| NFL PPV").genres.contains(&ContentGenre::Sports));
        assert!(traits("24/7 Comedy").genres.contains(&ContentGenre::Comedy));
        assert!(traits("NETFLIX MOVIES").genres.is_empty());
        assert!(traits("FOR ADULTS").adult);
        assert!(traits("XXX | Brazzers").adult);
        assert!(traits("FR| ADULTES 18+").adult);
        assert!(!traits("EN - FAMILY").adult);
        assert!(!traits("Adult Swim").adult);
        assert_eq!(genres_in("Action, Sci-Fi & Fantasy").into_iter().collect::<Vec<_>>(), [ContentGenre::Action, ContentGenre::Fantasy, ContentGenre::ScienceFiction]);
    }
}
