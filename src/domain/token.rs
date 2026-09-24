//! Pure token rules and the credentials aggregate.
//!
//! These are I/O-free domain rules: `StoredToken` computes expiry against the
//! system clock only at construction (the input `expires_in_secs`), and
//! `is_expired` compares stored expiry to the current time. No net/fs access.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// A stored access token with an optional absolute expiry (Unix seconds).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToken {
    pub token: String,
    pub expires_at: Option<u64>,
}

impl StoredToken {
    /// Build a token, converting a relative lifetime into an absolute expiry.
    pub fn new(token: String, expires_in_secs: Option<u64>) -> Self {
        let expires_at = expires_in_secs.map(|secs| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + secs
        });

        Self { token, expires_at }
    }

    /// Whether the token is expired, treating it as expired 5 minutes before
    /// the real expiry to leave a refresh margin. Tokens with no expiry never
    /// expire.
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.is_expired_at(now)
    }

    /// Pure expiry rule evaluated against a supplied reference time (Unix
    /// seconds). A token is expired when less than 5 minutes remain before its
    /// absolute expiry; tokens with no expiry never expire. This is the I/O-free
    /// core of [`is_expired`], which supplies the current time.
    pub fn is_expired_at(&self, now: u64) -> bool {
        match self.expires_at {
            // Consider expired if less than 5 minutes remaining.
            Some(exp) => now + 300 >= exp,
            None => false,
        }
    }
}

/// Credentials aggregate: all token material and regional metadata for a
/// logged-in identity.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Credentials {
    pub access_token: Option<StoredToken>,
    pub refresh_token: Option<String>,
    pub skype_token: Option<StoredToken>,
    pub graph_token: Option<StoredToken>,
    pub ic3_token: Option<StoredToken>,
    pub recorder_token: Option<StoredToken>,
    pub tenant_id: Option<String>,
    pub region_gtms: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn none_expiry_never_expires() {
        let token = StoredToken {
            token: "t".to_string(),
            expires_at: None,
        };
        assert!(!token.is_expired());
    }

    #[test]
    fn far_future_expiry_is_not_expired() {
        // One hour out is well beyond the 5-minute refresh margin.
        let token = StoredToken {
            token: "t".to_string(),
            expires_at: Some(now_unix() + 3600),
        };
        assert!(!token.is_expired());
    }

    #[test]
    fn expiry_within_refresh_margin_is_expired() {
        // Expires in ~60s, inside the 300s margin, so treated as expired.
        let token = StoredToken {
            token: "t".to_string(),
            expires_at: Some(now_unix() + 60),
        };
        assert!(token.is_expired());
    }

    #[test]
    fn already_past_expiry_is_expired() {
        let token = StoredToken {
            token: "t".to_string(),
            expires_at: Some(now_unix().saturating_sub(10)),
        };
        assert!(token.is_expired());
    }

    #[test]
    fn just_outside_refresh_margin_is_not_expired() {
        // Just beyond the 300s margin should not be considered expired.
        let token = StoredToken {
            token: "t".to_string(),
            expires_at: Some(now_unix() + 300 + 30),
        };
        assert!(!token.is_expired());
    }

    // Property 6: Testability — pure `is_expired` rule verified via proptest.
    // Validates: Requirements 9.4, 9.5, 1.4.
    mod properties {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(256))]

            /// `is_expired_at` is monotonic in the reference time: once a token
            /// is expired at some instant, it stays expired at every later
            /// instant. A token can go from live to expired as time advances,
            /// never the reverse.
            #[test]
            fn is_expired_monotonic_in_reference_time(
                expires_at in proptest::option::of(any::<u64>()),
                t1 in any::<u64>(),
                t2 in any::<u64>(),
            ) {
                let token = StoredToken { token: "t".to_string(), expires_at };
                let (earlier, later) = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
                if token.is_expired_at(earlier) {
                    prop_assert!(
                        token.is_expired_at(later),
                        "expiry regressed: expired at {earlier} but not at {later} (expires_at={expires_at:?})"
                    );
                }
            }

            /// A token with no absolute expiry is never expired, at any time.
            #[test]
            fn none_expiry_never_expires_at_any_time(now in any::<u64>()) {
                let token = StoredToken { token: "t".to_string(), expires_at: None };
                prop_assert!(!token.is_expired_at(now));
            }
        }
    }
}
