//! Domain error type.
//!
//! `DomainError` is the domain's failure type. It exposes no infrastructure
//! types (no `reqwest`, `toml`, `tokio-tungstenite`, etc.), so business logic
//! can handle failures without knowing about the underlying infra.

/// Domain-level failure, independent of any infrastructure concern.
///
/// Each variant carries a descriptive message explaining the failure.
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    /// The caller is not authenticated (e.g. missing/invalid credentials, HTTP 401).
    #[error("unauthenticated: {0}")]
    Unauthenticated(String),

    /// A token has expired and could not be refreshed.
    #[error("token expired: {0}")]
    TokenExpired(String),

    /// The requested resource does not exist (e.g. HTTP 404).
    #[error("not found: {0}")]
    NotFound(String),

    /// The input failed domain validation (e.g. empty message, empty chat id).
    #[error("invalid input: {0}")]
    Invalid(String),

    /// A transport-level failure occurred (e.g. timeout, DNS failure).
    #[error("transport failure: {0}")]
    Transport(String),

    /// A storage read/write failure occurred (e.g. token/config persistence).
    #[error("storage failure: {0}")]
    Storage(String),
}

/// Convenience alias for domain results.
pub type DomainResult<T> = Result<T, DomainError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthenticated_display_message() {
        let err = DomainError::Unauthenticated("no token".to_string());
        assert_eq!(err.to_string(), "unauthenticated: no token");
    }

    #[test]
    fn token_expired_display_message() {
        let err = DomainError::TokenExpired("refresh failed".to_string());
        assert_eq!(err.to_string(), "token expired: refresh failed");
    }

    #[test]
    fn not_found_display_message() {
        let err = DomainError::NotFound("chat 42".to_string());
        assert_eq!(err.to_string(), "not found: chat 42");
    }

    #[test]
    fn invalid_display_message() {
        let err = DomainError::Invalid("empty message".to_string());
        assert_eq!(err.to_string(), "invalid input: empty message");
    }

    #[test]
    fn transport_display_message() {
        let err = DomainError::Transport("timeout".to_string());
        assert_eq!(err.to_string(), "transport failure: timeout");
    }

    #[test]
    fn storage_display_message() {
        let err = DomainError::Storage("disk full".to_string());
        assert_eq!(err.to_string(), "storage failure: disk full");
    }
}
