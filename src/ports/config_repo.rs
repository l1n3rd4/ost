//! Config repository port.
//!
//! Loads and saves the domain [`Credentials`] aggregate. The contract is
//! expressed in domain terms only: no TOML, filesystem, or other infra type
//! appears in a public signature, and failures are reported as
//! [`DomainError`](crate::domain::DomainError) via [`DomainResult`].
//!
//! [`Credentials`]: crate::domain::Credentials

use crate::domain::{Credentials, DomainResult};

/// Persistence of the credentials aggregate, expressed in domain terms.
///
/// Exactly two operations: load and save.
pub trait ConfigRepositoryPort: Send + Sync {
    /// Load the persisted credentials.
    ///
    /// When no config has been persisted yet (e.g. the backing file is
    /// missing), the adapter returns [`Credentials::default`] rather than an
    /// error. Returning the default empty aggregate on a missing store is the
    /// adapter's responsibility.
    fn load(&self) -> DomainResult<Credentials>;

    /// Persist the given credentials durably.
    ///
    /// On failure the adapter returns [`DomainError::Storage`] and leaves any
    /// previously persisted content unchanged.
    ///
    /// [`DomainError::Storage`]: crate::domain::DomainError::Storage
    fn save(&self, creds: &Credentials) -> DomainResult<()>;
}
