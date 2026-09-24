//! Authentication module for Microsoft Teams
//!
//! Implements OAuth2 device code flow for Azure AD authentication,
//! then exchanges the AAD token for a Skype token.

pub mod acquire;
pub mod client;
pub mod config;
pub mod flow;
pub mod skype;
pub mod tokens;

// Part of the public auth surface (INV); re-exported for external callers even
// though nothing inside the crate imports it via this path today.
#[allow(unused_imports)]
pub use config::AuthConfig;
// `logout` is superseded at the composition root, which now performs logout via
// `ConfigRepositoryPort` (task 8.1); the legacy flow is kept as part of the
// public auth surface for external callers.
#[allow(unused_imports)]
pub use flow::{login, logout, refresh, status};
pub use tokens::{StoredToken, TokenStore};
