//! The demonstration source: clearly labeled, entirely fictional channels,
//! films, series and listings. Streams are public test streams published for
//! player testing (Apple's HLS examples, Unified Streaming's live demo, and
//! Blender Foundation open movies under CC BY). The interface always badges
//! this source as "Demo".
//!
//! Artwork addresses use the `xorvio-art://` scheme
//! (`xorvio-art://poster/the-glass-valley?v=0&t=The+Glass+Valley`): a kind, a
//! slug, a variant and the title. The interface paints them locally and
//! nothing is downloaded.

use chrono::{Datelike, Duration, Local, TimeZone};
use xorvio_model::{
    Category, CategoryKind, Channel, DEMO_SOURCE_ID, Episode, Movie, Program, ProgramFlags, SeasonInfo, Series,
    SourceCapabilities, StreamLocator, Timestamp, VideoFormat,
};

use super::ids;
use super::xmltv::ChannelName;
use crate::details::DetailsRecord;
use crate::snapshot::CatalogSnapshot;

mod streams {
    pub const LIVE: &str = "https://demo.unified-streaming.com/k8s/live/stable/live.isml/.m3u8";
    pub const BIPBOP_ADVANCED: &str =
        "https://devstreaming-cdn.apple.com/videos/streaming/examples/img_bipbop_adv_example_fmp4/master.m3u8";
    pub const BIPBOP_16X9: &str =
        "https://devstreaming-cdn.apple.com/videos/streaming/examples/bipbop_16x9/bipbop_16x9_variant.m3u8";
    pub const BIPBOP_4X3: &str =
        "https://devstreaming-cdn.apple.com/videos/streaming/examples/bipbop_4x3/bipbop_4x3_variant.m3u8";
    pub const DOLBY_VISION_ATMOS: &str = "https://devstreaming-cdn.apple.com/videos/streaming/examples/adv_dv_atmos/main.m3u8";
    pub const BIG_BUCK_BUNNY: &str = "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8";
    pub const TEARS_OF_STEEL: &str =
        "https://demo.unified-streaming.com/k8s/features/stable/video/tears-of-steel/tears-of-steel.ism/.m3u8";
    /// A Matroska sample (Big Buck Bunny, Blender Foundation, CC BY 3.0) that needs the remuxing path.
    pub const MATROSKA_SAMPLE: &str = "https://test-videos.co.uk/vids/bigbuckbunny/mkv/720/Big_Buck_Bunny_720_10s_2MB.mkv";

    pub const LIVE_ROTATION: [&str; 6] = [LIVE, BIPBOP_ADVANCED, LIVE, BIPBOP_16X9, LIVE, TEARS_OF_STEEL];
    pub const VOD_ROTATION: [&str; 5] = [BIG_BUCK_BUNNY, TEARS_OF_STEEL, BIPBOP_ADVANCED, BIPBOP_16X9, BIPBOP_4X3];
}

/// (name, category, replay days)
const CHANNELS: [(&str, &str, i64); 24] = [
    ("Harbor News 24", "News", 3),
    ("Meridian World", "News", 3),
    ("Civic Desk", "News", 0),
    ("Market Hour", "News", 0),
    ("Summit Sports", "Sports", 2),
    ("Summit Sports 2", "Sports", 0),
    ("Trackside", "Sports", 0),
    ("Open Water Sailing", "Sports", 0),
    ("Northlight Cinema", "Movies", 0),
    ("Northlight Classics", "Movies", 0),
    ("Reel Nights", "Movies", 0),
    ("Atlas Documentary", "Documentary", 7),
    ("Deep Field", "Documentary", 0),
    ("Wildland", "Documentary", 0),
    ("Studio Nine", "Entertainment", 1),
    ("Parlour", "Entertainment", 0),
    ("Late Lantern", "Entertainment", 0),
    ("Kitchen Table", "Lifestyle", 0),
    ("Home & Harbour", "Lifestyle", 0),
    ("Lumen Kids", "Kids", 0),
    ("Paper Kite", "Kids", 0),
    ("Meridian Music", "Music", 0),
    ("Night Transit Radio", "Music", 0),
    ("Crescent Jazz", "Music", 0),
];

fn program_titles(category: &str) -> &'static [&'static str] {
    match category {
        "News" => &["Morning Brief", "The Harbor Report", "World Tonight", "Midday Headlines", "The Long Read", "Business Close", "Late Edition", "Weekend Review"],
        "Sports" => &["Matchday Live", "Coastal League Highlights", "The Training Ground", "Race Weekend", "Final Whistle", "Summit Rewind", "Rowing Championship"],
        "Movies" => &["The Lighthouse Keeper", "Paper Boats", "Winter Orchard", "A Quiet Signal", "The Glass Valley", "North of Midnight", "Copper Skies"],
        "Documentary" => &["Rivers of Stone", "The Night Garden", "Cities Under Ice", "Small Worlds", "The Salt Road", "Edge of the Reef", "Deep Field Survey"],
        "Entertainment" => &["Studio Nine Live", "The Parlour Game", "Late Lantern", "Stand-Up Stories", "The Cabinet of Odd Things", "Weekend Variety"],
        "Lifestyle" => &["Kitchen Table", "One Pot Wonders", "Restoring the Cottage", "Garden Hours", "Market Day"],
        "Kids" => &["Paper Kite Adventures", "The Clockwork Fox", "Moonbeam Road", "Build It Big", "Story Lanterns"],
        "Music" => &["Night Transit", "Crescent Sessions", "Vinyl Hour", "Live From the Hall", "Morning Chorus"],
        _ => &["Programme"],
    }
}

/// (title, years before this one, genre, rating, plot, streams)
type MovieSeed = (&'static str, i32, &'static str, f64, &'static str, &'static [&'static str]);

const MOVIES: [MovieSeed; 18] = [
    ("The Lighthouse Keeper", 1, "Drama", 7.8, "A retired engineer takes one last post on a remote island and finds the light has been signalling someone.", &[streams::BIG_BUCK_BUNNY, streams::DOLBY_VISION_ATMOS]),
    ("Paper Boats", 0, "Family", 7.1, "Two siblings race homemade boats down the flooded streets of their town during the longest rain on record.", &[streams::TEARS_OF_STEEL, streams::MATROSKA_SAMPLE]),
    ("Winter Orchard", 6, "Romance", 6.9, "An apple grower and a travelling pianist spend one snowbound season deciding whether to stay.", &[streams::BIPBOP_16X9]),
    ("A Quiet Signal", 0, "Thriller", 7.4, "A radio operator intercepts a message that should not exist for another forty years.", &[streams::BIPBOP_ADVANCED, streams::DOLBY_VISION_ATMOS]),
    ("The Glass Valley", 1, "Science Fiction", 8.0, "Surveyors mapping a crystalline canyon discover that it is slowly mapping them back.", &[streams::TEARS_OF_STEEL]),
    ("North of Midnight", 3, "Mystery", 6.6, "A night-train conductor has until dawn to find a passenger who was never on the manifest.", &[streams::BIG_BUCK_BUNNY]),
    ("Copper Skies", 1, "Adventure", 7.2, "A balloon crew attempts the first crossing of a desert storm season.", &[streams::BIPBOP_4X3]),
    ("Salt and Cedar", 7, "Drama", 7.0, "Three generations of a boatbuilding family gather to launch a hull no one believes will float.", &[streams::BIPBOP_16X9]),
    ("The Clockmaker's Apprentice", 9, "Fantasy", 7.5, "An apprentice discovers each repaired clock returns a lost hour to its owner.", &[streams::TEARS_OF_STEEL]),
    ("Harbor Lights", 0, "Comedy", 6.4, "A struggling seaside theatre stages a disastrous and wonderful final season.", &[streams::BIG_BUCK_BUNNY]),
    ("Field Notes", 4, "Documentary", 7.9, "A year inside a research station tracking migratory birds across three continents.", &[streams::BIPBOP_ADVANCED]),
    ("Blue Hour", 1, "Drama", 7.3, "A photographer returns to her hometown to capture the ten minutes of light she remembers.", &[streams::TEARS_OF_STEEL, streams::DOLBY_VISION_ATMOS]),
    ("Static Bloom", 2, "Science Fiction", 6.8, "An engineer's greenhouse begins to hum at the same frequency as a distant pulsar.", &[streams::BIPBOP_16X9]),
    ("The Last Ferry", 10, "Thriller", 7.1, "On the final crossing before a storm, a ferry captain realizes one car has no driver.", &[streams::BIG_BUCK_BUNNY]),
    ("Kite Season", 3, "Family", 6.7, "A young inventor enters the town kite festival with a design her grandfather abandoned.", &[streams::BIPBOP_4X3]),
    ("Marrow Street", 5, "Crime", 7.6, "A detective on her last week follows a case back to the street where she grew up.", &[streams::TEARS_OF_STEEL]),
    ("Echo Lake", 2, "Mystery", 6.5, "Every summer the lake gives back one thing it took. This year it returns a letter.", &[streams::BIPBOP_ADVANCED]),
    ("Ironwood", 8, "Western", 6.9, "A surveyor and a rancher dispute a boundary that keeps moving overnight.", &[streams::BIG_BUCK_BUNNY]),
];

/// (title, years before this one, genre, rating, plot, episodes per season)
type SeriesSeed = (&'static str, i32, &'static str, f64, &'static str, &'static [i64]);

const SERIES: [SeriesSeed; 6] = [
    ("Tidewater", 1, "Drama", 8.1, "A coastal town's families navigate a century-old dispute over the harbor that feeds them.", &[8, 8, 6]),
    ("The Night Desk", 0, "Crime", 7.7, "A newsroom's overnight team keeps getting to the story before the police do.", &[10, 10]),
    ("Small Engines", 3, "Comedy", 7.2, "A repair shop run by three cousins who cannot agree on anything except the coffee.", &[6, 6, 6, 6]),
    ("Deep Field Survey", 1, "Documentary", 8.4, "An astronomy team spends a year photographing one tiny patch of sky.", &[5]),
    ("Lantern Row", 5, "Mystery", 7.5, "Each episode, a different resident of one street tells what they saw the night the lights went out.", &[8, 8]),
    ("The Clockwork Fox", 0, "Kids", 7.0, "A mechanical fox and a curious girl fix the town's broken inventions.", &[12]),
];

/// The guide ids and names of the demo channels, as an XMLTV file would list them.
pub(crate) fn guide_channel_names() -> Vec<ChannelName> {
    CHANNELS
        .iter()
        .enumerate()
        .map(|(index, (name, _, _))| ChannelName { epg_id: format!("demo-ch-{index}"), display_name: (*name).into() })
        .collect()
}

/// The whole demo catalog. Release years count back from this year, so the
/// demo always has new releases.
pub(crate) fn snapshot(now: Timestamp) -> CatalogSnapshot {
    let source = DEMO_SOURCE_ID;
    let mut snapshot = CatalogSnapshot::default();
    let category = |snapshot: &mut CatalogSnapshot, name: &str, kind: CategoryKind| -> String {
        let id = ids::category(source, kind, name);
        if !snapshot.categories.iter().any(|category| category.id == id) {
            let sort_index = snapshot.categories.len() as i64;
            snapshot.categories.push(Category { id: id.clone(), source_id: source.into(), kind, name: name.into(), sort_index });
        }
        id
    };

    for (index, (name, group, replay_days)) in CHANNELS.iter().enumerate() {
        let key = format!("demo-ch-{index}");
        let category_id = category(&mut snapshot, group, CategoryKind::Live);
        snapshot.channels.push(Channel {
            id: ids::channel(source, &key),
            source_id: source.into(),
            name: (*name).into(),
            number: Some(101 + index as i64),
            logo_url: None,
            category_id: Some(category_id),
            epg_id: Some(key),
            stream: StreamLocator::Url { url: streams::LIVE_ROTATION[index % streams::LIVE_ROTATION.len()].into() },
            catchup_days: *replay_days,
            // Demo replay plays a finite test stream; real sources use the provider's templates.
            catchup_source: (*replay_days > 0).then(|| streams::BIPBOP_16X9.into()),
            sort_index: index as i64,
            catchup_offset_minutes: 0,
            versions: Vec::new(),
        });
    }

    let this_year = Local.timestamp_opt(now, 0).single().map_or(2026, |date| date.year());
    for (index, (title, age, genre, rating, plot, versions)) in MOVIES.iter().enumerate() {
        let year = this_year - age;
        for (version, stream) in versions.iter().enumerate() {
            let premium = *stream == streams::DOLBY_VISION_ATMOS;
            let category_name = if premium {
                "4K & HDR"
            } else if *stream == streams::MATROSKA_SAMPLE {
                "MKV"
            } else {
                genre
            };
            let id = ids::movie(source, &format!("demo-mv-{index}-{version}"));
            let category_id = category(&mut snapshot, category_name, CategoryKind::Movie);
            snapshot.movies.push(Movie {
                id: id.clone(),
                source_id: source.into(),
                title: (*title).into(),
                year: Some(i64::from(year)),
                poster_url: Some(art("poster", title, 0)),
                backdrop_url: Some(art("backdrop", title, 0)),
                category_id: Some(category_id),
                rating: Some(*rating),
                added_at: Some(now - index as i64 * 86_400 * 3),
                plot: Some((*plot).into()),
                genre: Some((*genre).into()),
                tmdb_id: Some(format!("demo-{index}")),
                imdb_id: None,
                duration_seconds: Some(90 * 60 + index as i64 * 240),
                stream: StreamLocator::Url { url: (*stream).into() },
            });
            snapshot.details.insert(
                id,
                DetailsRecord {
                    backdrop_urls: (0..3).map(|variant| art("backdrop", title, variant)).collect(),
                    cast: cast_names(index, 6),
                    director: cast_names(index + 40, 1).pop(),
                    release_date: Some(format!("{year}-{:02}-{:02}", index % 12 + 1, index % 27 + 1)),
                    country: Some(["United Kingdom", "Canada", "Ireland", "New Zealand", "Australia"][index % 5].into()),
                    age_rating: Some(["PG", "12", "PG-13", "15", "PG"][index % 5].into()),
                    genres: vec![(*genre).into()],
                    runtime_minutes: Some(90 + index as i64 * 4),
                    video: Some(if premium {
                        VideoFormat { width: 3840, height: 2160, codec: Some("hevc".into()), is_hdr: true }
                    } else {
                        VideoFormat { width: 1920, height: 1080, codec: Some("h264".into()), is_hdr: false }
                    }),
                    audio_channels: Some(if premium { 6 } else { 2 }),
                    ..Default::default()
                },
            );
        }
    }

    for (index, (title, age, genre, rating, plot, seasons)) in SERIES.iter().enumerate() {
        let series_id = ids::series(source, &format!("demo-se-{index}"));
        let year = this_year - age;
        let category_id = category(&mut snapshot, genre, CategoryKind::Series);
        snapshot.series.push(Series {
            id: series_id.clone(),
            source_id: source.into(),
            title: (*title).into(),
            year: Some(i64::from(year)),
            poster_url: Some(art("poster", title, 0)),
            backdrop_url: Some(art("backdrop", title, 0)),
            category_id: Some(category_id),
            rating: Some(*rating),
            added_at: Some(now - index as i64 * 86_400 * 5),
            plot: Some((*plot).into()),
            genre: Some((*genre).into()),
            tmdb_id: Some(format!("demo-series-{index}")),
            imdb_id: None,
            remote_id: None,
        });
        snapshot.details.insert(
            series_id.clone(),
            DetailsRecord {
                backdrop_urls: (0..3).map(|variant| art("backdrop", title, variant)).collect(),
                cast: cast_names(index + 20, 6),
                director: cast_names(index + 60, 1).pop(),
                release_date: Some(format!("{year}-0{}-1{}", index % 9 + 1, index % 9)),
                age_rating: Some(if *genre == "Kids" { "G" } else { "TV-14" }.into()),
                network: Some(["Harbor One", "Meridian", "Studio Nine", "Atlas", "Northlight", "Paper Kite"][index % 6].into()),
                genres: vec![(*genre).into()],
                seasons: seasons
                    .iter()
                    .enumerate()
                    .map(|(season, count)| SeasonInfo {
                        number: season as i64 + 1,
                        name: Some(format!("Season {}", season + 1)),
                        overview: None,
                        air_date: None,
                        cover_url: None,
                        episode_count: Some(*count),
                    })
                    .collect(),
                ..Default::default()
            },
        );
        for (season_index, count) in seasons.iter().enumerate() {
            for number in 1..=*count {
                let rotation = (index + season_index + number as usize) % streams::VOD_ROTATION.len();
                snapshot.episodes.push(Episode {
                    id: ids::episode(&series_id, &format!("demo-ep-{}-{number}", season_index + 1)),
                    series_id: series_id.clone(),
                    source_id: source.into(),
                    season: season_index as i64 + 1,
                    number,
                    title: episode_title(index, season_index, number as usize),
                    plot: Some("A fictional episode used to demonstrate episode lists, progress and next-episode playback.".into()),
                    duration_seconds: Some(if *genre == "Kids" { 22 } else { 44 } * 60),
                    still_url: Some(art("still", title, season_index as i64 * 20 + number)),
                    stream: StreamLocator::Url { url: streams::VOD_ROTATION[rotation].into() },
                    air_date: None,
                });
            }
        }
    }

    snapshot.capabilities = SourceCapabilities { live: true, movies: true, series: true, guide: true, replay: true };
    snapshot
}

/// Fictional names for demo cast and crew.
fn cast_names(seed: usize, count: usize) -> Vec<String> {
    const FIRST: [&str; 16] = ["Mara", "Joon", "Teodor", "Ana", "Rui", "Ines", "Callum", "Priya", "Oskar", "Leila", "Tomas", "Wren", "Nadia", "Hugo", "Saoirse", "Kenji"];
    const LAST: [&str; 16] = ["Ellison", "Park", "Vass", "Lindqvist", "Bastos", "Calloway", "Reyes", "Okafor", "Brandt", "Haddad", "Moreau", "Kestrel", "Ferro", "Lund", "Byrne", "Arai"];
    (0..count)
        .map(|i| format!("{} {}", FIRST[(seed * 3 + i * 5) % FIRST.len()], LAST[(seed * 7 + i * 3) % LAST.len()]))
        .collect()
}

fn episode_title(series: usize, season: usize, number: usize) -> String {
    const WORDS: [&str; 14] = ["Low Tide", "The Crossing", "Signal Fire", "Old Maps", "Night Shift", "The Inventory", "Salt Marsh", "A Long Weekend", "Open Water", "The Visitor", "Undertow", "Harbor Lights", "First Light", "The Ledger"];
    WORDS[(series * 7 + season * 3 + number) % WORDS.len()].into()
}

/// A deterministic schedule from `days_back` days before today to the end of
/// `days_forward` days after it, aligned to half hours in local time, so Live
/// TV always has something on.
pub(crate) fn programs(now: Timestamp, days_back: i64, days_forward: i64) -> Vec<Program> {
    let today = Local
        .timestamp_opt(now, 0)
        .single()
        .and_then(|moment| moment.date_naive().and_hms_opt(0, 0, 0))
        .and_then(|midnight| Local.from_local_datetime(&midnight).earliest())
        .map_or(now - now.rem_euclid(86_400), |midnight| midnight.timestamp());
    let window_start = today - days_back * 86_400;
    let window_end = today + (days_forward + 1) * 86_400;
    const DURATIONS: [i64; 8] = [30, 60, 45, 90, 30, 60, 120, 60];

    let mut result = Vec::new();
    for (index, (name, group, _)) in CHANNELS.iter().enumerate() {
        let titles = program_titles(group);
        let epg_id = format!("demo-ch-{index}");
        let mut cursor = window_start;
        let mut step = index;
        while cursor < window_end {
            let length = Duration::minutes(DURATIONS[(step * 5 + index) % DURATIONS.len()]).num_seconds();
            let title = titles[(step + index) % titles.len()];
            let numbered = step % 3 == 0;
            result.push(Program {
                id: Program::make_id(DEMO_SOURCE_ID, &epg_id, cursor),
                source_id: DEMO_SOURCE_ID.into(),
                epg_id: epg_id.clone(),
                start: cursor,
                end: cursor + length,
                title: title.into(),
                subtitle: numbered.then(|| format!("Episode {}", step % 12 + 1)),
                summary: Some(format!(
                    "Fictional listing for {name}. Demonstration data shows how the guide handles overlapping schedules, replay windows and programme details."
                )),
                category: Some((*group).into()),
                image_url: Some(art("still", title, 0)),
                episode: numbered.then(|| format!("S1 E{}", step % 12 + 1)),
                flags: ProgramFlags { is_new: step % 7 == 0, ..Default::default() },
                age_rating: None,
                star_rating: None,
                year: None,
                genre: None,
            });
            cursor += length;
            step += 1;
        }
    }
    result
}

/// `xorvio-art://poster/the-glass-valley?v=0&t=The+Glass+Valley`
pub fn art(kind: &str, title: &str, variant: i64) -> String {
    let mut link = url::Url::parse(&format!("xorvio-art://{kind}/{}", slug(title))).expect("a slug is always a valid path");
    link.query_pairs_mut().append_pair("v", &variant.to_string()).append_pair("t", title);
    link.to_string()
}

fn slug(title: &str) -> String {
    title
        .to_lowercase()
        .replace('\'', "")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_is_complete_and_fictional() {
        let now = 1_800_000_000;
        let snapshot = snapshot(now);
        assert_eq!(snapshot.channels.len(), 24);
        assert_eq!(snapshot.movies.len(), 22, "four films come in two versions");
        assert_eq!(snapshot.series.len(), 6);
        assert_eq!(snapshot.episodes.len(), 8 + 8 + 6 + 10 + 10 + 6 * 4 + 5 + 16 + 12);
        assert!(snapshot.channels.iter().all(|c| c.source_id == DEMO_SOURCE_ID));
        assert!(snapshot.details.contains_key(&snapshot.movies[0].id));
        assert_eq!(art("poster", "The Clockmaker's Apprentice", 2), "xorvio-art://poster/the-clockmakers-apprentice?v=2&t=The+Clockmaker%27s+Apprentice");
    }

    #[test]
    fn the_guide_always_has_something_on() {
        let now = 1_800_000_000;
        let programs = programs(now, 2, 3);
        for index in 0..CHANNELS.len() {
            let epg = format!("demo-ch-{index}");
            let airing = programs.iter().filter(|p| p.epg_id == epg && p.is_airing(now)).count();
            assert_eq!(airing, 1, "{epg}");
        }
        let names = guide_channel_names();
        assert_eq!(names[0].display_name, "Harbor News 24");
    }
}
