//! Non-secret source settings, kept as a small JSON document (`sources.json`).
//! Logins and links live in the keychain, never here.

use std::path::{Path, PathBuf};

use anyhow::Context;
use xorvio_model::SourceConfig;

pub(crate) struct ConfigFile {
    path: PathBuf,
}

impl ConfigFile {
    pub fn new(directory: &Path) -> Self {
        Self { path: directory.join("sources.json") }
    }

    /// The folder Xorvio's data is in.
    pub fn directory(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
    }

    /// The saved sources in their order. A missing file is no sources; a
    /// damaged one is set aside (so it can be recovered by hand) rather than
    /// silently overwritten.
    pub fn load(&self) -> Vec<SourceConfig> {
        let Ok(text) = std::fs::read_to_string(&self.path) else { return Vec::new() };
        match serde_json::from_str::<Vec<SourceConfig>>(&text) {
            Ok(mut sources) => {
                sources.sort_by_key(|source| source.sort_index);
                sources
            }
            Err(error) => {
                tracing::error!("sources.json could not be read ({error}); keeping a copy as sources.damaged.json");
                let _ = std::fs::copy(&self.path, self.path.with_file_name("sources.damaged.json"));
                Vec::new()
            }
        }
    }

    /// Writes the whole list, replacing the file in one step so a crash never
    /// leaves half a file.
    pub fn save(&self, sources: &[SourceConfig]) -> anyhow::Result<()> {
        let text = serde_json::to_string_pretty(sources)?;
        let directory = self.path.parent().context("sources.json has no folder")?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        std::io::Write::write_all(&mut temporary, text.as_bytes())?;
        temporary.persist(&self.path).context("sources.json could not be saved")?;
        Ok(())
    }
}
