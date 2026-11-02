use std::collections::HashMap;
use std::sync::Mutex;

/// Where add-on links are kept. Same shape as `xorvio_core::SecretStore`
/// (re-declared so this crate doesn't depend on the core); the app hands both
/// the same keychain.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> anyhow::Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> anyhow::Result<()>;
    fn delete(&self, key: &str) -> anyhow::Result<()>;
}

/// Secrets kept in memory, for tests and previews.
#[derive(Default)]
pub struct MemorySecrets {
    values: Mutex<HashMap<String, String>>,
}

impl MemorySecrets {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecrets {
    fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(self
            .values
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned())
    }

    fn set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.values
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.into(), value.into());
        Ok(())
    }

    fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.values
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
        Ok(())
    }
}
