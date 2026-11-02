//! Interface preferences, kept as `settings.json` in the app data folder. The
//! TypeScript twin is `Settings` in src/lib/types.ts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use xorvio_media::RecordingFolders;
use xorvio_model::LiveStreamFormat;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub appearance: String,
    pub density: String,
    /// What shows when Xorvio opens: "home", "live", "watchlist", or
    /// "lastChannel" (Live TV, playing the channel watched last).
    pub launch_screen: String,
    /// Minutes a guide recording starts early.
    pub record_padding_before: i64,
    /// Minutes a guide recording keeps going after the programme ends.
    pub record_padding_after: i64,
    pub recordings_folder: Option<String>,
    /// Folders chosen for particular sources' recordings, by source id;
    /// sources not here use `recordings_folder`.
    pub source_recording_folders: HashMap<String, String>,
    /// Keep running in the menu bar while recordings are scheduled.
    pub keep_running_for_recordings: bool,
    /// What happens at a recording's ad breaks, which are looked for once
    /// it finishes unless this is off.
    pub commercial_skip: CommercialSkip,
    /// Minutes before a reminded programme starts that its notification goes
    /// out (0 is at the start).
    pub reminder_minutes: i64,
    pub addons_lead: bool,
    pub guide_hours: i64,
    pub guide_row_height: i64,
    pub show_channel_numbers: bool,
    /// Guide cells tinted by the programme's kind ("category") or not ("plain").
    pub guide_colors: String,
    /// Versions of a channel the provider lists several times show as one row.
    pub group_similar_channels: bool,
    /// Channel and category names lose providers' prefixes and tags
    /// ("UK | Channel One FHD" shows as "Channel One").
    pub tidy_names: bool,
    /// Minutes the parental PIN lasts once entered before locking again by
    /// itself; none is until Xorvio quits.
    pub parental_lock_minutes: Option<i64>,
    /// First run's welcome has been seen through or skipped.
    pub onboarding_done: bool,
    pub tvmaze: bool,
    /// TMDB is asked for details at all (with the person's key or the
    /// built-in one).
    pub tmdb: bool,
    /// Episodes' intros, recaps and credits are looked up on IntroDB.
    pub intro_db: bool,
    /// Home's Trakt charts and lists load before an account is connected.
    pub trakt_rows_without_account: bool,
    pub preferred_live_format: LiveStreamFormat,
    /// Play this language's audio track when a film has one ("eng", "fre"…).
    pub audio_language: Option<String>,
    /// Turn on subtitles in this language when there are some; none is off.
    pub subtitle_language: Option<String>,
    /// "small", "medium" or "large".
    pub subtitle_size: String,
    /// Play the next episode when one ends.
    pub autoplay_next_episode: bool,
    /// Skip intros and recaps without asking, rather than offering a button.
    pub skip_intros_automatically: bool,
    /// When live channels play from a buffer on this computer, so they can
    /// be paused and rewound.
    pub live_buffer: LiveBuffer,
    /// How far back the buffer reaches, in minutes (15, 30, 60 or 120).
    pub live_buffer_minutes: i64,
    /// How far behind live a channel plays when it isn't buffered:
    /// "automatic" (the players' own balance), "low" (about two seconds
    /// ahead, for quicker channel changes) or "minimal" (as little as the
    /// players allow, at the cost of more stalls on a weak connection).
    pub live_latency: String,
    /// Keep every event in the diagnostic log, not only warnings and errors.
    pub detailed_diagnostics: bool,
    /// The pop-out player stays above other windows.
    pub keep_pop_out_on_top: bool,
    /// Add-ons and the watchlist have been brought over from the Swift app.
    pub swift_library_imported: bool,
    /// Backing up on a schedule (backup.rs), never with logins.
    pub auto_backup: AutoBackup,
    /// Where automatic backups go; none is a Backups folder in the data folder.
    pub auto_backup_folder: Option<String>,
    /// Keep the library in step with other computers through a shared folder.
    pub sync_enabled: bool,
    pub sync_folder: Option<String>,
    /// A short muted clip of the featured film plays behind Home's hero
    /// (off unless chosen: otherwise backdrops never move).
    pub preview_clips: bool,
    /// A title's trailer plays, muted, behind its film or series page (not
    /// when the Mac asks for reduced motion).
    pub title_trailers: bool,
    /// A notification when a followed series has new episodes.
    pub new_episode_alerts: bool,
}

/// How often Xorvio backs up by itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoBackup {
    #[default]
    Off,
    Daily,
    Weekly,
}

impl AutoBackup {
    /// Seconds between automatic backups, or none when they're off.
    pub fn every(self) -> Option<i64> {
        match self {
            AutoBackup::Off => None,
            AutoBackup::Daily => Some(86_400),
            AutoBackup::Weekly => Some(7 * 86_400),
        }
    }
}

/// Recordings' ad breaks in the player.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommercialSkip {
    /// Not looked for by themselves, nor marked or skipped.
    Off,
    /// Marked on the seek bar, with a Skip Ad button in each.
    #[default]
    Button,
    /// Marked, and skipped once each without asking.
    Automatic,
}

/// When a live channel is buffered (ffmpeg writing it to a temporary folder,
/// which costs a little CPU and some disk while it plays).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiveBuffer {
    /// Never: live channels play directly and can't be paused.
    Off,
    /// From the moment a channel is paused (it resumes where it was paused).
    #[default]
    Paused,
    /// Always, from the moment a channel starts, so it can be rewound too.
    Always,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: "system".into(),
            density: "comfortable".into(),
            launch_screen: "home".into(),
            record_padding_before: 1,
            record_padding_after: 3,
            recordings_folder: None,
            source_recording_folders: HashMap::new(),
            keep_running_for_recordings: true,
            commercial_skip: CommercialSkip::Button,
            reminder_minutes: 5,
            addons_lead: false,
            guide_hours: 3,
            guide_row_height: 60,
            show_channel_numbers: true,
            guide_colors: "category".into(),
            group_similar_channels: true,
            tidy_names: true,
            parental_lock_minutes: Some(15),
            onboarding_done: false,
            tvmaze: true,
            tmdb: true,
            intro_db: true,
            trakt_rows_without_account: false,
            preferred_live_format: LiveStreamFormat::Hls,
            audio_language: None,
            subtitle_language: None,
            subtitle_size: "medium".into(),
            autoplay_next_episode: true,
            skip_intros_automatically: false,
            live_buffer: LiveBuffer::Paused,
            live_buffer_minutes: 30,
            live_latency: "automatic".into(),
            detailed_diagnostics: false,
            keep_pop_out_on_top: false,
            swift_library_imported: false,
            auto_backup: AutoBackup::Off,
            auto_backup_folder: None,
            sync_enabled: false,
            sync_folder: None,
            preview_clips: false,
            title_trailers: true,
            new_episode_alerts: true,
        }
    }
}

impl Settings {
    /// Missing or unreadable settings fall back to the defaults rather than
    /// stopping the app from opening.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// Passes on what the core does differently by these settings, at
    /// launch and each time they're saved.
    pub fn apply_to(&self, core: &xorvio_core::Core) {
        core.set_group_similar_channels(self.group_similar_channels);
        core.set_tidy_names(self.tidy_names);
        core.set_tvmaze_enabled(self.tvmaze);
        core.set_tmdb_enabled(self.tmdb);
        core.set_intro_db_enabled(self.intro_db);
        core.set_trakt_rows_without_account(self.trakt_rows_without_account);
        core.set_relock_after(self.relock_after());
    }

    /// How long an entered PIN lasts: none is until Xorvio quits.
    pub fn relock_after(&self) -> Option<std::time::Duration> {
        self.parental_lock_minutes.filter(|minutes| *minutes > 0).map(|minutes| std::time::Duration::from_secs(minutes.min(24 * 60) as u64 * 60))
    }

    /// The live buffer's window in seconds.
    pub fn live_buffer_seconds(&self) -> f64 {
        self.live_buffer_minutes.clamp(1, 240) as f64 * 60.0
    }

    /// Where recordings go: the chosen folder, or Movies/Xorvio.
    pub fn recordings_dir(&self) -> PathBuf {
        self.recordings_folder.as_ref().map(PathBuf::from).unwrap_or_else(default_recordings_dir)
    }

    /// The folders people chose here: for recordings (all and by source),
    /// automatic backups and sync.
    pub fn folders(&self) -> impl Iterator<Item = &str> {
        self.recordings_folder
            .iter()
            .chain(self.source_recording_folders.values())
            .chain(&self.auto_backup_folder)
            .chain(&self.sync_folder)
            .map(String::as_str)
    }

    /// Every folder recordings can go to, for the recorder.
    pub fn recording_folders(&self) -> RecordingFolders {
        RecordingFolders {
            main: self.recordings_dir(),
            fallback: default_recordings_dir(),
            by_source: self
                .source_recording_folders
                .iter()
                .filter(|(_, folder)| !folder.trim().is_empty())
                .map(|(source_id, folder)| (source_id.clone(), PathBuf::from(folder)))
                .collect(),
        }
    }
}

/// Movies/Xorvio, where recordings go unless another folder is chosen, and
/// when a chosen one isn't there.
pub fn default_recordings_dir() -> PathBuf {
    dirs::video_dir()
        .map(|d| d.join("Xorvio"))
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Xorvio Recordings"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_buffer_settings_read_and_default() {
        let old: Settings = serde_json::from_str(r#"{"appearance":"dark"}"#).unwrap();
        assert_eq!((old.live_buffer, old.live_buffer_minutes), (LiveBuffer::Paused, 30));
        let set: Settings = serde_json::from_str(r#"{"liveBuffer":"always","liveBufferMinutes":120}"#).unwrap();
        assert_eq!((set.live_buffer, set.live_buffer_seconds()), (LiveBuffer::Always, 7200.0));
        let json = serde_json::to_value(Settings { live_buffer: LiveBuffer::Off, ..Settings::default() }).unwrap();
        assert_eq!((json["liveBuffer"].as_str(), json["liveBufferMinutes"].as_i64()), (Some("off"), Some(30)));
    }

    #[test]
    fn ad_breaks_show_a_button_unless_chosen_otherwise() {
        let old: Settings = serde_json::from_str(r#"{"appearance":"dark"}"#).unwrap();
        assert_eq!(old.commercial_skip, CommercialSkip::Button);
        let set: Settings = serde_json::from_str(r#"{"commercialSkip":"automatic"}"#).unwrap();
        assert_eq!(set.commercial_skip, CommercialSkip::Automatic);
        let json = serde_json::to_value(Settings { commercial_skip: CommercialSkip::Off, ..Settings::default() }).unwrap();
        assert_eq!(json["commercialSkip"].as_str(), Some("off"));
    }

    #[test]
    fn recording_folders_by_source() {
        let old: Settings = serde_json::from_str(r#"{"recordingsFolder":"/Volumes/TV/Recordings"}"#).unwrap();
        assert!(old.source_recording_folders.is_empty());
        let folders = old.recording_folders();
        assert_eq!(folders.main, PathBuf::from("/Volumes/TV/Recordings"));
        assert_eq!(folders.fallback, default_recordings_dir());

        let set: Settings = serde_json::from_str(r#"{"sourceRecordingFolders":{"sports":"/Volumes/Sport","news":" "}}"#).unwrap();
        let folders = set.recording_folders();
        assert_eq!(folders.main, default_recordings_dir(), "today's place stays the default");
        assert_eq!(folders.by_source.len(), 1, "an empty choice is no choice");
        assert_eq!(folders.by_source["sports"], PathBuf::from("/Volumes/Sport"));
    }

    #[test]
    fn preview_clips_stay_off_and_alerts_and_trailers_on_until_chosen() {
        let old: Settings = serde_json::from_str(r#"{"appearance":"dark"}"#).unwrap();
        assert!(!old.preview_clips, "backdrops never move unless someone asks");
        assert!(old.new_episode_alerts);
        assert!(old.title_trailers, "trailers play behind title pages unless turned off");
        let chosen: Settings = serde_json::from_str(r#"{"previewClips":true,"newEpisodeAlerts":false,"titleTrailers":false}"#).unwrap();
        assert!(chosen.preview_clips && !chosen.new_episode_alerts && !chosen.title_trailers);
    }

    #[test]
    fn names_and_the_pin_read_and_default() {
        let old: Settings = serde_json::from_str(r#"{"appearance":"dark"}"#).unwrap();
        assert!(old.tidy_names && !old.onboarding_done);
        assert_eq!(old.relock_after(), Some(std::time::Duration::from_secs(15 * 60)));
        let set: Settings = serde_json::from_str(r#"{"tidyNames":false,"parentalLockMinutes":null,"onboardingDone":true}"#).unwrap();
        assert!(!set.tidy_names && set.onboarding_done);
        assert_eq!(set.relock_after(), None, "until Xorvio quits");
        let hours: Settings = serde_json::from_str(r#"{"parentalLockMinutes":240}"#).unwrap();
        assert_eq!(hours.relock_after(), Some(std::time::Duration::from_secs(4 * 3600)));
        let json = serde_json::to_value(Settings { parental_lock_minutes: None, ..Settings::default() }).unwrap();
        assert!(json["parentalLockMinutes"].is_null());
    }
}
