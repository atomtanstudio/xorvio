//! Where credential-bearing settings live: the operating system's keychain in
//! the app, memory in tests. Xorvio stores one entry per source, keyed by the
//! source's id, holding its [`SourceSecrets`] as JSON.

#[cfg(not(target_os = "macos"))]
use anyhow::Context;
use std::collections::HashMap;
use std::sync::Mutex;
use xorvio_model::SourceSecrets;

use crate::store::lock;

/// A small key–value store for secrets.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> anyhow::Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> anyhow::Result<()>;
    fn delete(&self, key: &str) -> anyhow::Result<()>;
}

/// The keychain service every Xorvio secret is filed under.
pub const KEYCHAIN_SERVICE: &str = "com.xorvio.Xorvio";

/// The keychain as messages name it, with Windows' own name for it.
#[cfg(not(target_os = "macos"))]
const KEYCHAIN_NAME: &str = if cfg!(windows) { "Windows Credential Manager" } else { "keychain" };

/// The operating system's keychain (Keychain on macOS, Credential Manager on
/// Windows, the Secret Service on Linux). Each secret is read from it once a
/// run and remembered, rather than on every refresh, play and request.
///
/// On macOS it never lets the keychain ask the person anything. An entry
/// Xorvio can't read without asking (one a differently signed copy of Xorvio
/// saved, a development build say) reads as an error that says to enter the
/// login again, and saving that login replaces the entry with one this copy
/// can read. Entries this copy saved, or one signed the same way (every
/// release), read without asking.
#[derive(Default)]
pub struct KeyringSecrets {
    remembered: Remembered,
    /// The keychain to use instead of the person's own, in tests.
    #[cfg(all(test, target_os = "macos"))]
    keychain: Option<security_framework::os::macos::keychain::SecKeychain>,
}

/// What a read of the keychain found.
#[derive(Debug, Clone, PartialEq)]
enum Found {
    Value(Option<String>),
    /// There's an entry, but macOS would ask the person before Xorvio may read it.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    WouldAsk,
}

const WOULD_ASK: &str = "macOS would ask for your keychain password before Xorvio could read this saved login (a differently signed copy of Xorvio saved it), so Xorvio leaves it. Enter the login again to save it for this copy.";

/// What's been read from the keychain, and saved to it, this run.
#[derive(Default)]
struct Remembered {
    found: Mutex<HashMap<String, Found>>,
}

impl Remembered {
    /// `key`'s secret, read with `read` the first time it's asked for.
    fn get(&self, key: &str, read: impl FnOnce() -> anyhow::Result<Found>) -> anyhow::Result<Option<String>> {
        let known = lock(&self.found).get(key).cloned();
        let found = match known {
            Some(found) => found,
            None => {
                let found = read()?;
                lock(&self.found).insert(key.to_string(), found.clone());
                found
            }
        };
        match found {
            Found::Value(value) => Ok(value),
            Found::WouldAsk => Err(anyhow::anyhow!(WOULD_ASK)),
        }
    }

    fn note(&self, key: &str, value: Option<&str>) {
        lock(&self.found).insert(key.to_string(), Found::Value(value.map(str::to_string)));
    }
}

impl SecretStore for KeyringSecrets {
    fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        self.remembered.get(key, || self.read(key))
    }

    fn set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.write(key, value)?;
        self.remembered.note(key, Some(value));
        Ok(())
    }

    fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.remove(key)?;
        self.remembered.note(key, None);
        Ok(())
    }
}

/// Runs `f` letting macOS ask the person about keychain entries, for reading
/// another app's logins when they've chosen to import them (and were told
/// macOS would ask). Elsewhere Xorvio never lets it ask.
pub fn allowing_keychain_dialogs<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(target_os = "macos")]
    let result = macos::allowing_dialogs(f);
    #[cfg(not(target_os = "macos"))]
    let result = f();
    result
}

#[cfg(not(target_os = "macos"))]
impl KeyringSecrets {
    fn entry(key: &str) -> anyhow::Result<keyring::Entry> {
        keyring::Entry::new(KEYCHAIN_SERVICE, key).with_context(|| format!("The {KEYCHAIN_NAME} could not be opened."))
    }

    fn read(&self, key: &str) -> anyhow::Result<Found> {
        match Self::entry(key)?.get_password() {
            Ok(value) => Ok(Found::Value(Some(value))),
            Err(keyring::Error::NoEntry) => Ok(Found::Value(None)),
            Err(error) => Err(anyhow::anyhow!("The {KEYCHAIN_NAME} could not be read ({error}).")),
        }
    }

    fn write(&self, key: &str, value: &str) -> anyhow::Result<()> {
        Self::entry(key)?
            .set_password(value)
            .map_err(|error| anyhow::anyhow!("The {KEYCHAIN_NAME} could not be written ({error})."))
    }

    fn remove(&self, key: &str) -> anyhow::Result<()> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(anyhow::anyhow!("The {KEYCHAIN_NAME} entry could not be removed ({error}).")),
        }
    }
}

#[cfg(target_os = "macos")]
impl KeyringSecrets {
    /// The person's own keychain, or a test's.
    fn keychain(&self) -> anyhow::Result<security_framework::os::macos::keychain::SecKeychain> {
        #[cfg(test)]
        if let Some(keychain) = &self.keychain {
            return Ok(keychain.clone());
        }
        security_framework::os::macos::keychain::SecKeychain::default().map_err(|error| anyhow::anyhow!("The keychain could not be opened ({error})."))
    }

    fn read(&self, key: &str) -> anyhow::Result<Found> {
        macos::read(&self.keychain()?, KEYCHAIN_SERVICE, key)
    }

    fn write(&self, key: &str, value: &str) -> anyhow::Result<()> {
        macos::write(&self.keychain()?, KEYCHAIN_SERVICE, key, value)
    }

    fn remove(&self, key: &str) -> anyhow::Result<()> {
        macos::remove(&self.keychain()?, KEYCHAIN_SERVICE, key)
    }
}

/// The macOS keychain, with its dialogs turned off.
#[cfg(target_os = "macos")]
mod macos {
    use std::sync::{Mutex, MutexGuard};

    use security_framework::item::{ItemClass, ItemSearchOptions, Reference, SearchResult};
    use security_framework::os::macos::item::ItemSearchOptionsExt;
    use security_framework::os::macos::keychain::{KeychainUserInteractionLock, SecKeychain};
    use security_framework::os::macos::keychain_item::SecKeychainItem;
    use security_framework::os::macos::passwords::find_generic_password;

    use super::Found;
    use crate::store::lock;

    /// errSecItemNotFound.
    const NOT_FOUND: i32 = -25300;
    /// What macOS answers, with its dialogs off, where it would have asked
    /// for the keychain password: errSecAuthFailed, errSecInteractionNotAllowed.
    const WOULD_ASK: [i32; 2] = [-25293, -25308];

    /// While this holds a lock, macOS shows Xorvio no keychain dialogs (the
    /// setting is the whole app's). Every keychain call holds it, so none
    /// runs while an import lets macOS ask.
    static QUIET: Mutex<Option<KeychainUserInteractionLock>> = Mutex::new(None);

    fn quiet() -> MutexGuard<'static, Option<KeychainUserInteractionLock>> {
        let mut held = lock(&QUIET);
        if held.is_none() {
            match SecKeychain::disable_user_interaction() {
                Ok(quiet) => *held = Some(quiet),
                Err(error) => tracing::warn!("the keychain's dialogs couldn't be turned off: {error}"),
            }
        }
        held
    }

    pub(super) fn allowing_dialogs<T>(f: impl FnOnce() -> T) -> T {
        let mut held = lock(&QUIET);
        // Letting the lock go lets macOS ask again.
        held.take();
        let result = f();
        if let Ok(quiet) = SecKeychain::disable_user_interaction() {
            *held = Some(quiet);
        }
        result
    }

    pub(super) fn read(keychain: &SecKeychain, service: &str, account: &str) -> anyhow::Result<Found> {
        let _quiet = quiet();
        match find_generic_password(Some(std::slice::from_ref(keychain)), service, account) {
            Ok((password, _)) => Ok(Found::Value(Some(String::from_utf8(password.to_vec())?))),
            Err(error) if error.code() == NOT_FOUND => Ok(Found::Value(None)),
            Err(error) if WOULD_ASK.contains(&error.code()) => Ok(Found::WouldAsk),
            Err(error) => Err(anyhow::anyhow!("The keychain could not be read ({error}).")),
        }
    }

    pub(super) fn write(keychain: &SecKeychain, service: &str, account: &str, value: &str) -> anyhow::Result<()> {
        let _quiet = quiet();
        if keychain.set_generic_password(service, account, value.as_bytes()).is_ok() {
            return Ok(());
        }
        // An entry this copy may not read or change without asking: it may
        // still be removed, and saved again as this copy's.
        for entry in entries(keychain, service, account) {
            entry.delete();
        }
        keychain
            .add_generic_password(service, account, value.as_bytes())
            .map_err(|error| anyhow::anyhow!("The keychain could not be written ({error})."))
    }

    pub(super) fn remove(keychain: &SecKeychain, service: &str, account: &str) -> anyhow::Result<()> {
        let _quiet = quiet();
        for entry in entries(keychain, service, account) {
            entry.delete();
        }
        if entries(keychain, service, account).is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!("The keychain entry could not be removed."))
        }
    }

    /// The entries for `service` and `account`, found without reading them,
    /// which needs no one's permission.
    fn entries(keychain: &SecKeychain, service: &str, account: &str) -> Vec<SecKeychainItem> {
        ItemSearchOptions::new()
            .keychains(std::slice::from_ref(keychain))
            .class(ItemClass::generic_password())
            .service(service)
            .account(account)
            .load_refs(true)
            .search()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|found| match found {
                SearchResult::Ref(Reference::KeychainItem(item)) => Some(item),
                _ => None,
            })
            .collect()
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

    #[test]
    fn the_keychain_is_read_once_a_secret() {
        let remembered = Remembered::default();
        let reads = std::cell::Cell::new(0);
        let read = |found: Found| {
            reads.set(reads.get() + 1);
            Ok(found)
        };
        assert_eq!(remembered.get("a", || read(Found::Value(Some("one".into())))).unwrap().as_deref(), Some("one"));
        assert_eq!(remembered.get("a", || read(Found::Value(Some("two".into())))).unwrap().as_deref(), Some("one"));
        assert_eq!(remembered.get("missing", || read(Found::Value(None))).unwrap(), None);
        assert_eq!(remembered.get("missing", || read(Found::Value(Some("late".into())))).unwrap(), None);
        assert_eq!(reads.get(), 2, "once each");

        // An entry macOS would ask about is never read again this run, until it's saved anew.
        let refused = remembered.get("other", || read(Found::WouldAsk)).unwrap_err().to_string();
        assert!(refused.contains("Enter the login again"), "{refused}");
        assert!(remembered.get("other", || read(Found::Value(Some("x".into())))).is_err());
        assert_eq!(reads.get(), 3);
        remembered.note("other", Some("new"));
        assert_eq!(remembered.get("other", || read(Found::WouldAsk)).unwrap().as_deref(), Some("new"));
        remembered.note("other", None);
        assert_eq!(remembered.get("other", || read(Found::WouldAsk)).unwrap(), None);
        assert_eq!(reads.get(), 3, "saving and removing are remembered too");

        // A read that fails is tried again next time.
        assert!(remembered.get("flaky", || Err(anyhow::anyhow!("busy"))).is_err());
        assert_eq!(remembered.get("flaky", || read(Found::Value(Some("ok".into())))).unwrap().as_deref(), Some("ok"));
    }

    /// Against a keychain of its own, never the person's: an entry another
    /// app made (here Apple's `security` tool) is reported, not asked about,
    /// and saving over it replaces it with one Xorvio can read.
    #[cfg(target_os = "macos")]
    #[test]
    fn another_apps_entry_is_never_asked_about_and_is_replaced() {
        use security_framework::os::macos::keychain::{CreateOptions, SecKeychain};

        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("test.keychain-db");
        let password = "xorvio-test-keychain";
        let searched = || std::process::Command::new("security").args(["list-keychains", "-d", "user"]).output().unwrap().stdout;
        let before = searched();
        let mut keychain = CreateOptions::new().password(password).create(&path).unwrap();
        keychain.unlock(Some(password)).unwrap();
        assert!(path.exists(), "a keychain file of its own");
        assert_eq!(searched(), before, "not added to the person's keychains");

        let account = format!("xorvio-secrets-test-{}", std::process::id());
        let made = std::process::Command::new("security")
            .args(["add-generic-password", "-s", KEYCHAIN_SERVICE, "-a", &account, "-w", "saved-by-another-app"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(made.success());

        let store = || KeyringSecrets { keychain: Some(keychain.clone()), ..Default::default() };
        let first = store();
        let refused = first.get(&account).unwrap_err().to_string();
        assert!(refused.contains("Enter the login again"), "{refused}");
        assert!(!SecKeychain::user_interaction_allowed().unwrap(), "no dialog could have shown");

        first.set(&account, "entered-again").unwrap();
        assert_eq!(first.get(&account).unwrap().as_deref(), Some("entered-again"));
        assert_eq!(store().get(&account).unwrap().as_deref(), Some("entered-again"), "read from the keychain, not remembered");

        first.delete(&account).unwrap();
        assert_eq!(store().get(&account).unwrap(), None);
        first.delete(&account).unwrap();

        // An import may let macOS ask, and only while it runs.
        assert!(allowing_keychain_dialogs(|| SecKeychain::user_interaction_allowed().unwrap()));
        store().get("anything").unwrap();
        assert!(!SecKeychain::user_interaction_allowed().unwrap());
    }
}
