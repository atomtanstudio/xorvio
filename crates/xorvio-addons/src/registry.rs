//! `addons.json`: the installed add-ons in the user's order, whether each is
//! on, and its manifest as last read (so the app works before the add-ons
//! answer). Add-on links are not in it: they live in the secret store.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use xorvio_model::{AddonCatalogInfo, AddonInfo};

use crate::protocol::Manifest;

pub(crate) const FILE_NAME: &str = "addons.json";

/// The secret-store key for an add-on's link (backups name it too).
pub fn secret_key(addon_id: &str) -> String {
    format!("addon-link:{addon_id}")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredAddon {
    pub id: String,
    pub enabled: bool,
    /// Shown in place of the link.
    pub host: String,
    pub manifest: Manifest,
    #[serde(default)]
    pub installed_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    version: u32,
    addons: Vec<StoredAddon>,
}

pub(crate) fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

/// The saved add-ons; a missing file is an empty list. An unreadable file is
/// an error rather than silently empty, so a save can't wipe it.
pub(crate) fn load(path: &Path) -> anyhow::Result<Vec<StoredAddon>> {
    match fs::read(path) {
        Ok(bytes) => {
            let file: RegistryFile =
                serde_json::from_slice(&bytes).context("addons.json couldn't be read")?;
            Ok(file.addons)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).context("addons.json couldn't be opened"),
    }
}

/// Writes through a temporary file and a rename, so a crash mid-write leaves
/// the previous list intact.
pub(crate) fn save(path: &Path, addons: &[StoredAddon]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context("the data folder couldn't be created")?;
    }
    let file = RegistryFile {
        version: 1,
        addons: addons.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&file)?;
    let temp = path.with_extension("json.tmp");
    {
        let mut out = fs::File::create(&temp).context("addons.json couldn't be written")?;
        out.write_all(&bytes)?;
        out.sync_all()?;
    }
    fs::rename(&temp, path).context("addons.json couldn't be saved")?;
    Ok(())
}

/// What the interface shows for an add-on.
pub(crate) fn info(stored: &StoredAddon) -> AddonInfo {
    let manifest = &stored.manifest;
    AddonInfo {
        id: stored.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        description: manifest.description.clone(),
        logo_url: manifest.logo.clone(),
        host: stored.host.clone(),
        enabled: stored.enabled,
        types: manifest.types.clone(),
        resources: manifest.resource_names(),
        catalogs: manifest
            .catalogs
            .iter()
            .map(|catalog| AddonCatalogInfo {
                media_type: catalog.media_type.clone(),
                id: catalog.id.clone(),
                name: catalog.name.clone(),
                genres: catalog.genres.clone(),
                supports_search: catalog.supports_search(),
                browsable: catalog.is_browsable(),
            })
            .collect(),
    }
}
