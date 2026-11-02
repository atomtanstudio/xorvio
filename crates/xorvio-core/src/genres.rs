//! A programme's broad kind, read from its guide category ("Sports",
//! "Spielfilm", "Dessin animé", "Noticias"), so the guide can tint it.
//! Guides write categories in their own language and their own words, so
//! this goes by word stems in several languages and leaves anything it
//! doesn't recognise plain.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;
use xorvio_model::ProgramGenre;

/// Stems found anywhere in the category (German runs words together:
/// "Kinderserie", "Dokumentarfilm"), and short words that must stand alone.
struct Rule {
    genre: ProgramGenre,
    stems: &'static [&'static str],
    words: &'static [&'static str],
}

/// First match wins, so the order settles mixed categories: a children's
/// film is for children, sports news is sport, a documentary film is factual.
const RULES: &[Rule] = &[
    Rule {
        genre: ProgramGenre::Kids,
        stems: &[
            "child", "kinder", "enfant", "jeunesse", "infantil", "bambin", "ragazz", "cartoon", "cartoni", "animat",
            "animac", "animaz", "animad", "dessin", "dibujos", "zeichentrick", "preschool", "youth", "jeugd", "dzieci",
            "cocuk",
        ],
        words: &["kid", "kids"],
    },
    Rule {
        genre: ProgramGenre::Sport,
        stems: &[
            "sport", "deporte", "football", "futbol", "fussball", "calcio", "soccer", "tennis", "golf", "basketball",
            "baloncesto", "baseball", "hockey", "rugby", "cricket", "boxing", "boxe", "wrestling", "racing", "formula 1",
            "formel 1", "motogp", "nascar", "cycling", "ciclismo", "athletics", "atletismo", "olympi", "handball",
            "volleyball", "snooker", "darts",
        ],
        words: &["nfl", "nba", "mlb", "nhl", "ufc", "mma", "f1", "spor"],
    },
    Rule {
        genre: ProgramGenre::News,
        stems: &[
            "news", "nachricht", "noticia", "notizi", "nieuws", "nyhet", "nyhed", "wiadomo", "haber", "actualit",
            "actualidad", "informati", "informativ", "informazion", "telediario", "telegiornale", "journal", "jornal",
            "weather", "meteo", "wetter", "current affairs", "politi",
        ],
        words: &["info", "infos"],
    },
    Rule {
        genre: ProgramGenre::Factual,
        stems: &[
            "docu", "doku", "factual", "science", "scienc", "wissen", "ciencia", "scienza", "natur", "wildlife", "animal",
            "tiere", "history", "histoire", "historia", "geschichte", "storia", "educati", "bildung", "reportage",
            "biograph", "biografi", "belgesel",
        ],
        words: &[],
    },
    Rule {
        genre: ProgramGenre::Film,
        stems: &["movie", "film", "cinema", "kino", "pelicula", "largometraje"],
        words: &["cine"],
    },
    Rule {
        genre: ProgramGenre::Music,
        stems: &["music", "musik", "musiq", "muziek", "muzik", "concert", "konzert", "concier", "ballet", "jazz"],
        words: &["opera", "clip", "clips"],
    },
    Rule {
        genre: ProgramGenre::Entertainment,
        stems: &[
            "serie", "sitcom", "soap", "novela", "feuilleton", "fiction", "comed", "komodie", "humo", "drama", "dramm",
            "entertain", "divertis", "entreten", "intratten", "unterhaltung", "amusement", "rozrywka", "show", "quiz",
            "game", "concurso", "reality", "realite", "talk", "variety", "variete", "varieta", "spectacle", "theatre",
            "theater", "krimi", "crime", "thriller", "horror", "fantasy", "mystery", "romance", "western", "adventure",
            "scifi",
        ],
        words: &["jeu", "action", "dizi"],
    },
];

/// Phrases whose words would be read wrongly one by one ("science" in
/// "science fiction", "opera" in "soap opera"), after folding.
const PHRASES: &[(&str, &str)] = &[
    ("soap opera", "soap"),
    ("science fiction", "scifi"),
    ("sci fi", "scifi"),
    ("ciencia ficcion", "scifi"),
    ("fantascienza", "scifi"),
];

/// Lowercase without accents, with "ß" as "ss" and hyphens as spaces.
fn fold(text: &str) -> String {
    let mut folded: String = text
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .map(|c| if c == '-' || c == '_' { ' ' } else { c })
        .collect::<String>()
        .replace('ß', "ss");
    for (phrase, meaning) in PHRASES {
        if folded.contains(phrase) {
            folded = folded.replace(phrase, meaning);
        }
    }
    folded
}

/// The genre a guide category points to, or `None` when it's unknown or
/// says nothing about the kind of programme ("Lifestyle", "Magazine").
pub fn program_genre(category: &str) -> Option<ProgramGenre> {
    let folded = fold(category);
    let words: Vec<&str> = folded.split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty()).collect();
    if words.is_empty() {
        return None;
    }
    RULES
        .iter()
        .find(|rule| rule.stems.iter().any(|stem| folded.contains(stem)) || words.iter().any(|word| rule.words.contains(word)))
        .map(|rule| rule.genre)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ProgramGenre::*;

    #[test]
    fn reads_categories_in_several_languages() {
        for (category, expected) in [
            // English, including XMLTV's DVB wording and Schedules Direct's.
            ("Sports", Some(Sport)),
            ("Sports event", Some(Sport)),
            ("Football", Some(Sport)),
            ("NBA", Some(Sport)),
            ("News", Some(News)),
            ("News / Current affairs", Some(News)),
            ("Weather", Some(News)),
            ("Movie", Some(Film)),
            ("Movie / Drama", Some(Film)),
            ("Children", Some(Kids)),
            ("Children's / Youth programmes", Some(Kids)),
            ("Animation", Some(Kids)),
            ("Documentary", Some(Factual)),
            ("Education / Science / Factual topics", Some(Factual)),
            ("Nature", Some(Factual)),
            ("Series", Some(Entertainment)),
            ("Sitcom", Some(Entertainment)),
            ("Show / Game show", Some(Entertainment)),
            ("Reality", Some(Entertainment)),
            ("Music / Ballet / Dance", Some(Music)),
            // French
            ("Sport", Some(Sport)),
            ("Information", Some(News)),
            ("Journal", Some(News)),
            ("Météo", Some(News)),
            ("Film", Some(Film)),
            ("Téléfilm", Some(Film)),
            ("Jeunesse", Some(Kids)),
            ("Dessin animé", Some(Kids)),
            ("Documentaire", Some(Factual)),
            ("Série", Some(Entertainment)),
            ("Divertissement", Some(Entertainment)),
            ("Téléréalité", Some(Entertainment)),
            ("Musique", Some(Music)),
            // Spanish
            ("Deportes", Some(Sport)),
            ("Fútbol", Some(Sport)),
            ("Noticias", Some(News)),
            ("Informativo", Some(News)),
            ("Cine", Some(Film)),
            ("Películas", Some(Film)),
            ("Infantil", Some(Kids)),
            ("Dibujos animados", Some(Kids)),
            ("Documental", Some(Factual)),
            ("Entretenimiento", Some(Entertainment)),
            ("Concurso", Some(Entertainment)),
            ("Telenovela", Some(Entertainment)),
            ("Música", Some(Music)),
            // German
            ("Fußball", Some(Sport)),
            ("Nachrichten", Some(News)),
            ("Spielfilm", Some(Film)),
            ("Fernsehfilm", Some(Film)),
            ("Kinder", Some(Kids)),
            ("Zeichentrick", Some(Kids)),
            ("Dokumentation", Some(Factual)),
            ("Wissen", Some(Factual)),
            ("Unterhaltung", Some(Entertainment)),
            ("Krimiserie", Some(Entertainment)),
            ("Komödie", Some(Entertainment)),
            ("Musik", Some(Music)),
            // Italian, Portuguese, Dutch
            ("Notiziario", Some(News)),
            ("Cartoni animati", Some(Kids)),
            ("Desporto", Some(Sport)),
            ("Nieuws", Some(News)),
            ("Muziek", Some(Music)),
            // Upper case and stray spaces
            ("  SPORTS  ", Some(Sport)),
        ] {
            assert_eq!(program_genre(category), expected, "{category}");
        }
    }

    #[test]
    fn mixed_categories_go_to_the_more_telling_kind() {
        for (category, expected) in [
            ("Children's movie", Kids),
            ("Kinderfilm", Kids),
            ("Kinderserie", Kids),
            ("Sports news", Sport),
            ("Sportnachrichten", Sport),
            ("Documentary film", Factual),
            ("Dokumentarfilm", Factual),
            ("Science fiction", Entertainment),
            ("Science-Fiction", Entertainment),
            ("Ciencia ficción", Entertainment),
            ("Soap opera", Entertainment),
            ("Opera", Music),
        ] {
            assert_eq!(program_genre(category), Some(expected), "{category}");
        }
    }

    #[test]
    fn unknown_categories_stay_plain() {
        for category in ["", "  ", "Lifestyle", "Magazine", "Cooking", "Shopping", "Religion", "Travel", "Special", "Paid Programming", "Other"] {
            assert_eq!(program_genre(category), None, "{category}");
        }
    }
}
