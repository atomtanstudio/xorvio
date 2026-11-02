//! The parental PIN, and the categories it locks without being asked: ones
//! whose names look adult. The PIN is kept only as a salted, stretched hash
//! in the library, and is never logged. Entering it unlocks everything it
//! covers until someone locks again, the time chosen in Settings passes
//! ([`Core::set_relock_after`]), or Xorvio quits.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;
use xorvio_model::ParentalStatus;

use crate::store::lock;
use crate::{Core, Result};

/// Where the hash lives in the library's preferences.
pub(crate) const PIN_KEY: &str = "parental_pin";

/// Four digits can't hold out against someone who has the library file;
/// stretching the hash only keeps the PIN from being read off at a glance.
const HASH_ROUNDS: u32 = 10_000;

/// Wrong PINs in a row before tries are turned away for a while.
const ALLOWED_TRIES: u32 = 5;
const WAIT_AFTER_TRIES: Duration = Duration::from_secs(60);

static ADULT_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|[^\p{L}\p{N}])(?:adult(?:s|e|es|o|os|a|as|i)?|x{3,}|porn\p{L}*|eroti[ckq]\p{L}*|hentai|nsfw|yetiskin\p{L}*|erwachsene)(?:[^\p{L}\p{N}]|$)|(?:^|[^\p{N}])(?:18\s*\+|\+\s*18)(?:[^\p{N}]|$)",
    )
    .expect("built-in pattern")
});

/// Names that use "adult" for something else.
const NOT_ADULT: &[&str] = &["adult swim", "young adult"];

/// Whether a category's name looks adult: "XXX", "Adults", "18+",
/// "Adultos", "Adultes", "Porn"… as whole words, in any case or accents.
pub fn looks_adult(name: &str) -> bool {
    let mut folded: String = name.nfkd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase).collect();
    for phrase in NOT_ADULT {
        folded = folded.replace(phrase, " ");
    }
    ADULT_WORDS.is_match(&folded)
}

fn is_valid_pin(pin: &str) -> bool {
    pin.len() == 4 && pin.bytes().all(|byte| byte.is_ascii_digit())
}

fn hash(pin: &str, salt: &[u8]) -> [u8; 32] {
    let mut digest: [u8; 32] = Sha256::new().chain_update(salt).chain_update(pin.as_bytes()).finalize().into();
    for _ in 1..HASH_ROUNDS {
        digest = Sha256::new().chain_update(salt).chain_update(digest).finalize().into();
    }
    digest
}

/// "v1:<salt>:<hash>" in hex, for a new PIN.
fn stored_form(pin: &str) -> String {
    let salt = uuid::Uuid::new_v4().into_bytes();
    format!("v1:{}:{}", hex(&salt), hex(&hash(pin, &salt)))
}

fn matches_stored(pin: &str, stored: &str) -> bool {
    let mut parts = stored.split(':');
    let (Some("v1"), Some(salt), Some(expected), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let (Some(salt), Some(expected)) = (unhex(salt), unhex(expected)) else { return false };
    let actual = hash(pin, &salt);
    // Compared in full whatever differs first.
    expected.len() == actual.len() && expected.iter().zip(actual).fold(0, |difference, (a, b)| difference | (a ^ b)) == 0
}

/// When the PIN was entered, and how long it lasts.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Unlock {
    since: Option<Instant>,
    /// None: until Xorvio quits.
    after: Option<Duration>,
}

impl Unlock {
    /// How much longer the PIN entered holds at `now`: `None` while locked,
    /// `Some(None)` when it holds until Xorvio quits.
    fn remaining(&self, now: Instant) -> Option<Option<Duration>> {
        let since = self.since?;
        match self.after {
            None => Some(None),
            Some(after) => after.checked_sub(now.saturating_duration_since(since)).filter(|left| !left.is_zero()).map(Some),
        }
    }

    fn holds(&self, now: Instant) -> bool {
        self.remaining(now).is_some()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok()).collect()
}

impl Core {
    pub fn parental_status(&self) -> Result<ParentalStatus> {
        let edits = self.load_edits()?;
        if !edits.has_pin {
            return Ok(ParentalStatus::default());
        }
        let remaining = lock(&self.unlock).remaining(Instant::now());
        Ok(ParentalStatus {
            has_pin: true,
            unlocked: remaining.is_some(),
            locked_count: self.locked_ids(&edits)?.len() as i64,
            // Rounded up, so the interface never reloads a moment too soon.
            locks_at: remaining.flatten().map(|left| crate::now() + left.as_secs_f64().ceil() as i64),
        })
    }

    /// How long the PIN holds once entered; `None` until Xorvio quits. It
    /// applies to a PIN already entered too, counted from when it was.
    pub fn set_relock_after(&self, after: Option<Duration>) {
        lock(&self.unlock).after = after;
    }

    /// Creates the PIN, changes it, or (with no new PIN) removes it. Changing
    /// or removing it takes the current one. A new PIN locks what it covers
    /// straight away.
    pub fn set_pin(&self, current_pin: Option<&str>, new_pin: Option<&str>) -> Result<()> {
        if new_pin.is_some_and(|pin| !is_valid_pin(pin)) {
            anyhow::bail!("A PIN is four digits.");
        }
        if let Some(stored) = self.library.preference(PIN_KEY)?
            && !self.pin_matches(current_pin.unwrap_or_default(), &stored)?
        {
            anyhow::bail!("That PIN isn't correct.");
        }
        self.library.set_preference(PIN_KEY, new_pin.map(stored_form).as_deref())?;
        self.lock();
        self.edits_changed();
        Ok(())
    }

    /// Enters the PIN: locked channels and categories show until
    /// [`Core::lock`], the time set by [`Core::set_relock_after`] passes, or
    /// Xorvio quits. False when the PIN is wrong.
    pub fn unlock(&self, pin: &str) -> Result<bool> {
        let Some(stored) = self.library.preference(PIN_KEY)? else { return Ok(true) };
        let matched = self.pin_matches(pin, &stored)?;
        if matched {
            lock(&self.unlock).since = Some(Instant::now());
        }
        Ok(matched)
    }

    /// Locks again until the PIN is next entered.
    pub fn lock(&self) {
        lock(&self.unlock).since = None;
    }

    /// Lists ask this each time, so what's locked hides again by itself once
    /// the time is up.
    pub(crate) fn is_unlocked(&self) -> bool {
        lock(&self.unlock).holds(Instant::now())
    }

    /// Checks a PIN. After several wrong ones in a row, tries are turned
    /// away for a minute, so the PIN can't simply be run through.
    fn pin_matches(&self, pin: &str, stored: &str) -> Result<bool> {
        let earlier = {
            let failures = lock(&self.pin_failures);
            let recent = failures.1.is_some_and(|at| at.elapsed() < WAIT_AFTER_TRIES);
            if recent && failures.0 >= ALLOWED_TRIES {
                anyhow::bail!("Too many wrong PINs. Try again in a minute.");
            }
            if recent { failures.0 } else { 0 }
        };
        let matched = matches_stored(pin, stored);
        *lock(&self.pin_failures) = if matched { (0, None) } else { (earlier + 1, Some(Instant::now())) };
        Ok(matched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_adult_category_names() {
        for name in [
            "XXX",
            "Adults",
            "FR| ADULTES",
            "ES | Adultos +18",
            "18+ Only",
            "VOD - XXX Movies",
            "Porn HD",
            "PORNO",
            "Érotique",
            "TR | YETİŞKİN",
            "Für Erwachsene",
            "Adult",
        ] {
            assert!(looks_adult(name), "{name}");
        }
        for name in ["News", "Adult Swim", "Young Adult Fiction", "Kids 2018+", "XX Sports", "Sports 18", "Adultery Drama"] {
            assert!(!looks_adult(name), "{name}");
        }
    }

    #[test]
    fn an_unlock_lasts_as_long_as_chosen() {
        let entered = Instant::now();
        let minute = Duration::from_secs(60);
        assert!(Unlock::default().remaining(entered).is_none(), "locked until the PIN is entered");
        let forever = Unlock { since: Some(entered), after: None };
        assert_eq!(forever.remaining(entered + 1000 * minute), Some(None), "until Xorvio quits");
        let five = Unlock { since: Some(entered), after: Some(5 * minute) };
        assert_eq!(five.remaining(entered), Some(Some(5 * minute)));
        assert_eq!(five.remaining(entered + 4 * minute), Some(Some(minute)));
        assert!(five.holds(entered + 5 * minute - Duration::from_millis(1)));
        assert!(!five.holds(entered + 5 * minute), "locked again on the minute");
        assert!(!five.holds(entered + 60 * minute));
        // A clock read just before the PIN was entered counts as no time gone.
        assert_eq!(five.remaining(entered.checked_sub(minute).unwrap_or(entered)), Some(Some(5 * minute)));
    }

    #[test]
    fn keeps_only_a_salted_hash() {
        let stored = stored_form("1234");
        assert!(!stored.contains("1234"));
        assert_ne!(stored, stored_form("1234"), "each PIN gets its own salt");
        assert!(matches_stored("1234", &stored));
        assert!(!matches_stored("1235", &stored));
        assert!(!matches_stored("1234", "v1:zz:00"));
        assert!(!matches_stored("1234", ""));
    }
}
