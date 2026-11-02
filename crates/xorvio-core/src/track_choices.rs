//! Audio and subtitles chosen while watching a series, remembered for its
//! other episodes: a Spanish dub, English subtitles, or subtitles off. The
//! player picks the closest match among each episode's own tracks
//! (`src/player/playerRules.ts`). Kept in the library's preferences, one
//! key per series, so nothing new is needed in its schema.

use serde::{Deserialize, Serialize};

use crate::{Core, Result};

/// The preference key a series' choice is kept under.
fn key(series_id: &str) -> String {
    format!("track_choice:{series_id}")
}

/// Subtitles as chosen for a series.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SubtitleChoice {
    Off,
    /// A track by its language (ISO 639-2) and the name the player showed for it.
    Track { language: Option<String>, label: String },
}

/// What's remembered for a series. Fields left out are left to the settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TrackChoice {
    /// The audio language chosen, as an ISO 639-2 code ("spa").
    pub audio_language: Option<String>,
    pub subtitles: Option<SubtitleChoice>,
}

impl Core {
    /// The audio and subtitles chosen for a series, if any.
    pub fn track_choice(&self, series_id: &str) -> Result<Option<TrackChoice>> {
        let Some(text) = self.library.preference(&key(series_id))? else { return Ok(None) };
        // A choice that can't be read (written by a later version) is as good as none.
        Ok(serde_json::from_str(&text).ok())
    }

    /// Remembers the choice for a series, or forgets it (`None`, or a choice with nothing in it).
    pub fn set_track_choice(&self, series_id: &str, choice: Option<&TrackChoice>) -> Result<()> {
        if series_id.trim().is_empty() {
            anyhow::bail!("Only a series' episodes remember their audio and subtitles.");
        }
        let text = match choice {
            Some(choice) if *choice != TrackChoice::default() => Some(serde_json::to_string(choice)?),
            _ => None,
        };
        self.library.set_preference(&key(series_id), text.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::MemorySecrets;

    #[test]
    fn a_series_remembers_its_audio_and_subtitles() {
        let folder = tempfile::tempdir().unwrap();
        let core = Core::open(folder.path(), Arc::new(MemorySecrets::default())).unwrap();
        assert_eq!(core.track_choice("demo|series|1").unwrap(), None);

        let dubbed = TrackChoice {
            audio_language: Some("spa".into()),
            subtitles: Some(SubtitleChoice::Track { language: Some("eng".into()), label: "English (SDH)".into() }),
        };
        core.set_track_choice("demo|series|1", Some(&dubbed)).unwrap();
        assert_eq!(core.track_choice("demo|series|1").unwrap(), Some(dubbed));
        assert_eq!(core.track_choice("demo|series|2").unwrap(), None, "each series has its own");

        let off = TrackChoice { audio_language: None, subtitles: Some(SubtitleChoice::Off) };
        core.set_track_choice("addon|series|tt1", Some(&off)).unwrap();
        assert_eq!(core.track_choice("addon|series|tt1").unwrap(), Some(off));

        // It survives a relaunch.
        drop(core);
        let core = Core::open(folder.path(), Arc::new(MemorySecrets::default())).unwrap();
        assert!(core.track_choice("demo|series|1").unwrap().is_some());

        core.set_track_choice("demo|series|1", None).unwrap();
        assert_eq!(core.track_choice("demo|series|1").unwrap(), None);
        core.set_track_choice("demo|series|2", Some(&TrackChoice::default())).unwrap();
        assert_eq!(core.track_choice("demo|series|2").unwrap(), None, "an empty choice is no choice");
        assert!(core.set_track_choice(" ", Some(&TrackChoice::default())).is_err());
    }

    #[test]
    fn the_interface_s_shape_reads_as_it_is_written() {
        let choice: TrackChoice =
            serde_json::from_str(r#"{"audioLanguage":"fre","subtitles":{"kind":"track","language":null,"label":"Forced"}}"#).unwrap();
        assert_eq!(choice.subtitles, Some(SubtitleChoice::Track { language: None, label: "Forced".into() }));
        let off: TrackChoice = serde_json::from_str(r#"{"subtitles":{"kind":"off"}}"#).unwrap();
        assert_eq!(off, TrackChoice { audio_language: None, subtitles: Some(SubtitleChoice::Off) });
        assert_eq!(serde_json::to_string(&off).unwrap(), r#"{"audioLanguage":null,"subtitles":{"kind":"off"}}"#);
    }
}
