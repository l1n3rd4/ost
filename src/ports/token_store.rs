//! Token store port.
//!
//! Generalizes the original `TokenStore` trait (`auth/tokens.rs`) into a
//! domain-typed port covering all six token types the system manages: the AAD
//! access token, its refresh token, and the Skype, Graph, IC3, and recorder
//! tokens. This port is synchronous (token storage is in-memory / fast local
//! state, not an async I/O boundary) and, like every port, exposes only domain
//! types: access-style tokens are [`StoredToken`] and the refresh token is a
//! bare `String`.
//!
//! Per Requirement 4.4 the port evolves the existing `TokenStore` (which
//! covered only access + refresh) to cover all six token types.
//!
//! [`StoredToken`]: crate::domain::StoredToken

use crate::domain::StoredToken;

/// Storage for the six token types an authenticated identity holds.
///
/// `get_*` return the currently stored token (if any); `set_*` store a token,
/// converting a relative `expires_in` lifetime into an absolute expiry via
/// [`StoredToken::new`]. The refresh token has no expiry, so it is stored and
/// returned as a plain `String`. [`clear_tokens`](Self::clear_tokens) drops all
/// stored token material.
pub trait TokenStorePort: Send + Sync {
    /// Get the stored AAD access token, if any.
    fn get_access_token(&self) -> Option<StoredToken>;

    /// Store the AAD access token with an optional relative lifetime (seconds).
    fn set_access_token(&mut self, token: String, expires_in: Option<u64>);

    /// Get the stored refresh token, if any.
    fn get_refresh_token(&self) -> Option<String>;

    /// Store the refresh token.
    fn set_refresh_token(&mut self, token: String);

    /// Get the stored Skype token, if any.
    fn get_skype_token(&self) -> Option<StoredToken>;

    /// Store the Skype token with an optional relative lifetime (seconds).
    fn set_skype_token(&mut self, token: String, expires_in: Option<u64>);

    /// Get the stored Graph token, if any.
    fn get_graph_token(&self) -> Option<StoredToken>;

    /// Store the Graph token with an optional relative lifetime (seconds).
    fn set_graph_token(&mut self, token: String, expires_in: Option<u64>);

    /// Get the stored IC3 token, if any.
    fn get_ic3_token(&self) -> Option<StoredToken>;

    /// Store the IC3 token with an optional relative lifetime (seconds).
    fn set_ic3_token(&mut self, token: String, expires_in: Option<u64>);

    /// Get the stored recorder token, if any.
    fn get_recorder_token(&self) -> Option<StoredToken>;

    /// Store the recorder token with an optional relative lifetime (seconds).
    fn set_recorder_token(&mut self, token: String, expires_in: Option<u64>);

    /// Drop all stored token material.
    fn clear_tokens(&mut self);
}
