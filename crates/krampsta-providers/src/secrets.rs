//! Where account credentials live: the system keychain (Windows Credential Manager, macOS
//! Keychain, the Secret Service on Linux), one entry per provider under the service `krampsta`.
//!
//! Only small, long-lived secrets are stored (a GitHub token, a ChatGPT refresh token and
//! account id): Windows caps an entry at 2560 bytes, and short-lived tokens are re-minted.

use std::collections::HashMap;
use std::sync::Mutex;

/// The keychain service every Krampsta entry is filed under.
pub const SERVICE: &str = "krampsta";

/// Reads and writes one secret per key. Calls may block briefly (a keychain round trip).
pub trait SecretStore: Send + Sync + 'static {
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    fn set(&self, key: &str, secret: &str) -> Result<(), String>;
    /// Removing a key that isn't there is not an error.
    fn delete(&self, key: &str) -> Result<(), String>;
}

/// The operating system's keychain.
#[derive(Debug, Default)]
pub struct KeyringStore;

impl KeyringStore {
    fn entry(key: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, key).map_err(|e| format!("the system keychain is unavailable: {e}"))
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        match Self::entry(key)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("could not read `{key}` from the system keychain: {e}")),
        }
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        Self::entry(key)?
            .set_password(secret)
            .map_err(|e| format!("could not save `{key}` in the system keychain: {e}"))
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("could not remove `{key}` from the system keychain: {e}")),
        }
    }
}

/// An in-memory store, for tests and for running without a keychain.
#[derive(Debug, Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        // A panic while holding the lock can't leave the map half-written: every write is one insert.
        self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.entries().get(key).cloned())
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        self.entries().insert(key.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.entries().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_store_round_trips_and_deleting_twice_is_fine() {
        let s = MemoryStore::default();
        assert_eq!(s.get("copilot"), Ok(None));
        s.set("copilot", "gho_x").unwrap();
        assert_eq!(s.get("copilot"), Ok(Some("gho_x".into())));
        s.delete("copilot").unwrap();
        s.delete("copilot").unwrap();
        assert_eq!(s.get("copilot"), Ok(None));
    }
}
