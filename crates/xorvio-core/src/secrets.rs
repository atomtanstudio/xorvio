//! Where credential-bearing settings live: the operating system's keychain in
//! the app, memory in tests. Xorvio stores one entry per source, keyed by the
//! source's id, holding its [`SourceSecrets`] as JSON.

use anyhow::Context;
use std::collections::HashMap;
use std::sync::Mutex;
use xorvio_model::SourceSecrets;

/// A small key–value store for secrets.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> anyhow::Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> anyhow::Result<()>;
    fn delete(&self, key: &str) -> anyhow::Result<()>;
}

/// The keychain service every Xorvio secret is filed under.
pub const KEYCHAIN_SERVICE: &str = "com.xorvio.Xorvio";

/// The keychain as messages name it, with Windows' own name for it.
const KEYCHAIN_NAME: &str = if cfg!(windows) { "Windows Credential Manager" } else { "keychain" };

/// The operating system's keychain (Keychain on macOS, Credential Manager on
/// Windows, the Secret Service on Linux).
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringSecrets;

impl KeyringSecrets {
    fn entry(key: &str) -> anyhow::Result<keyring::Entry> {
        keyring::Entry::new(KEYCHAIN_SERVICE, key).with_context(|| format!("The {KEYCHAIN_NAME} could not be opened."))
    }
}

impl SecretStore for KeyringSecrets {
    fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        match Self::entry(key)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(anyhow::anyhow!("The {KEYCHAIN_NAME} could not be read ({error}).")),
        }
    }

    fn set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        Self::entry(key)?
            .set_password(value)
            .map_err(|error| anyhow::anyhow!("The {KEYCHAIN_NAME} could not be written ({error})."))
    }

    fn delete(&self, key: &str) -> anyhow::Result<()> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(anyhow::anyhow!("The {KEYCHAIN_NAME} entry could not be removed ({error}).")),
        }
    }
}

/// Secrets held in memory, for tests and previews. Nothing is written anywhere.
#[derive(Debug, Default)]
pub struct MemorySecrets {
    values: Mutex<HashMap<String, String>>,
}

impl MemorySecrets {
    pub fn new() -> Self {
        Self::default()
    }

    fn values(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.values.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl SecretStore for MemorySecrets {
    fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(self.values().get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.values().insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.values().remove(key);
        Ok(())
    }
}

/// A source's secrets, or empty ones when nothing is stored or the stored
/// value can't be read.
pub(crate) fn load(store: &dyn SecretStore, source_id: &str) -> anyhow::Result<SourceSecrets> {
    Ok(store
        .get(source_id)?
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default())
}

pub(crate) fn save(store: &dyn SecretStore, source_id: &str, secrets: &SourceSecrets) -> anyhow::Result<()> {
    store.set(source_id, &serde_json::to_string(secrets)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_round_trip() {
        let store = MemorySecrets::new();
        let secrets = SourceSecrets { username: Some("u".into()), password: Some("p".into()), ..Default::default() };
        save(&store, "a", &secrets).unwrap();
        assert_eq!(load(&store, "a").unwrap(), secrets);
        store.delete("a").unwrap();
        assert_eq!(load(&store, "a").unwrap(), SourceSecrets::default());
        store.delete("missing").unwrap();
    }
}
