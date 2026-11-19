//! Where the refresh token lives. Only the operating system credential store is used in
//! production: Windows Credential Manager, Secret Service on Linux, Keychain on macOS.
//! Nothing is written to a file and nothing is logged.

use std::sync::Mutex;

use crate::config::{DriveConfig, KEYRING_SERVICE};
use crate::error::DriveError;
use crate::secret::Secret;

/// Storage for the long-lived refresh token (the access token is only kept in memory).
pub trait TokenStore: Send + Sync {
    fn load_refresh_token(&self) -> Result<Option<Secret>, DriveError>;
    fn save_refresh_token(&self, token: &Secret) -> Result<(), DriveError>;
    /// Removes the token. Removing a token that is not there is not an error.
    fn clear(&self) -> Result<(), DriveError>;
}

fn store_error(e: keyring_core::Error) -> DriveError {
    // keyring errors describe the failed operation, they never include the secret.
    DriveError::CredentialStore(e.to_string())
}

/// Credential store backed by the `keyring` crate.
pub struct KeyringStore {
    entry: keyring_core::Entry,
}

impl KeyringStore {
    /// The entry of this application and this OAuth client in the native credential store.
    /// The user name contains the client id, so builds with different clients do not mix.
    pub fn native(config: &DriveConfig) -> Result<Self, DriveError> {
        let user = format!("refresh-token:{}", config.client_id());
        let entry = keyring::Entry::new(KEYRING_SERVICE, &user).map_err(store_error)?;
        Ok(KeyringStore { entry: entry.inner })
    }

    /// Uses any other `keyring-core` entry; the tests pass one from the in-memory mock store.
    pub fn with_entry(entry: keyring_core::Entry) -> Self {
        KeyringStore { entry }
    }
}

impl TokenStore for KeyringStore {
    fn load_refresh_token(&self) -> Result<Option<Secret>, DriveError> {
        match self.entry.get_password() {
            Ok(p) if p.is_empty() => Ok(None),
            Ok(p) => Ok(Some(Secret::new(p))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(store_error(e)),
        }
    }

    fn save_refresh_token(&self, token: &Secret) -> Result<(), DriveError> {
        self.entry.set_password(token.expose()).map_err(store_error)
    }

    fn clear(&self) -> Result<(), DriveError> {
        match self.entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(store_error(e)),
        }
    }
}

/// Keeps the token in process memory only. For tests and for callers that do not want to
/// remember the sign-in.
#[derive(Default)]
pub struct MemoryStore {
    token: Mutex<Option<Secret>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Secret>> {
        self.token.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl TokenStore for MemoryStore {
    fn load_refresh_token(&self) -> Result<Option<Secret>, DriveError> {
        Ok(self.lock().clone())
    }

    fn save_refresh_token(&self, token: &Secret) -> Result<(), DriveError> {
        *self.lock() = Some(token.clone());
        Ok(())
    }

    fn clear(&self) -> Result<(), DriveError> {
        *self.lock() = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyring_core::api::CredentialStoreApi;

    fn mock_entry(user: &str) -> keyring_core::Entry {
        // The mock store lives in memory. Entries made from it never reach the real store.
        keyring_core::mock::Store::new()
            .unwrap()
            .build("cadviewer-drive-test", user, None)
            .unwrap()
    }

    #[test]
    fn keyring_store_round_trip_with_mock() {
        let store = KeyringStore::with_entry(mock_entry("u1"));
        assert_eq!(store.load_refresh_token().unwrap(), None);
        store.save_refresh_token(&Secret::new("rt-1")).unwrap();
        assert_eq!(store.load_refresh_token().unwrap().unwrap().expose(), "rt-1");
        store.save_refresh_token(&Secret::new("rt-2")).unwrap();
        assert_eq!(store.load_refresh_token().unwrap().unwrap().expose(), "rt-2");
        store.clear().unwrap();
        assert_eq!(store.load_refresh_token().unwrap(), None);
        store.clear().unwrap(); // clearing twice is fine
    }

    #[test]
    fn keyring_store_reports_errors_in_czech_without_secret() {
        // The mock reports an error when asked to.
        let failing = mock_entry("u2");
        let reason: Box<dyn std::error::Error + Send + Sync> = "zamčeno".into();
        failing
            .as_any()
            .downcast_ref::<keyring_core::mock::Cred>()
            .unwrap()
            .set_error(keyring_core::Error::NoStorageAccess(reason));
        let store = KeyringStore::with_entry(failing);
        let err = store.save_refresh_token(&Secret::new("velmi-tajny-token")).unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("Úložiště přihlašovacích údajů"), "{text}");
        assert!(!text.contains("velmi-tajny-token"));
        // The error was one-shot: the next call works again.
        store.save_refresh_token(&Secret::new("ok")).unwrap();
    }

    #[test]
    fn memory_store_round_trip() {
        let store = MemoryStore::new();
        assert_eq!(store.load_refresh_token().unwrap(), None);
        store.save_refresh_token(&Secret::new("x")).unwrap();
        assert_eq!(store.load_refresh_token().unwrap().unwrap().expose(), "x");
        store.clear().unwrap();
        assert_eq!(store.load_refresh_token().unwrap(), None);
    }
}
