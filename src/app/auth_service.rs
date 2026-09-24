//! Authentication use-case service.
//!
//! `AuthService` orchestrates the [`TokenStorePort`] and [`ConfigRepositoryPort`]
//! for the auth-related intents (`login`, `logout`, `status`, `refresh`). It is
//! the hexagonal use-case wrapper around token/config state; it never writes to
//! stdout/stderr and never exits the process (Requirement 5.2). Ports are
//! injected as `Arc<dyn Port>` (Requirement 6.7).
//!
//! The core rule this service encodes is Requirement 12.2: IF the access token
//! is expired ([`StoredToken::is_expired`]) AND a refresh via the token store
//! fails, THEN return [`DomainError::TokenExpired`] with **no further refresh
//! retry**.
//!
//! ## Port mutability
//!
//! [`TokenStorePort`] setters and [`clear_tokens`] take `&mut self`, but
//! Requirement 6.7 injects every port as `Arc<dyn Port>`. To satisfy both, the
//! token store is held as `Arc<Mutex<dyn TokenStorePort>>`: shared ownership via
//! `Arc`, `&mut` access via the `Mutex`. The config repository's operations take
//! `&self`, so it is held as a plain `Arc<dyn ConfigRepositoryPort>`.
//!
//! ## Network seam (future task)
//!
//! The real OAuth device-code acquisition still lives in `auth/flow.rs` (infra)
//! and is not yet expressed as a port. Until a dedicated auth/network port
//! exists, [`AuthService::login`] persists the credentials it is handed and the
//! actual device-code round trip remains in `auth/flow.rs`. [`refresh`] models
//! the refresh via the token store only (read the refresh token; absence means
//! the session cannot be renewed). Both methods are structured so a later task
//! can plug the network refresh in behind a port without changing this service's
//! public shape.
//!
//! [`StoredToken::is_expired`]: crate::domain::StoredToken::is_expired
//! [`DomainError::TokenExpired`]: crate::domain::DomainError::TokenExpired
//! [`clear_tokens`]: crate::ports::TokenStorePort::clear_tokens

use std::sync::{Arc, Mutex};

use crate::domain::{Credentials, DomainError, DomainResult};
use crate::ports::{ConfigRepositoryPort, TokenStorePort};

/// Use-case service for authentication operations.
///
/// Holds a [`TokenStorePort`] (behind a `Mutex` for `&mut` access) and a
/// [`ConfigRepositoryPort`], both injected as `Arc<dyn Port>` (Requirement 6.7).
pub struct AuthService {
    tokens: Arc<Mutex<dyn TokenStorePort>>,
    config: Arc<dyn ConfigRepositoryPort>,
}

impl AuthService {
    /// Construct an `AuthService` from the ports it depends on.
    pub fn new(
        tokens: Arc<Mutex<dyn TokenStorePort>>,
        config: Arc<dyn ConfigRepositoryPort>,
    ) -> Self {
        Self { tokens, config }
    }

    /// Persist the credentials obtained from a login flow.
    ///
    /// The interactive OAuth device-code acquisition currently lives in
    /// `auth/flow.rs` (infra, not yet a port); this method covers the part
    /// expressible via the ports: it persists the supplied [`Credentials`] via
    /// [`ConfigRepositoryPort::save`]. A save failure surfaces as
    /// [`DomainError::Storage`] (mapped by the adapter). A future task can wrap
    /// the network acquisition in a port and call it from here before saving.
    pub fn login(&self, creds: &Credentials) -> DomainResult<()> {
        self.config.save(creds)
    }

    /// Log out: clear all stored token material and reset persisted credentials.
    ///
    /// Clears the token store via [`TokenStorePort::clear_tokens`], then persists
    /// a default (empty) [`Credentials`] aggregate. A persistence failure is
    /// surfaced as a [`DomainError`] by the config adapter (`DomainError::Storage`).
    pub fn logout(&self) -> DomainResult<()> {
        self.with_tokens_mut(|store| store.clear_tokens());
        self.config.save(&Credentials::default())
    }

    /// Report whether the current session is usable.
    ///
    /// Reads the AAD access token from the token store. Returns `Ok(())` when a
    /// non-expired access token is present. When there is no access token the
    /// caller is [`DomainError::Unauthenticated`]. When the access token is
    /// expired, `status` attempts a single refresh (see [`refresh`]); if that
    /// refresh fails the result is [`DomainError::TokenExpired`] with no retry
    /// (Requirement 12.2).
    ///
    /// Output routing (human-readable status) is the responsibility of a primary
    /// adapter; this method performs pure orchestration and never prints.
    ///
    /// [`refresh`]: Self::refresh
    pub fn status(&self) -> DomainResult<()> {
        let access = self.with_tokens(|store| store.get_access_token());
        match access {
            None => Err(DomainError::Unauthenticated(
                "no access token; run login".to_string(),
            )),
            Some(token) if token.is_expired() => self.refresh(),
            Some(_) => Ok(()),
        }
    }

    /// Attempt to renew the session from the stored refresh token.
    ///
    /// This encodes Requirement 12.2. A refresh is attempted **at most once**:
    /// the refresh token is read from the [`TokenStorePort`] exactly one time. If
    /// it is absent, the session cannot be renewed and the result is
    /// [`DomainError::TokenExpired`] — there is no further retry. When a refresh
    /// token is present the renewal is considered to have succeeded at the level
    /// the token store can express.
    ///
    /// The real network refresh (exchanging the refresh token for fresh access
    /// tokens over the wire) lives in `auth/flow.rs` and is not yet a port; this
    /// method is the single, documented seam where that port call will be added.
    /// The "no retry" guarantee is a property of this method: it reads the
    /// refresh token once and branches, never looping.
    pub fn refresh(&self) -> DomainResult<()> {
        // Single read of the refresh token — no loop, no retry (Requirement 12.2).
        let refresh_token = self.with_tokens(|store| store.get_refresh_token());
        match refresh_token {
            // A future task performs the network exchange here behind a port.
            Some(_) => Ok(()),
            None => Err(DomainError::TokenExpired(
                "access token expired and no refresh token available".to_string(),
            )),
        }
    }

    /// Run a closure with shared (`&`) access to the token store.
    fn with_tokens<T>(&self, f: impl FnOnce(&dyn TokenStorePort) -> T) -> T {
        let guard = self.tokens.lock().expect("token store mutex poisoned");
        f(&*guard)
    }

    /// Run a closure with exclusive (`&mut`) access to the token store.
    fn with_tokens_mut<T>(&self, f: impl FnOnce(&mut dyn TokenStorePort) -> T) -> T {
        let mut guard = self.tokens.lock().expect("token store mutex poisoned");
        f(&mut *guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::domain::StoredToken;

    /// Fake token store recording how many times the refresh token was read, so
    /// tests can assert the "no retry" guarantee (Requirement 12.2).
    #[derive(Default)]
    struct FakeTokenStore {
        access_token: Option<StoredToken>,
        refresh_token: Option<String>,
        cleared: bool,
        refresh_reads: AtomicUsize,
    }

    impl TokenStorePort for FakeTokenStore {
        fn get_access_token(&self) -> Option<StoredToken> {
            self.access_token.clone()
        }
        fn set_access_token(&mut self, token: String, expires_in: Option<u64>) {
            self.access_token = Some(StoredToken::new(token, expires_in));
        }
        fn get_refresh_token(&self) -> Option<String> {
            self.refresh_reads.fetch_add(1, Ordering::SeqCst);
            self.refresh_token.clone()
        }
        fn set_refresh_token(&mut self, token: String) {
            self.refresh_token = Some(token);
        }
        fn get_skype_token(&self) -> Option<StoredToken> {
            None
        }
        fn set_skype_token(&mut self, _token: String, _expires_in: Option<u64>) {}
        fn get_graph_token(&self) -> Option<StoredToken> {
            None
        }
        fn set_graph_token(&mut self, _token: String, _expires_in: Option<u64>) {}
        fn get_ic3_token(&self) -> Option<StoredToken> {
            None
        }
        fn set_ic3_token(&mut self, _token: String, _expires_in: Option<u64>) {}
        fn get_recorder_token(&self) -> Option<StoredToken> {
            None
        }
        fn set_recorder_token(&mut self, _token: String, _expires_in: Option<u64>) {}
        fn clear_tokens(&mut self) {
            self.cleared = true;
            self.access_token = None;
            self.refresh_token = None;
        }
    }

    /// Fake config repository recording the last saved credentials and able to
    /// simulate a persistence failure.
    #[derive(Default)]
    struct FakeConfigRepo {
        saved: Mutex<Option<Credentials>>,
        fail_save: bool,
        save_calls: AtomicUsize,
    }

    impl ConfigRepositoryPort for FakeConfigRepo {
        fn load(&self) -> DomainResult<Credentials> {
            Ok(Credentials::default())
        }
        fn save(&self, creds: &Credentials) -> DomainResult<()> {
            self.save_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_save {
                return Err(DomainError::Storage("simulated persistence failure".to_string()));
            }
            *self.saved.lock().unwrap() = Some(creds.clone());
            Ok(())
        }
    }

    fn expired_token() -> StoredToken {
        // Already past expiry, so `is_expired()` is true.
        StoredToken {
            token: "old".to_string(),
            expires_at: Some(0),
        }
    }

    #[test]
    fn refresh_with_no_refresh_token_returns_token_expired_and_reads_once() {
        let store = Arc::new(Mutex::new(FakeTokenStore {
            refresh_token: None,
            ..Default::default()
        }));
        let config = Arc::new(FakeConfigRepo::default());
        let svc = AuthService::new(store.clone(), config);

        let result = svc.refresh();

        assert!(
            matches!(result, Err(DomainError::TokenExpired(_))),
            "expected TokenExpired, got {result:?}"
        );
        // No retry: the refresh token was read at most once (Requirement 12.2).
        let reads = store.lock().unwrap().refresh_reads.load(Ordering::SeqCst);
        assert_eq!(reads, 1, "refresh must be attempted at most once, got {reads}");
    }

    #[test]
    fn status_with_expired_token_and_no_refresh_returns_token_expired_no_retry() {
        let store = Arc::new(Mutex::new(FakeTokenStore {
            access_token: Some(expired_token()),
            refresh_token: None,
            ..Default::default()
        }));
        let config = Arc::new(FakeConfigRepo::default());
        let svc = AuthService::new(store.clone(), config);

        let result = svc.status();

        assert!(
            matches!(result, Err(DomainError::TokenExpired(_))),
            "expected TokenExpired, got {result:?}"
        );
        // Exactly one refresh attempt was made — no retry loop.
        let reads = store.lock().unwrap().refresh_reads.load(Ordering::SeqCst);
        assert_eq!(reads, 1, "refresh must be attempted at most once, got {reads}");
    }

    #[test]
    fn status_with_no_access_token_is_unauthenticated() {
        let store = Arc::new(Mutex::new(FakeTokenStore::default()));
        let config = Arc::new(FakeConfigRepo::default());
        let svc = AuthService::new(store, config);

        let result = svc.status();

        assert!(
            matches!(result, Err(DomainError::Unauthenticated(_))),
            "expected Unauthenticated, got {result:?}"
        );
    }

    #[test]
    fn status_with_valid_token_is_ok() {
        let store = Arc::new(Mutex::new(FakeTokenStore {
            access_token: Some(StoredToken::new("fresh".to_string(), Some(3600))),
            ..Default::default()
        }));
        let config = Arc::new(FakeConfigRepo::default());
        let svc = AuthService::new(store, config);

        assert!(svc.status().is_ok());
    }

    #[test]
    fn refresh_with_present_refresh_token_is_ok() {
        let store = Arc::new(Mutex::new(FakeTokenStore {
            refresh_token: Some("refresh-abc".to_string()),
            ..Default::default()
        }));
        let config = Arc::new(FakeConfigRepo::default());
        let svc = AuthService::new(store, config);

        assert!(svc.refresh().is_ok());
    }

    #[test]
    fn logout_clears_tokens_and_saves_default_credentials() {
        let store = Arc::new(Mutex::new(FakeTokenStore {
            access_token: Some(StoredToken::new("a".to_string(), Some(3600))),
            refresh_token: Some("r".to_string()),
            ..Default::default()
        }));
        let config = Arc::new(FakeConfigRepo::default());
        let svc = AuthService::new(store.clone(), config.clone());

        let result = svc.logout();

        assert!(result.is_ok());
        assert!(store.lock().unwrap().cleared, "tokens should be cleared");
        // A default (empty) credentials aggregate was persisted.
        let saved = config.saved.lock().unwrap();
        let saved = saved.as_ref().expect("credentials should have been saved");
        assert!(saved.access_token.is_none());
        assert!(saved.refresh_token.is_none());
    }

    #[test]
    fn logout_maps_save_failure_to_storage_error() {
        let store = Arc::new(Mutex::new(FakeTokenStore::default()));
        let config = Arc::new(FakeConfigRepo {
            fail_save: true,
            ..Default::default()
        });
        let svc = AuthService::new(store, config);

        let result = svc.logout();

        assert!(
            matches!(result, Err(DomainError::Storage(_))),
            "expected Storage error, got {result:?}"
        );
    }

    #[test]
    fn login_persists_credentials() {
        let store = Arc::new(Mutex::new(FakeTokenStore::default()));
        let config = Arc::new(FakeConfigRepo::default());
        let svc = AuthService::new(store, config.clone());

        let mut creds = Credentials::default();
        creds.refresh_token = Some("saved-refresh".to_string());

        assert!(svc.login(&creds).is_ok());
        let saved = config.saved.lock().unwrap();
        let saved = saved.as_ref().expect("credentials should have been saved");
        assert_eq!(saved.refresh_token.as_deref(), Some("saved-refresh"));
    }

    #[test]
    fn login_maps_save_failure_to_storage_error() {
        let store = Arc::new(Mutex::new(FakeTokenStore::default()));
        let config = Arc::new(FakeConfigRepo {
            fail_save: true,
            ..Default::default()
        });
        let svc = AuthService::new(store, config);

        let result = svc.login(&Credentials::default());
        assert!(
            matches!(result, Err(DomainError::Storage(_))),
            "expected Storage error, got {result:?}"
        );
    }
}
