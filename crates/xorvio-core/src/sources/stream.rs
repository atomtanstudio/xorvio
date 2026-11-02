//! Turning stored stream locators into links to open. Logins are applied only
//! here, at playback time, and are never written to the catalog.

use chrono::{DateTime, Datelike, Timelike};
use xorvio_model::{Channel, Timestamp};

/// Whether a programme falls inside the replay window the channel advertises.
/// A guide listing alone never implies replay.
pub fn can_replay(channel: &Channel, start: Timestamp, now: Timestamp) -> bool {
    if channel.catchup_days <= 0 || start >= now {
        return false;
    }
    start >= now - channel.catchup_days * 86_400
}

/// A playlist channel's replay link: its `catchup-source` template filled in,
/// either a full link or a query appended to the live link.
pub fn playlist_replay_url(live_url: &str, template: &str, start: Timestamp, end: Timestamp, now: Timestamp) -> String {
    let filled = fill_template(template, start, end, now);
    if filled.starts_with("http") {
        return filled;
    }
    // Relative templates ("?utc=…") are appended to the live link.
    if live_url.contains('?') { format!("{live_url}{}", filled.replacen('?', "&", 1)) } else { format!("{live_url}{filled}") }
}

/// Fills the catch-up placeholders playlists use.
pub fn fill_template(template: &str, start: Timestamp, end: Timestamp, now: Timestamp) -> String {
    let duration = (end - start).max(0);
    let offset = now - start;
    let utc = DateTime::from_timestamp(start, 0).unwrap_or_default();
    let replacements: [(&str, String); 18] = [
        ("${start}", start.to_string()),
        ("${end}", end.to_string()),
        ("${timestamp}", now.to_string()),
        ("${duration}", duration.to_string()),
        ("${offset}", offset.to_string()),
        ("{utc}", start.to_string()),
        ("{start}", start.to_string()),
        ("{utcend}", end.to_string()),
        ("{end}", end.to_string()),
        ("{lutc}", now.to_string()),
        ("{duration}", duration.to_string()),
        ("{offset}", offset.to_string()),
        ("{Y}", utc.year().to_string()),
        ("{m}", format!("{:02}", utc.month())),
        ("{d}", format!("{:02}", utc.day())),
        ("{H}", format!("{:02}", utc.hour())),
        ("{M}", format!("{:02}", utc.minute())),
        ("{S}", format!("{:02}", utc.second())),
    ];
    replacements.iter().fold(template.to_string(), |text, (placeholder, value)| text.replace(placeholder, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xorvio_model::StreamLocator;

    fn channel(days: i64) -> Channel {
        Channel {
            id: "c".into(),
            source_id: "s".into(),
            name: "C".into(),
            number: None,
            logo_url: None,
            category_id: None,
            epg_id: Some("c".into()),
            stream: StreamLocator::Url { url: "http://h/live.m3u8".into() },
            catchup_days: days,
            catchup_source: Some("?utc={utc}".into()),
            sort_index: 0,
            catchup_offset_minutes: 0,
            versions: Vec::new(),
        }
    }

    #[test]
    fn replay_requires_an_advertised_window() {
        let now = 1_800_000_000;
        assert!(!can_replay(&channel(0), now - 7200, now), "a guide listing alone never implies replay");
        assert!(can_replay(&channel(1), now - 7200, now));
        assert!(!can_replay(&channel(1), now - 3 * 86_400, now), "older than the window");
        assert!(!can_replay(&channel(1), now + 600, now), "not aired yet");
    }

    #[test]
    fn fills_templates() {
        let start = 1_790_346_600; // 2026-09-25 14:30 UTC
        let filled = fill_template("{Y}/{m}/{d}/{H}-{M}-{S}?s=${start}&e={utcend}&d={duration}&o={offset}", start, start + 3600, start + 7200);
        assert_eq!(filled, format!("2026/09/25/14-30-00?s={start}&e={}&d=3600&o=7200", start + 3600));
        assert_eq!(playlist_replay_url("http://h/live.m3u8", "?utc={utc}", 5, 10, 20), "http://h/live.m3u8?utc=5");
        assert_eq!(playlist_replay_url("http://h/live.m3u8?token=1", "?utc={utc}", 5, 10, 20), "http://h/live.m3u8?token=1&utc=5");
        assert_eq!(playlist_replay_url("http://h/live.m3u8", "https://r.example/{utc}.m3u8", 5, 10, 20), "https://r.example/5.m3u8");
    }
}
