//! The watchlist's rules: the order Home shows it in, and following a saved
//! programme when the guide moves it.

use xorvio_model::{MediaKind, Program, Timestamp, WatchlistEntry};

use crate::names;

/// How far ahead a programme counts as "starting soon" and leads the list.
pub const SOON_SECONDS: i64 = 12 * 3600;
/// How far a programme may move and still be recognised as the same one.
pub const MATCH_WINDOW_SECONDS: i64 = 6 * 3600;
/// How long after a programme starts its reminder may still go out: a Mac
/// that wakes just after kick-off still hears about it, but not about a game
/// that's well under way.
pub const REMINDER_GRACE_SECONDS: i64 = 2 * 60;

/// A programme that has finished airing. Films and series never end.
pub fn has_ended(entry: &WatchlistEntry, now: Timestamp) -> bool {
    entry.kind == MediaKind::Program && entry.end.unwrap_or(Timestamp::MAX) <= now
}

/// A programme on the air right now.
pub fn is_airing(entry: &WatchlistEntry, now: Timestamp) -> bool {
    match (entry.kind, entry.start, entry.end) {
        (MediaKind::Program, Some(start), Some(end)) => start <= now && now < end,
        _ => false,
    }
}

/// The entry for a guide programme: its listing is copied, so the entry still
/// reads well after the guide refreshes, and a "Live:" marker is dropped.
pub fn entry_for_program(program: &Program, channel_id: &str, remind: bool, added_at: Timestamp) -> WatchlistEntry {
    WatchlistEntry {
        id: program.id.clone(),
        kind: MediaKind::Program,
        source_id: program.source_id.clone(),
        title: names::live_marker(&program.title).0,
        subtitle: program.episode.clone().or_else(|| program.subtitle.clone()),
        artwork_url: program.image_url.clone(),
        added_at,
        channel_id: Some(channel_id.to_string()),
        start: Some(program.start),
        end: Some(program.end),
        remind,
    }
}

/// The order Home shows: what's on now, then programmes starting within
/// `soon` seconds, then films and series (newest first), then later
/// programmes by start time. Time-sensitive things lead; nothing else jumps
/// the queue. Programmes that have ended are left out.
pub fn ordered(entries: &[WatchlistEntry], now: Timestamp, soon: i64) -> Vec<WatchlistEntry> {
    let start_of = |entry: &WatchlistEntry| entry.start.unwrap_or(Timestamp::MAX);
    let mut live: Vec<&WatchlistEntry> = entries.iter().filter(|entry| is_airing(entry, now)).collect();
    live.sort_by_key(|entry| entry.start.unwrap_or(Timestamp::MIN));
    let mut upcoming: Vec<&WatchlistEntry> = entries
        .iter()
        .filter(|entry| entry.kind == MediaKind::Program && !is_airing(entry, now) && !has_ended(entry, now))
        .collect();
    upcoming.sort_by_key(|entry| start_of(entry));
    let (starting_soon, later): (Vec<&WatchlistEntry>, Vec<&WatchlistEntry>) =
        upcoming.into_iter().partition(|entry| start_of(entry) <= now.saturating_add(soon));
    let mut media: Vec<&WatchlistEntry> = entries.iter().filter(|entry| entry.kind != MediaKind::Program).collect();
    media.sort_by_key(|entry| std::cmp::Reverse(entry.added_at));
    live.into_iter().chain(starting_soon).chain(media).chain(later).cloned().collect()
}

/// The listing that is the same programme as a saved one after the guide
/// refreshes: the same title on the same channel, starting nearest the saved
/// time and within `window` seconds. Lets a reminder follow a game that moved.
pub fn matching<'a>(entry: &WatchlistEntry, programs: &'a [Program], window: i64) -> Option<&'a Program> {
    let start = entry.start.filter(|_| entry.kind == MediaKind::Program)?;
    let title = normalized(&entry.title);
    programs
        .iter()
        .filter(|program| normalized(&program.title) == title && (program.start - start).abs() <= window)
        .min_by_key(|program| (program.start - start).abs())
}

/// Programmes whose reminder is due at `now`: reminded ones starting within
/// `lead` seconds, or that started no more than [`REMINDER_GRACE_SECONDS`]
/// ago, soonest first. Whether a reminder was already sent is the library's
/// business, not this rule's.
pub fn reminders_due(entries: &[WatchlistEntry], now: Timestamp, lead: i64) -> Vec<&WatchlistEntry> {
    let mut due: Vec<&WatchlistEntry> = entries
        .iter()
        .filter(|entry| entry.kind == MediaKind::Program && entry.remind && !has_ended(entry, now))
        .filter(|entry| entry.start.is_some_and(|start| start - lead.max(0) <= now && now <= start + REMINDER_GRACE_SECONDS))
        .collect();
    due.sort_by_key(|entry| entry.start);
    due
}

/// A reminder's headline, from the time actually left (the check runs every
/// half minute, so "5 minutes before" can go out at 4½): "Starting in 5
/// minutes", "Starting now", or "On now" once it has begun.
pub fn reminder_title(start: Timestamp, now: Timestamp) -> String {
    if start < now - 30 {
        return "On now".into();
    }
    // To the nearest minute.
    match (start - now + 30).div_euclid(60) {
        ..=0 => "Starting now".into(),
        1 => "Starting in 1 minute".into(),
        minutes => format!("Starting in {minutes} minutes"),
    }
}

/// Case and punctuation don't matter, sports listings' "@", "at", "v" and
/// "vs." read the same, and a leading "Live:" (which some guides put in the
/// title and Xorvio turns into a flag) is ignored.
pub(crate) fn normalized(title: &str) -> String {
    let lowered = title.to_lowercase().replace('@', " at ");
    let mut words: Vec<&str> = lowered
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| if matches!(word, "v" | "vs" | "versus") { "at" } else { word })
        .collect();
    if words.len() > 1 && words[0] == "live" {
        words.remove(0);
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use xorvio_model::ProgramFlags;

    const NOW: Timestamp = 1_800_000_000;

    fn program(title: &str, starts_in_minutes: i64, length_minutes: i64) -> Program {
        let start = NOW + starts_in_minutes * 60;
        Program {
            id: Program::make_id("s", "espn", start),
            source_id: "s".into(),
            epg_id: "espn".into(),
            start,
            end: start + length_minutes * 60,
            title: title.into(),
            subtitle: None,
            summary: None,
            category: None,
            image_url: None,
            episode: None,
            flags: ProgramFlags::default(),
            age_rating: None,
            star_rating: None,
            year: None,
            genre: None,
        }
    }

    fn saved(title: &str, starts_in_minutes: i64) -> WatchlistEntry {
        entry_for_program(&program(title, starts_in_minutes, 120), "c1", true, NOW)
    }

    fn film(key: &str, added_minutes_ago: i64) -> WatchlistEntry {
        WatchlistEntry {
            id: format!("s|mv|{key}"),
            kind: MediaKind::Movie,
            source_id: "s".into(),
            title: format!("Film {key}"),
            subtitle: None,
            artwork_url: None,
            added_at: NOW - added_minutes_ago * 60,
            channel_id: None,
            start: None,
            end: None,
            remind: false,
        }
    }

    #[test]
    fn live_and_soon_lead_then_films_then_later_programmes() {
        let live = saved("Game On", -30);
        let tonight = saved("Tonight's Game", 180);
        let next_week = saved("Final", 6 * 24 * 60);
        let ended = saved("Over", -300);
        let older = film("1", 600);
        let newer = film("2", 5);
        let ordered = ordered(&[next_week, older.clone(), tonight, ended.clone(), newer, live.clone()], NOW, SOON_SECONDS);
        let titles: Vec<&str> = ordered.iter().map(|entry| entry.title.as_str()).collect();
        assert_eq!(titles, ["Game On", "Tonight's Game", "Film 2", "Film 1", "Final"]);
        assert!(is_airing(&live, NOW));
        assert!(has_ended(&ended, NOW));
        assert!(!has_ended(&older, NOW), "films never expire");
    }

    #[test]
    fn follows_a_programme_that_moved() {
        let saved_game = saved("Lakers at Celtics", 120);
        let listings = [program("NBA Countdown", 90, 30), program("Lakers @ Celtics", 150, 120), program("Lakers at Celtics", 60 * 30, 120)];
        let found = matching(&saved_game, &listings, MATCH_WINDOW_SECONDS).unwrap();
        assert_eq!(found.start, NOW + 150 * 60, "same title, nearest start; “@” reads as “at”");
        let versus = saved("Arsenal v Chelsea", 120);
        assert!(matching(&versus, &[program("Arsenal vs. Chelsea", 125, 120)], MATCH_WINDOW_SECONDS).is_some());
        let marked = WatchlistEntry { title: "Live: NBA Basketball".into(), ..saved("x", 120) };
        assert!(
            matching(&marked, &[program("NBA Basketball", 120, 120)], MATCH_WINDOW_SECONDS).is_some(),
            "a saved “Live:” title still finds the cleaned listing"
        );
        assert!(matching(&saved_game, &[program("Something Else", 120, 60)], MATCH_WINDOW_SECONDS).is_none());
        assert!(matching(&film("1", 1), &listings, MATCH_WINDOW_SECONDS).is_none());
    }

    #[test]
    fn reminder_titles_say_how_long_is_left() {
        let start = NOW + 300;
        assert_eq!(reminder_title(start, NOW), "Starting in 5 minutes");
        assert_eq!(reminder_title(start, NOW + 25), "Starting in 5 minutes", "a check half a minute late still reads 5");
        assert_eq!(reminder_title(start, NOW + 230), "Starting in 1 minute");
        assert_eq!(reminder_title(start, NOW + 290), "Starting now");
        assert_eq!(reminder_title(start, NOW + 320), "Starting now");
        assert_eq!(reminder_title(start, NOW + 400), "On now");
    }

    #[test]
    fn programme_entries_drop_the_live_marker() {
        let mut listing = program("Live: Premier League", 10, 90);
        listing.episode = Some("S2 E5".into());
        listing.subtitle = Some("Matchday".into());
        let entry = entry_for_program(&listing, "c1", true, NOW);
        assert_eq!(entry.title, "Premier League");
        assert_eq!(entry.subtitle.as_deref(), Some("S2 E5"));
        assert_eq!(entry.id, listing.id);
    }
}
