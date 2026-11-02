//! The parts of an episode worth skipping (intro, recap, end credits), from
//! IntroDB (introdb.app): free, community-timed, and needing no account.
//! Only the series' IMDb id and the episode's number are sent, and nothing
//! when Settings › Playback says not to.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde_json::Value;
use xorvio_model::{EpisodeSegments, Segment};

use crate::{Core, Result};

/// Where IntroDB is. Tests use a stand-in, and anything that forgets to set
/// one reaches nothing.
#[cfg(not(test))]
pub(crate) const DEFAULT_BASE: &str = "https://api.introdb.app";
#[cfg(test)]
pub(crate) const DEFAULT_BASE: &str = "http://127.0.0.1:9";

/// Answers already fetched this session, missing ones included, so each
/// episode is asked about once.
static CACHE: LazyLock<Mutex<HashMap<String, Option<EpisodeSegments>>>> = LazyLock::new(Mutex::default);

impl Core {
    #[cfg(test)]
    pub(crate) fn set_intro_db_base(&self, base: &str) {
        *crate::store::lock(&self.intro_db_base) = base.to_string();
    }

    /// What to skip in an episode, or `None` when nobody has timed it (or
    /// IntroDB can't be reached: skipping is a nicety, never an error).
    pub async fn episode_segments(&self, imdb_id: &str, season: i64, episode: i64) -> Result<Option<EpisodeSegments>> {
        if !self.intro_db_enabled.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(None);
        }
        let valid = imdb_id.strip_prefix("tt").is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
        if !valid || season < 0 || episode < 1 {
            return Ok(None);
        }
        let key = format!("{imdb_id}|{season}|{episode}");
        if let Some(known) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Ok(known.clone());
        }
        let base = crate::store::lock(&self.intro_db_base).clone();
        let mut link = url::Url::parse(&format!("{}/segments", base.trim_end_matches('/')))?;
        link.query_pairs_mut()
            .append_pair("imdb_id", imdb_id)
            .append_pair("season", &season.to_string())
            .append_pair("episode", &episode.to_string());
        let found = match self.http.json(link.as_str(), None).await {
            Ok(value) => parse(&value),
            Err(error) => {
                tracing::debug!("IntroDB had nothing for an episode: {error}");
                None
            }
        };
        CACHE.lock().unwrap_or_else(|e| e.into_inner()).insert(key, found.clone());
        Ok(found)
    }
}

fn parse(value: &Value) -> Option<EpisodeSegments> {
    let segment = |name: &str| {
        let part = &value[name];
        let start = part["start_sec"].as_f64()?;
        let end = part["end_sec"].as_f64()?;
        (start.is_finite() && end > start).then_some(Segment { start_seconds: start.max(0.0), end_seconds: end })
    };
    let segments = EpisodeSegments { intro: segment("intro"), recap: segment("recap"), credits: segment("outro") };
    (segments.intro.is_some() || segments.recap.is_some() || segments.credits.is_some()).then_some(segments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_introdb_segments() {
        let found = parse(&json!({
            "imdb_id": "tt0903747", "season": 1, "episode": 1,
            "intro": { "start_sec": 2.5, "end_sec": 58, "confidence": 0.92 },
            "recap": null, "outro": { "start_sec": 2700.0, "end_sec": 2650.0 },
            "no_intro": false
        }))
        .unwrap();
        assert_eq!(found.intro, Some(Segment { start_seconds: 2.5, end_seconds: 58.0 }));
        assert_eq!(found.credits, None, "an end before its start is nonsense");
        assert!(parse(&json!({ "intro": null, "recap": null, "outro": null })).is_none());
    }
}
