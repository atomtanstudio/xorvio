//! Series recordings: "record every new episode of …". After every guide
//! refresh, and every 15 minutes, each series looks for its programme in the
//! guide and schedules what it should: every airing once, never one someone
//! cancelled, and with new episodes only, no repeat and no episode it already
//! has. A series that keeps only its latest few recordings moves older ones
//! to the Trash as new ones finish.
//!
//! Which recordings clash for the provider's streams is worked out here too,
//! since a series is what usually schedules more than a provider allows.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::anyhow;
use regex::Regex;
use xorvio_model::{
    Channel, Program, ProgramFlags, Recording, RecordingStatus, ScheduleRequest, SeriesRule,
    SeriesRuleOptions,
};

use super::store::Row;
use super::{Recorder, files, lock, new_row, unix_now};

/// How far ahead series look for airings: further than any guide reaches.
const HORIZON: i64 = 14 * 86_400;
/// How often series look again between guide refreshes.
pub(crate) const EVERY: Duration = Duration::from_secs(15 * 60);
/// Keys in a series' history.
const EPISODE: &str = "episode:";
const AIRING: &str = "airing:";
/// A kept recording that captured less than this share of its programme
/// doesn't count as having the episode, so a later airing is recorded.
const ENOUGH: f64 = 0.5;

/// What series recordings need from the guide.
pub trait SeriesGuide: Send + Sync {
    /// Programmes titled `title` (matched as the app matches titles) that
    /// start between `from` and `to`, soonest first: on `channel_id`, or with
    /// `any_channel` on every channel.
    fn airings(
        &self,
        title: &str,
        channel_id: &str,
        any_channel: bool,
        from: i64,
        to: i64,
    ) -> anyhow::Result<Vec<Airing>>;
}

/// One listing of a series' programme.
#[derive(Debug, Clone, PartialEq)]
pub struct Airing {
    pub program: Program,
    /// The channel to record it from.
    pub channel: Channel,
    /// Shared by the versions of one channel ("News 24 HD", "News 24 4K"),
    /// so the same airing on several of them is recorded once.
    pub family: String,
}

/// A series recording to make, from a programme someone chose.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSeriesRule {
    pub title: String,
    pub source_id: String,
    pub channel_id: String,
    pub channel_name: String,
    pub channel_logo_url: Option<String>,
    pub artwork_url: Option<String>,
    pub options: SeriesRuleOptions,
}

/// An airing a series will record.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Planned {
    pub airing: Airing,
    pub episode_key: Option<String>,
    pub airing_key: String,
}

// MARK: Matching

/// Titles, subtitles and descriptions compared without case or punctuation.
fn text_key(text: &str) -> String {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

static SEASON_EPISODE: LazyLock<[Regex; 2]> = LazyLock::new(|| {
    [
        Regex::new(
            r"(?i)\bs(?:eason)?\s*(\d{1,4})\s*[,.:/-]?\s*e(?:p(?:isode)?)?\.?\s*(\d{1,5})\b",
        )
        .expect("valid pattern"),
        Regex::new(r"\b(\d{1,2})x(\d{1,4})\b").expect("valid pattern"),
    ]
});
static EPISODE_ONLY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\be(?:p(?:isode)?)?\.?\s*(\d{1,5})\b").expect("valid pattern")
});

/// The season and episode in a guide's episode text ("S2 E5", "S02E05",
/// "Season 2 Episode 5", "2x05", "Ep. 5").
fn season_episode(text: &str) -> (Option<i64>, Option<i64>) {
    for pattern in SEASON_EPISODE.iter() {
        if let Some(found) = pattern.captures(text) {
            return (found[1].parse().ok(), found[2].parse().ok());
        }
    }
    let episode = EPISODE_ONLY
        .captures(text)
        .and_then(|found| found[1].parse().ok());
    (None, episode)
}

/// Which episode a programme is: its season and episode number, else its
/// subtitle, else a lone episode number, else its description. `None` when
/// the guide says nothing that tells episodes apart.
pub(crate) fn episode_key(program: &Program) -> Option<String> {
    let (season, episode) = program
        .episode
        .as_deref()
        .map(season_episode)
        .unwrap_or_default();
    if let (Some(season), Some(episode)) = (season, episode) {
        return Some(format!("s{season}e{episode}"));
    }
    let words = |text: &Option<String>| text.as_deref().map(text_key).filter(|key| !key.is_empty());
    if let Some(subtitle) = words(&program.subtitle) {
        return Some(format!("t:{subtitle}"));
    }
    if let Some(episode) = episode {
        return Some(format!("e{episode}"));
    }
    words(&program.summary)
        .map(|summary| format!("d:{}", summary.chars().take(160).collect::<String>()))
}

/// The guide says it was shown before, and doesn't also call it new.
fn is_repeat(flags: &ProgramFlags) -> bool {
    flags.is_repeat && !flags.is_new && !flags.is_premiere
}

fn airing_key(airing: &Airing) -> String {
    format!("{}|{}", airing.family, airing.program.start)
}

/// A recording that has (or will have) its episode: still to record,
/// recording, or kept with most of the programme in it.
pub(crate) fn has_episode(row: &Row) -> bool {
    match row.recording.status {
        RecordingStatus::Scheduled | RecordingStatus::Recording | RecordingStatus::Completed => {
            true
        }
        RecordingStatus::Partial => {
            let length = (row.recording.end - row.recording.start).max(1) as f64;
            row.covered_seconds >= ENOUGH * length
        }
        RecordingStatus::Failed | RecordingStatus::Cancelled => false,
    }
}

/// The airings a series should schedule, soonest first: ones still to start,
/// each once (the same start on versions of one channel is one airing),
/// never one it had before or someone cancelled, and with `new_only`,
/// neither repeats nor episodes it has.
pub(crate) fn plan(
    options: &SeriesRuleOptions,
    airings: Vec<Airing>,
    rows: &[Row],
    history: &HashSet<String>,
    now: i64,
) -> Vec<Planned> {
    let mut taken: HashSet<String> = rows.iter().filter_map(|r| r.airing_key.clone()).collect();
    taken.extend(
        history
            .iter()
            .filter_map(|key| key.strip_prefix(AIRING))
            .map(String::from),
    );
    let mut episodes: HashSet<String> = rows
        .iter()
        .filter(|row| has_episode(row))
        .filter_map(|row| row.episode_key.clone())
        .collect();
    episodes.extend(
        history
            .iter()
            .filter_map(|key| key.strip_prefix(EPISODE))
            .map(String::from),
    );

    let mut planned = Vec::new();
    for airing in airings {
        if airing.program.start <= now {
            continue;
        }
        let key = airing_key(&airing);
        if !taken.insert(key.clone()) {
            continue;
        }
        let episode = episode_key(&airing.program);
        if options.new_only {
            if is_repeat(&airing.program.flags) {
                continue;
            }
            if let Some(episode) = &episode {
                // Guides that give every episode the same description would
                // otherwise record one episode ever; one the guide calls new
                // is new.
                let by_description_only = episode.starts_with("d:");
                let said_new = airing.program.flags.is_new || airing.program.flags.is_premiere;
                if !episodes.insert(episode.clone()) && !(by_description_only && said_new) {
                    continue;
                }
            }
        }
        planned.push(Planned {
            airing,
            episode_key: episode,
            airing_key: key,
        });
    }
    planned
}

/// Finished recordings beyond the latest `keep` (0 keeps them all).
pub(crate) fn beyond_keep(keep: i64, rows: &[Row]) -> Vec<&Row> {
    if keep <= 0 {
        return Vec::new();
    }
    let mut finished: Vec<&Row> = rows
        .iter()
        .filter(|row| {
            matches!(
                row.recording.status,
                RecordingStatus::Completed | RecordingStatus::Partial
            )
        })
        .collect();
    finished.sort_by_key(|row| std::cmp::Reverse((row.recording.start, row.recording.created_at)));
    finished.into_iter().skip(keep as usize).collect()
}

// MARK: Conflicts

/// When a recording holds one of the provider's streams: a live one for its
/// padded time; one from catch-up from when it was asked for, for about the
/// programme's length (it goes as fast as the provider sends it), and at
/// least until now while it runs.
pub(crate) fn stream_window(recording: &Recording, now: i64) -> (i64, i64) {
    let from = recording.start - recording.padding_before_seconds.max(0);
    let until = recording.end + recording.padding_after_seconds.max(0);
    if recording.from_catchup {
        let began = recording.created_at;
        (began, (began + (until - from).max(60)).max(now + 60))
    } else {
        (from, until)
    }
}

/// Scheduled and running recordings that, with others from the same source,
/// need more streams at once than its provider allows, each with a sentence
/// saying so. `limit` gives a source's allowance, when it reports one.
pub(crate) fn conflicts(
    recordings: &[Recording],
    limit: impl Fn(&str) -> Option<i64>,
    now: i64,
) -> HashMap<String, String> {
    let mut by_source: HashMap<&str, Vec<&Recording>> = HashMap::new();
    for recording in recordings.iter().filter(|r| {
        matches!(
            r.status,
            RecordingStatus::Scheduled | RecordingStatus::Recording
        )
    }) {
        by_source
            .entry(recording.source_id.as_str())
            .or_default()
            .push(recording);
    }
    let mut found = HashMap::new();
    for (source, list) in by_source {
        let Some(allowed) = limit(source).filter(|n| *n > 0) else {
            continue;
        };
        if list.len() as i64 <= allowed {
            continue;
        }
        let windows: Vec<(i64, i64)> = list.iter().map(|r| stream_window(r, now)).collect();
        // The most at once is at one of the starts.
        let mut worst: HashMap<usize, usize> = HashMap::new();
        for &(at, _) in &windows {
            let together: Vec<usize> = windows
                .iter()
                .enumerate()
                .filter(|(_, (from, until))| *from <= at && at < *until)
                .map(|(index, _)| index)
                .collect();
            if together.len() as i64 > allowed {
                for index in &together {
                    let most = worst.entry(*index).or_default();
                    *most = (*most).max(together.len());
                }
            }
        }
        for (index, together) in worst {
            let others = together - 1;
            found.insert(
                list[index].id.clone(),
                format!(
                    "Clashes with {others} other recording{}: your provider allows {allowed} stream{} at a time.",
                    if others == 1 { "" } else { "s" },
                    if allowed == 1 { "" } else { "s" },
                ),
            );
        }
    }
    found
}

// MARK: The recorder's series

impl Recorder {
    /// Where series find their airings. Until it's set, series schedule nothing.
    pub fn set_guide(&self, guide: Arc<dyn SeriesGuide>) {
        *self.guide.write().unwrap_or_else(|e| e.into_inner()) = Some(guide);
    }

    /// The padding series use unless they have their own (Settings › Recording).
    /// Once the recorder runs, a change reschedules those series' airings
    /// that haven't started.
    pub fn set_default_padding(&self, before_seconds: i64, after_seconds: i64) {
        let padding = (before_seconds.max(0), after_seconds.max(0));
        let previous = std::mem::replace(&mut *lock(&self.padding), padding);
        if previous == padding {
            return;
        }
        let (Some(runtime), Some(me)) = (self.runtime.get(), self.me.upgrade()) else {
            return;
        };
        runtime.spawn_blocking(move || {
            let rules = match me.store().rules() {
                Ok(rules) => rules,
                Err(error) => {
                    tracing::warn!("series recordings couldn't be read: {error:#}");
                    return;
                }
            };
            for rule in rules {
                let follows = rule.options.padding_before_seconds.is_none()
                    || rule.options.padding_after_seconds.is_none();
                if follows && let Err(error) = me.update_rule(&rule.id, rule.options.clone()) {
                    tracing::warn!("a series couldn't take the new padding: {error:#}");
                }
            }
        });
    }

    /// Hears when recordings come or go other than one by one (a series
    /// scheduled some, or old ones were cleared out), so the list is read again.
    pub fn on_list_change(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self
            .on_list_change
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(callback);
    }

    /// Every series recording, by title, with its next airing and counts.
    pub fn rules(&self) -> anyhow::Result<Vec<SeriesRule>> {
        let (rules, rows) = {
            let store = self.store();
            (store.rules()?, store.list()?)
        };
        let recordings = self.with_conflicts(rows.into_iter().map(|row| row.recording).collect());
        Ok(rules
            .into_iter()
            .map(|rule| counted(rule, &recordings))
            .collect())
    }

    fn rule(&self, id: &str) -> anyhow::Result<SeriesRule> {
        self.rules()?
            .into_iter()
            .find(|rule| rule.id == id)
            .ok_or_else(|| anyhow!("That series recording no longer exists."))
    }

    /// Makes a series recording and schedules what the guide has of it. The
    /// programme it was made from records at once if it's on now. The same
    /// title on the same channel (or on any channel, twice) is the same
    /// series, which takes the new options.
    pub fn add_rule(
        &self,
        new: NewSeriesRule,
        chosen: Option<Airing>,
    ) -> anyhow::Result<SeriesRule> {
        let title = text_key(&new.title);
        if title.is_empty() {
            anyhow::bail!("That programme has no title to look for.");
        }
        let options = tidy(new.options);
        let existing = self.store().rules()?.into_iter().find(|rule| {
            text_key(&rule.title) == title
                && (rule.channel_id == new.channel_id
                    || (rule.options.any_channel && options.any_channel))
        });
        let id = match existing {
            Some(rule) => {
                self.update_rule(&rule.id, options)?;
                rule.id
            }
            None => {
                let rule = SeriesRule {
                    id: uuid::Uuid::new_v4().to_string(),
                    title: new.title.trim().to_string(),
                    source_id: new.source_id,
                    channel_id: new.channel_id,
                    channel_name: new.channel_name,
                    channel_logo_url: new.channel_logo_url,
                    artwork_url: new.artwork_url,
                    options,
                    created_at: unix_now(),
                    next_airing: None,
                    next_channel_name: None,
                    scheduled_count: 0,
                    recorded_count: 0,
                    conflict_count: 0,
                };
                let _one_at_a_time = lock(&self.evaluating);
                self.store().save_rule(&rule)?;
                if let Err(error) = self.evaluate_rule(&rule) {
                    tracing::warn!("a new series couldn't look in the guide: {error:#}");
                }
                rule.id
            }
        };
        if let Some(airing) = chosen.filter(|a| a.program.start <= unix_now()) {
            let rule = self
                .store()
                .rule(&id)?
                .ok_or_else(|| anyhow!("That series recording no longer exists."))?;
            self.record_airing_now(&rule, airing)?;
        }
        self.list_changed();
        self.rule(&id)
    }

    /// Saves a series' options. Airings not yet started are found again
    /// with them; cancelled ones stay cancelled.
    pub fn update_rule(&self, id: &str, options: SeriesRuleOptions) -> anyhow::Result<SeriesRule> {
        let one_at_a_time = lock(&self.evaluating);
        let mut rule = self
            .store()
            .rule(id)?
            .ok_or_else(|| anyhow!("That series recording no longer exists."))?;
        rule.options = tidy(options);
        self.store().save_rule(&rule)?;
        let rows = self.store().rule_rows(id)?;
        for row in rows {
            let running = lock(&self.jobs).contains_key(&row.recording.id);
            if row.recording.status == RecordingStatus::Scheduled && !running {
                self.store().delete(&row.recording.id)?;
            }
        }
        if let Err(error) = self.evaluate_rule(&rule) {
            tracing::warn!("a series couldn't look in the guide: {error:#}");
        }
        self.prune(&rule);
        drop(one_at_a_time);
        self.list_changed();
        self.rule(id)
    }

    /// Deletes a series and its schedule. Its recordings go to the Trash with
    /// `delete_recordings`; otherwise they stay as recordings of their own,
    /// and one that's recording carries on.
    pub fn delete_rule(&self, id: &str, delete_recordings: bool) -> anyhow::Result<()> {
        let _one_at_a_time = lock(&self.evaluating);
        let rows = self.store().rule_rows(id)?;
        for row in rows {
            let recording = &row.recording;
            match recording.status {
                RecordingStatus::Scheduled | RecordingStatus::Cancelled => {
                    if !lock(&self.jobs).contains_key(&recording.id) {
                        self.store().delete(&recording.id)?;
                    }
                }
                RecordingStatus::Recording if delete_recordings => {
                    self.delete(&recording.id, true)?;
                }
                RecordingStatus::Recording => {}
                _ if delete_recordings => {
                    files::trash(&row.parts);
                    self.store().delete(&recording.id)?;
                }
                _ => {}
            }
        }
        self.store().detach_from_rule(id)?;
        self.store().delete_rule(id)?;
        self.list_changed();
        Ok(())
    }

    /// Every series looks in the guide and schedules what it should, then
    /// clears out what it no longer keeps. Reads the guide, so it blocks.
    /// Returns how many airings were scheduled.
    pub fn evaluate_rules(&self) -> anyhow::Result<usize> {
        let _one_at_a_time = lock(&self.evaluating);
        // Providers' allowances can change with a refresh.
        lock(&self.limits).clear();
        let mut scheduled = 0;
        let rules = self.store().rules()?;
        for rule in rules {
            match self.evaluate_rule(&rule) {
                Ok(count) => scheduled += count,
                Err(error) => tracing::warn!("a series couldn't look in the guide: {error:#}"),
            }
            self.prune(&rule);
        }
        if scheduled > 0 {
            self.list_changed();
        }
        Ok(scheduled)
    }

    /// [`Recorder::evaluate_rules`] in the background, as after a guide refresh.
    pub fn evaluate_rules_soon(&self) {
        let (Some(runtime), Some(me)) = (self.runtime.get(), self.me.upgrade()) else {
            return;
        };
        runtime.spawn_blocking(move || {
            if let Err(error) = me.evaluate_rules() {
                tracing::warn!("series recordings couldn't be checked: {error:#}");
            }
        });
    }

    fn guide(&self) -> Option<Arc<dyn SeriesGuide>> {
        self.guide.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn evaluate_rule(&self, rule: &SeriesRule) -> anyhow::Result<usize> {
        let Some(guide) = self.guide() else {
            return Ok(0);
        };
        let now = unix_now();
        let airings = guide.airings(
            &rule.title,
            &rule.channel_id,
            rule.options.any_channel,
            now,
            now + HORIZON,
        )?;
        let (rows, history) = {
            let store = self.store();
            (store.rule_rows(&rule.id)?, store.history(&rule.id)?)
        };
        self.drop_moved(&rows, &airings, now)?;
        let rows: Vec<Row> = self.store().rule_rows(&rule.id)?;
        let planned = plan(&rule.options, airings, &rows, &history, now);
        let count = planned.len();
        for airing in planned {
            self.schedule_airing(rule, airing)?;
        }
        Ok(count)
    }

    /// Takes away scheduled airings the guide no longer lists (it moved the
    /// programme, which is then found at its new time), but only where the
    /// guide reaches past them: an empty or shorter guide drops nothing.
    fn drop_moved(&self, rows: &[Row], airings: &[Airing], now: i64) -> anyhow::Result<()> {
        let Some(reach) = airings.iter().map(|a| a.program.start).max() else {
            return Ok(());
        };
        let listed: HashSet<String> = airings.iter().map(airing_key).collect();
        for row in rows {
            let recording = &row.recording;
            let gone = row
                .airing_key
                .as_ref()
                .is_some_and(|key| !listed.contains(key));
            let waiting = recording.status == RecordingStatus::Scheduled
                && recording.start - recording.padding_before_seconds > now
                && recording.start < reach;
            if gone && waiting && !lock(&self.jobs).contains_key(&recording.id) {
                self.store().delete(&recording.id)?;
                self.list_changed();
            }
        }
        Ok(())
    }

    fn padding_for(&self, rule: &SeriesRule) -> (i64, i64) {
        let (before, after) = *lock(&self.padding);
        (
            rule.options.padding_before_seconds.unwrap_or(before),
            rule.options.padding_after_seconds.unwrap_or(after),
        )
    }

    fn request_for(&self, rule: &SeriesRule, airing: &Airing) -> ScheduleRequest {
        let (before, after) = self.padding_for(rule);
        let program = &airing.program;
        ScheduleRequest {
            source_id: airing.channel.source_id.clone(),
            channel_id: airing.channel.id.clone(),
            channel_name: airing.channel.name.clone(),
            channel_logo_url: airing.channel.logo_url.clone(),
            title: program.title.clone(),
            subtitle: program.subtitle.clone(),
            summary: program.summary.clone(),
            artwork_url: program
                .image_url
                .clone()
                .or_else(|| rule.artwork_url.clone()),
            program_id: Some(program.id.clone()),
            start: program.start,
            end: program.end,
            padding_before_seconds: before,
            padding_after_seconds: after,
        }
    }

    fn schedule_airing(&self, rule: &SeriesRule, planned: Planned) -> anyhow::Result<()> {
        let request = self.request_for(rule, &planned.airing);
        // Already scheduled by hand: that recording stands for it.
        if self.existing(&request)?.is_some() {
            return Ok(());
        }
        let now = unix_now();
        let wanted_from = request.start.max(now);
        let mut row = new_row(request, now, wanted_from);
        row.recording.rule_id = Some(rule.id.clone());
        row.recording.episode = planned.airing.program.episode.clone();
        row.episode_key = planned.episode_key;
        row.airing_key = Some(planned.airing_key);
        self.store().insert(&row)?;
        self.emit(&row.recording.id);
        if now >= row.recording.start - row.recording.padding_before_seconds {
            self.start_job(row);
        }
        Ok(())
    }

    /// The programme a series was made from, when it's on now.
    fn record_airing_now(&self, rule: &SeriesRule, airing: Airing) -> anyhow::Result<()> {
        let key = airing_key(&airing);
        let rows = self.store().rule_rows(&rule.id)?;
        if airing.program.end <= unix_now()
            || rows.iter().any(|r| r.airing_key.as_ref() == Some(&key))
        {
            return Ok(());
        }
        let episode_key = episode_key(&airing.program);
        self.schedule_airing(
            rule,
            Planned {
                airing,
                episode_key,
                airing_key: key,
            },
        )
    }

    /// Moves a series' finished recordings beyond the latest it keeps to the
    /// Trash. They stay in its history, so they aren't recorded again.
    pub(crate) fn prune(&self, rule: &SeriesRule) {
        let rows = match self.store().rule_rows(&rule.id) {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!("a series' recordings couldn't be read: {error:#}");
                return;
            }
        };
        let old = beyond_keep(rule.options.keep, &rows);
        if old.is_empty() {
            return;
        }
        let now = unix_now();
        for row in old {
            files::trash(&row.parts);
            let store = self.store();
            if let Some(key) = &row.airing_key {
                let _ = store.remember(&rule.id, &format!("{AIRING}{key}"), now);
            }
            if let Err(error) = store.delete(&row.recording.id) {
                tracing::error!("an old recording couldn't be removed: {error:#}");
            }
        }
        self.list_changed();
    }

    /// A series recording finished: its episode is had (when enough of it
    /// was captured), and older ones may now go.
    pub(crate) fn series_recording_ended(&self, id: &str) {
        let Some(row) = self.current_row(id) else {
            return;
        };
        let Some(rule_id) = row.recording.rule_id.clone() else {
            return;
        };
        if has_episode(&row)
            && let Some(episode) = &row.episode_key
        {
            let _ = self
                .store()
                .remember(&rule_id, &format!("{EPISODE}{episode}"), unix_now());
        }
        let rule = self.store().rule(&rule_id);
        if let Ok(Some(rule)) = rule {
            self.prune(&rule);
        }
    }

    /// A series' airing was cancelled or deleted: it isn't scheduled again.
    pub(crate) fn forget_airing(&self, row: &Row) {
        if let (Some(rule_id), Some(key)) = (&row.recording.rule_id, &row.airing_key) {
            let _ = self
                .store()
                .remember(rule_id, &format!("{AIRING}{key}"), unix_now());
        }
    }

    pub(crate) fn list_changed(&self) {
        let callback = self
            .on_list_change
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(callback) = callback {
            callback();
        }
    }

    /// The provider's allowance for a source, asked once until series look
    /// again.
    fn limit(&self, source_id: &str) -> Option<i64> {
        if let Some(known) = lock(&self.limits).get(source_id) {
            return *known;
        }
        let limit = self.resolver.max_connections(source_id);
        lock(&self.limits).insert(source_id.to_string(), limit);
        limit
    }

    /// Fills in which recordings clash. Takes the store's lock itself, so
    /// call it without holding it.
    pub(crate) fn with_conflicts(&self, mut recordings: Vec<Recording>) -> Vec<Recording> {
        let found = conflicts(&recordings, |source| self.limit(source), unix_now());
        for recording in &mut recordings {
            recording.conflict = found.get(&recording.id).cloned();
        }
        recordings
    }

    /// How many recordings from a source hold a stream at some point
    /// between `from` and `to`, for the warning before adding another.
    pub fn streams_in_use(&self, source_id: &str, from: i64, to: i64) -> anyhow::Result<usize> {
        let now = unix_now();
        Ok(self
            .store()
            .pending()?
            .iter()
            .map(|row| &row.recording)
            .filter(|r| r.source_id == source_id)
            .filter(|r| {
                let (start, end) = stream_window(r, now);
                start < to && from < end
            })
            .count())
    }
}

/// Options within what the interface offers.
fn tidy(mut options: SeriesRuleOptions) -> SeriesRuleOptions {
    options.keep = options.keep.clamp(0, 100);
    options.padding_before_seconds = options.padding_before_seconds.map(|s| s.clamp(0, 3600));
    options.padding_after_seconds = options.padding_after_seconds.map(|s| s.clamp(0, 4 * 3600));
    options
}

/// A series with its next airing and counts, from the recordings.
fn counted(mut rule: SeriesRule, recordings: &[Recording]) -> SeriesRule {
    let mine: Vec<&Recording> = recordings
        .iter()
        .filter(|r| r.rule_id.as_deref() == Some(rule.id.as_str()))
        .collect();
    let next = mine
        .iter()
        .filter(|r| r.status == RecordingStatus::Scheduled)
        .min_by_key(|r| r.start);
    rule.next_airing = next.map(|r| r.start);
    rule.next_channel_name = next.map(|r| r.channel_name.clone());
    rule.scheduled_count = mine
        .iter()
        .filter(|r| r.status == RecordingStatus::Scheduled)
        .count() as i64;
    rule.recorded_count = mine
        .iter()
        .filter(|r| {
            matches!(
                r.status,
                RecordingStatus::Completed | RecordingStatus::Partial
            )
        })
        .count() as i64;
    rule.conflict_count = mine.iter().filter(|r| r.conflict.is_some()).count() as i64;
    rule
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use xorvio_model::StreamTarget;

    use crate::recorder::ChannelResolver;

    const NOW: i64 = 1_800_000_000;

    fn program(title: &str, start: i64, episode: Option<&str>, flags: ProgramFlags) -> Program {
        Program {
            id: format!("g|epg|{start}"),
            source_id: "g".into(),
            epg_id: "epg".into(),
            start,
            end: start + 3600,
            title: title.into(),
            subtitle: None,
            summary: None,
            category: None,
            image_url: None,
            episode: episode.map(String::from),
            flags,
            age_rating: None,
            star_rating: None,
            year: None,
            genre: None,
        }
    }

    fn channel(id: &str, name: &str) -> Channel {
        Channel {
            id: format!("s|ch|{id}"),
            source_id: "s".into(),
            name: name.into(),
            number: None,
            logo_url: None,
            category_id: None,
            epg_id: Some("epg".into()),
            stream: xorvio_model::StreamLocator::Url {
                url: "http://x.example/1.ts".into(),
            },
            catchup_days: 0,
            catchup_source: None,
            sort_index: 0,
            catchup_offset_minutes: 0,
            versions: vec![],
        }
    }

    fn airing(start: i64, episode: Option<&str>) -> Airing {
        Airing {
            program: program("Tidewater", start, episode, ProgramFlags::default()),
            channel: channel("1", "Harbor One HD"),
            family: "harborone".into(),
        }
    }

    fn starts(planned: &[Planned]) -> Vec<i64> {
        planned.iter().map(|p| p.airing.program.start).collect()
    }

    #[test]
    fn reads_episode_numbers() {
        assert_eq!(season_episode("S2 E5"), (Some(2), Some(5)));
        assert_eq!(season_episode("S02E05"), (Some(2), Some(5)));
        assert_eq!(season_episode("Season 3, Episode 12"), (Some(3), Some(12)));
        assert_eq!(season_episode("2x05"), (Some(2), Some(5)));
        assert_eq!(season_episode("Ep. 7"), (None, Some(7)));
        assert_eq!(season_episode("E7"), (None, Some(7)));
        assert_eq!(season_episode("Part one"), (None, None));

        let mut p = program("Show", NOW, Some("S1 E2"), ProgramFlags::default());
        p.subtitle = Some("Low Tide".into());
        assert_eq!(episode_key(&p).as_deref(), Some("s1e2"));
        p.episode = Some("E2".into());
        assert_eq!(
            episode_key(&p).as_deref(),
            Some("t:low tide"),
            "the subtitle before a lone number"
        );
        p.subtitle = None;
        assert_eq!(episode_key(&p).as_deref(), Some("e2"));
        p.episode = None;
        p.summary = Some("Mara finds the map.".into());
        assert_eq!(episode_key(&p).as_deref(), Some("d:mara finds the map"));
        p.summary = None;
        assert_eq!(episode_key(&p), None);
    }

    #[test]
    fn new_episodes_only_skips_ones_it_has_and_repeats() {
        let options = SeriesRuleOptions::default();
        assert!(options.new_only, "new episodes only is the default");
        let repeat = ProgramFlags {
            is_repeat: true,
            ..Default::default()
        };
        let mut shown_before = airing(NOW + 7200, Some("S1 E3"));
        shown_before.program.flags = repeat.clone();
        let mut repeat_but_new = airing(NOW + 9000, Some("S1 E9"));
        repeat_but_new.program.flags = ProgramFlags {
            is_new: true,
            ..repeat
        };
        let airings = vec![
            airing(NOW - 600, Some("S1 E1")), // already on
            airing(NOW + 3600, Some("S1 E2")),
            shown_before,
            airing(NOW + 5400, Some("S1 E2")), // the same episode again
            repeat_but_new,
            airing(NOW + 86_400, Some("S1 E4")),
            airing(NOW + 90_000, None), // the guide can't tell
            airing(NOW + 93_600, None),
        ];
        let planned = plan(&options, airings.clone(), &[], &HashSet::new(), NOW);
        assert_eq!(
            starts(&planned),
            [
                NOW + 3600,
                NOW + 9000,
                NOW + 86_400,
                NOW + 90_000,
                NOW + 93_600
            ]
        );
        assert_eq!(planned[0].episode_key.as_deref(), Some("s1e2"));

        // An episode it has, or had before its recording was deleted, isn't recorded again.
        let history = HashSet::from(["episode:s1e4".to_string()]);
        let mut had = row_for(&planned[0], RecordingStatus::Completed);
        had.airing_key = Some("elsewhere|1".into());
        let planned = plan(&options, airings.clone(), &[had], &history, NOW);
        assert_eq!(starts(&planned), [NOW + 9000, NOW + 90_000, NOW + 93_600]);

        // All episodes: repeats too, each airing once.
        let all = SeriesRuleOptions {
            new_only: false,
            ..options
        };
        assert_eq!(plan(&all, airings, &[], &HashSet::new(), NOW).len(), 7);
    }

    #[test]
    fn a_failed_or_mostly_missed_episode_is_recorded_again() {
        let options = SeriesRuleOptions::default();
        let first = plan(
            &options,
            vec![airing(NOW + 3600, Some("S1 E2"))],
            &[],
            &HashSet::new(),
            NOW,
        );
        let mut failed = row_for(&first[0], RecordingStatus::Failed);
        failed.airing_key = Some("other|1".into());
        let mut thin = failed.clone();
        thin.recording.status = RecordingStatus::Partial;
        thin.covered_seconds = 600.0;
        let mut most = thin.clone();
        most.covered_seconds = 3000.0;
        let again = vec![airing(NOW + 7200, Some("S1 E2"))];
        assert_eq!(
            plan(&options, again.clone(), &[failed], &HashSet::new(), NOW).len(),
            1
        );
        assert_eq!(
            plan(&options, again.clone(), &[thin], &HashSet::new(), NOW).len(),
            1
        );
        assert!(plan(&options, again, &[most], &HashSet::new(), NOW).is_empty());
    }

    #[test]
    fn a_generic_description_doesnt_stop_new_episodes() {
        let options = SeriesRuleOptions::default();
        let with = |start: i64, new: bool| {
            let mut a = airing(start, None);
            a.program.summary = Some("The day's news.".into());
            a.program.flags.is_new = new;
            a
        };
        let planned = plan(
            &options,
            vec![
                with(NOW + 60, true),
                with(NOW + 86_400, true),
                with(NOW + 90_000, false),
            ],
            &[],
            &HashSet::new(),
            NOW,
        );
        assert_eq!(starts(&planned), [NOW + 60, NOW + 86_400]);
    }

    #[test]
    fn any_channel_records_an_airing_on_several_versions_once() {
        let options = SeriesRuleOptions {
            any_channel: true,
            new_only: false,
            ..Default::default()
        };
        let on = |id: &str, name: &str, family: &str, start: i64| Airing {
            channel: channel(id, name),
            family: family.into(),
            ..airing(start, None)
        };
        let airings = vec![
            on("4k", "Harbor One 4K", "harborone", NOW + 3600),
            on("hd", "Harbor One HD", "harborone", NOW + 3600),
            on("fhd", "UK: Harbor One FHD", "harborone", NOW + 3600),
            on("plus", "Harbor One +1", "harborone+1", NOW + 7200),
            on("other", "Summit", "summit", NOW + 3600),
            on("hd", "Harbor One HD", "harborone", NOW + 86_400),
        ];
        let planned = plan(&options, airings, &[], &HashSet::new(), NOW);
        let picked: Vec<(&str, i64)> = planned
            .iter()
            .map(|p| (p.airing.channel.id.as_str(), p.airing.program.start))
            .collect();
        assert_eq!(
            picked,
            [
                ("s|ch|4k", NOW + 3600),
                ("s|ch|plus", NOW + 7200),
                ("s|ch|other", NOW + 3600),
                ("s|ch|hd", NOW + 86_400),
            ],
            "the first listed version (the guide puts the best first), the +1 and another channel apart"
        );
    }

    fn row_for(planned: &Planned, status: RecordingStatus) -> Row {
        let request = ScheduleRequest {
            source_id: "s".into(),
            channel_id: planned.airing.channel.id.clone(),
            channel_name: planned.airing.channel.name.clone(),
            channel_logo_url: None,
            title: planned.airing.program.title.clone(),
            subtitle: None,
            summary: None,
            artwork_url: None,
            program_id: Some(planned.airing.program.id.clone()),
            start: planned.airing.program.start,
            end: planned.airing.program.end,
            padding_before_seconds: 0,
            padding_after_seconds: 0,
        };
        let mut row = new_row(request, NOW, planned.airing.program.start);
        row.recording.status = status;
        row.recording.rule_id = Some("rule".into());
        row.episode_key = planned.episode_key.clone();
        row.airing_key = Some(planned.airing_key.clone());
        row
    }

    #[test]
    fn keeps_the_latest_finished_recordings() {
        let rows: Vec<Row> = (0..6)
            .map(|day| {
                let planned = &plan(
                    &SeriesRuleOptions::default(),
                    vec![airing(NOW + day * 86_400 + 60, None)],
                    &[],
                    &HashSet::new(),
                    NOW,
                )[0];
                let status = match day {
                    4 => RecordingStatus::Failed,
                    5 => RecordingStatus::Scheduled,
                    _ => RecordingStatus::Completed,
                };
                row_for(planned, status)
            })
            .collect();
        assert!(beyond_keep(0, &rows).is_empty(), "0 keeps them all");
        let old: Vec<i64> = beyond_keep(3, &rows)
            .iter()
            .map(|r| r.recording.start)
            .collect();
        assert_eq!(
            old,
            [NOW + 60],
            "the oldest finished one; failed and scheduled ones aren't counted"
        );
        assert_eq!(beyond_keep(1, &rows).len(), 3);
    }

    fn recording(
        id: &str,
        source: &str,
        start: i64,
        end: i64,
        status: RecordingStatus,
    ) -> Recording {
        let mut row = new_row(
            ScheduleRequest {
                source_id: source.into(),
                channel_id: format!("{source}|ch|{id}"),
                channel_name: id.into(),
                channel_logo_url: None,
                title: id.into(),
                subtitle: None,
                summary: None,
                artwork_url: None,
                program_id: None,
                start,
                end,
                padding_before_seconds: 60,
                padding_after_seconds: 180,
            },
            NOW,
            start,
        );
        row.recording.id = id.into();
        row.recording.status = status;
        row.recording
    }

    #[test]
    fn conflicts_are_recordings_beyond_the_providers_streams() {
        use RecordingStatus::*;
        let list = vec![
            recording("a", "two", NOW + 3600, NOW + 7200, Scheduled),
            recording("b", "two", NOW + 5400, NOW + 9000, Scheduled),
            recording("c", "two", NOW + 6000, NOW + 6600, Scheduled),
            // After the others: no clash, though the padding nearly touches.
            recording("d", "two", NOW + 9200, NOW + 10_000, Scheduled),
            // Another source, and finished or cancelled ones, don't count.
            recording("e", "one", NOW + 6000, NOW + 6600, Scheduled),
            recording("f", "two", NOW + 6000, NOW + 6600, Cancelled),
        ];
        let limit = |source: &str| match source {
            "two" => Some(2),
            "one" => Some(1),
            _ => None,
        };
        let found = conflicts(&list, limit, NOW);
        let mut ids: Vec<&str> = found.keys().map(String::as_str).collect();
        ids.sort();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(
            found["c"],
            "Clashes with 2 other recordings: your provider allows 2 streams at a time."
        );
        assert!(
            conflicts(&list, |_| None, NOW).is_empty(),
            "no allowance, no conflicts"
        );
        assert!(conflicts(&list, |_| Some(3), NOW).is_empty());

        // A catch-up save takes a stream now, whatever time the programme was.
        let mut saving = recording("g", "one", NOW - 7200, NOW - 3600, Recording);
        saving.from_catchup = true;
        saving.created_at = NOW - 30;
        let mut live = recording("h", "one", NOW - 600, NOW + 600, Recording);
        live.padding_before_seconds = 0;
        let found = conflicts(&[saving, live], |_| Some(1), NOW);
        assert_eq!(found.len(), 2);
        assert!(found["h"].starts_with("Clashes with 1 other recording:"));
    }

    // MARK: The recorder's series, with a guide that's a list

    struct Listings(Mutex<Vec<Airing>>);

    impl SeriesGuide for Listings {
        fn airings(
            &self,
            title: &str,
            channel_id: &str,
            any: bool,
            from: i64,
            to: i64,
        ) -> anyhow::Result<Vec<Airing>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|a| text_key(&a.program.title) == text_key(title))
                .filter(|a| any || a.channel.id == channel_id)
                .filter(|a| from <= a.program.start && a.program.start < to)
                .cloned()
                .collect())
        }
    }

    struct Limited;

    impl ChannelResolver for Limited {
        fn resolve(&self, _: &str, _: &str) -> anyhow::Result<StreamTarget> {
            anyhow::bail!("no streams in this test")
        }
        fn max_connections(&self, _: &str) -> Option<i64> {
            Some(1)
        }
    }

    fn live(start_in: i64, episode: &str) -> Airing {
        let now = unix_now();
        let mut a = airing(now + start_in, Some(episode));
        a.program.id = format!("g|epg|{}", now + start_in);
        a
    }

    fn new_rule(options: SeriesRuleOptions) -> NewSeriesRule {
        NewSeriesRule {
            title: "Tidewater".into(),
            source_id: "s".into(),
            channel_id: "s|ch|1".into(),
            channel_name: "Harbor One".into(),
            channel_logo_url: None,
            artwork_url: None,
            options,
        }
    }

    #[test]
    fn series_schedule_once_and_cancelled_airings_stay_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let recorder =
            Recorder::open(dir.path(), dir.path().join("rec"), None, Arc::new(Limited)).unwrap();
        let listings = Arc::new(Listings(Mutex::new(vec![
            live(3600, "S1 E1"),
            live(3 * 3600, "S1 E2"),
            live(86_400, "S1 E3"),
        ])));
        recorder.set_guide(listings.clone());
        recorder.set_default_padding(60, 180);
        let changes = Arc::new(Mutex::new(0));
        let heard = changes.clone();
        recorder.on_list_change(Arc::new(move || *heard.lock().unwrap() += 1));

        let rule = recorder
            .add_rule(new_rule(SeriesRuleOptions::default()), None)
            .unwrap();
        assert_eq!(rule.scheduled_count, 3);
        assert_eq!(
            rule.next_airing,
            Some(listings.0.lock().unwrap()[0].program.start)
        );
        assert!(*changes.lock().unwrap() > 0, "the interface hears of it");
        let scheduled = recorder.list().unwrap();
        assert!(
            scheduled
                .iter()
                .all(|r| r.rule_id.as_deref() == Some(rule.id.as_str()))
        );
        assert_eq!(scheduled[0].episode.as_deref(), Some("S1 E1"));
        assert_eq!(
            (
                scheduled[0].padding_before_seconds,
                scheduled[0].padding_after_seconds
            ),
            (60, 180),
            "the default padding"
        );
        assert!(
            scheduled.iter().all(|r| r.conflict.is_none()),
            "hours apart, one stream is enough"
        );

        // Looking again schedules nothing twice.
        assert_eq!(recorder.evaluate_rules().unwrap(), 0);
        assert_eq!(recorder.list().unwrap().len(), 3);

        // Cancelling one: it stays cancelled however often the series looks,
        // even after the cancelled one is deleted.
        recorder.cancel(&scheduled[1].id).unwrap();
        assert_eq!(recorder.evaluate_rules().unwrap(), 0);
        recorder.delete(&scheduled[1].id, false).unwrap();
        assert_eq!(recorder.evaluate_rules().unwrap(), 0);
        let left: Vec<i64> = recorder.list().unwrap().iter().map(|r| r.start).collect();
        assert_eq!(left, [scheduled[0].start, scheduled[2].start]);

        // A new episode in the guide is found; the same title again is the same series.
        listings.0.lock().unwrap().push(live(2 * 86_400, "S1 E4"));
        let again = recorder
            .add_rule(
                new_rule(SeriesRuleOptions {
                    keep: 3,
                    ..Default::default()
                }),
                None,
            )
            .unwrap();
        assert_eq!(again.id, rule.id);
        assert_eq!(again.options.keep, 3);
        assert_eq!(again.scheduled_count, 3);
        assert_eq!(recorder.rules().unwrap().len(), 1);

        // A clash: an airing overlapping another, with one stream allowed.
        listings.0.lock().unwrap().push(live(3600 + 1800, "S1 E5"));
        assert_eq!(recorder.evaluate_rules().unwrap(), 1);
        let clashing: Vec<Recording> = recorder
            .list()
            .unwrap()
            .into_iter()
            .filter(|r| r.conflict.is_some())
            .collect();
        assert_eq!(clashing.len(), 2);
        assert_eq!(
            clashing[0].conflict.as_deref(),
            Some("Clashes with 1 other recording: your provider allows 1 stream at a time.")
        );
        assert_eq!(recorder.rules().unwrap()[0].conflict_count, 2);

        // Changing to all episodes finds them again with the new options, cancelled one aside.
        let all = recorder
            .update_rule(
                &rule.id,
                SeriesRuleOptions {
                    new_only: false,
                    padding_after_seconds: Some(0),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(all.scheduled_count, 4);
        assert!(
            recorder
                .list()
                .unwrap()
                .iter()
                .all(|r| r.padding_after_seconds == 0)
        );

        // Deleting the series takes its schedule with it.
        recorder.delete_rule(&rule.id, false).unwrap();
        assert!(recorder.rules().unwrap().is_empty());
        assert!(recorder.list().unwrap().is_empty());
    }

    #[test]
    fn a_moved_programme_is_rescheduled_at_its_new_time() {
        let dir = tempfile::tempdir().unwrap();
        let recorder =
            Recorder::open(dir.path(), dir.path().join("rec"), None, Arc::new(Limited)).unwrap();
        let listings = Arc::new(Listings(Mutex::new(vec![
            live(3600, "S1 E1"),
            live(86_400, "S1 E2"),
        ])));
        recorder.set_guide(listings.clone());
        recorder
            .add_rule(new_rule(SeriesRuleOptions::default()), None)
            .unwrap();
        // The guide moves the first half an hour later.
        listings.0.lock().unwrap()[0] = live(5400, "S1 E1");
        recorder.evaluate_rules().unwrap();
        let starts: Vec<i64> = recorder.list().unwrap().iter().map(|r| r.start).collect();
        let now = unix_now();
        assert_eq!(starts.len(), 2);
        assert!((starts[0] - (now + 5400)).abs() <= 2, "{starts:?}");
        // An empty guide (cleared, or a refresh that failed) drops nothing.
        listings.0.lock().unwrap().clear();
        recorder.evaluate_rules().unwrap();
        assert_eq!(recorder.list().unwrap().len(), 2);
    }

    #[test]
    fn keeping_the_latest_few_clears_out_older_ones_but_remembers_them() {
        let dir = tempfile::tempdir().unwrap();
        let recorder =
            Recorder::open(dir.path(), dir.path().join("rec"), None, Arc::new(Limited)).unwrap();
        let rule = recorder
            .add_rule(
                new_rule(SeriesRuleOptions {
                    keep: 2,
                    ..Default::default()
                }),
                None,
            )
            .unwrap();
        // Three finished episodes, as the recorder would have left them.
        let now = unix_now();
        let mut files = Vec::new();
        for day in 1..=3 {
            let planned = Planned {
                airing: airing(now - day * 86_400, Some(&format!("S1 E{}", 4 - day))),
                episode_key: Some(format!("s1e{}", 4 - day)),
                airing_key: format!("harborone|{}", now - day * 86_400),
            };
            let mut row = row_for(&planned, RecordingStatus::Completed);
            row.recording.rule_id = Some(rule.id.clone());
            let file = dir.path().join(format!("episode {day}.ts"));
            std::fs::write(&file, b"x").unwrap();
            row.parts = vec![file.to_string_lossy().into_owned()];
            recorder.store().insert(&row).unwrap();
            recorder.series_recording_ended(&row.recording.id);
            files.push(file);
        }
        assert_eq!(recorder.list().unwrap().len(), 2);
        assert!(!files[2].exists(), "the oldest went");
        assert!(files[0].exists() && files[1].exists());

        // The cleared-out episode isn't recorded again when it repeats.
        let listings = Arc::new(Listings(Mutex::new(vec![
            live(3600, "S1 E1"),
            live(7200, "S1 E9"),
        ])));
        recorder.set_guide(listings);
        assert_eq!(recorder.evaluate_rules().unwrap(), 1);
        assert_eq!(
            recorder
                .list()
                .unwrap()
                .iter()
                .filter(|r| r.status == RecordingStatus::Scheduled)
                .count(),
            1
        );

        // Deleting the series with its recordings removes them.
        recorder.delete_rule(&rule.id, true).unwrap();
        assert!(recorder.list().unwrap().is_empty());
        assert!(!files[0].exists() && !files[1].exists());
    }
}
