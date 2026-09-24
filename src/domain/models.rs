//! I/O-free domain models.
//!
//! Pure data types for the domain core. This module imports only from the
//! allowed domain dependency set ({serde, thiserror, chrono, uuid}) and the
//! sibling `error` module; it must not reference any infrastructure crate
//! (`reqwest`, `toml`, `tokio-tungstenite`, `clap`, `ratatui`, `std::fs`).

use chrono::{DateTime, Utc};

use crate::domain::error::{DomainError, DomainResult};

/// A chat conversation.
#[derive(Debug, Clone)]
pub struct Chat {
    pub id: ChatId,
    pub name: String,
    pub is_group: bool,
    pub last_message: Option<MessagePreview>,
}

/// Identifier for a [`Chat`]. Never empty; use [`ChatId::new`] to construct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChatId(pub String);

impl ChatId {
    /// Construct a validated `ChatId`.
    ///
    /// Rejects an empty (or whitespace-only) identifier with
    /// [`DomainError::Invalid`] before any I/O occurs.
    pub fn new(id: impl Into<String>) -> DomainResult<Self> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(DomainError::Invalid("chat id must not be empty".to_string()));
        }
        Ok(ChatId(id))
    }

    /// Borrow the underlying identifier string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Short preview of the most recent message in a chat.
#[derive(Debug, Clone)]
pub struct MessagePreview {
    pub sender: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub text: String,
}

/// A message within a chat.
#[derive(Debug, Clone)]
pub struct Message {
    pub sender: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub content: String,
}

/// A user/identity.
#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub display_name: String,
    pub email: Option<String>,
}

/// Presence/availability state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    Available,
    Busy,
    Away,
    DoNotDisturb,
    Offline,
    Custom(String),
}

/// Result of successfully sending a message.
#[derive(Debug, Clone)]
pub struct SentMessage {
    pub id: String,
    pub chat_id: ChatId,
    pub timestamp: Option<DateTime<Utc>>,
}

/// A team the user belongs to.
#[derive(Debug, Clone)]
pub struct Team {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
}

/// An event delivered over the realtime channel.
#[derive(Debug, Clone)]
pub enum RealtimeEvent {
    MessageReceived { chat_id: ChatId, message: Message },
    PresenceChanged { user_id: String, presence: Presence },
    CallInfo(String),
    Unknown(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_id_rejects_empty_string() {
        let result = ChatId::new("");
        assert!(matches!(result, Err(DomainError::Invalid(_))));
    }

    #[test]
    fn chat_id_rejects_whitespace_only() {
        let result = ChatId::new("   \t\n");
        assert!(matches!(result, Err(DomainError::Invalid(_))));
    }

    #[test]
    fn chat_id_accepts_non_empty() {
        let id = ChatId::new("19:abc@thread.v2").expect("non-empty id should be accepted");
        assert_eq!(id.as_str(), "19:abc@thread.v2");
    }

    #[test]
    fn chat_id_preserves_surrounding_whitespace_when_non_empty() {
        // Trimming is used only for the empty check; the stored value is kept as-is.
        let id = ChatId::new(" x ").expect("non-empty id should be accepted");
        assert_eq!(id.as_str(), " x ");
    }
}
