//! Teams and Graph API ports.
//!
//! These ports state the app's needs in domain terms. Every signature uses
//! only domain types and returns [`DomainResult`]; no infrastructure type
//! (`reqwest::Response`, `serde_json::Value`, `toml::Value`, ...) ever appears
//! in a public signature. Adapters convert infra failures to [`DomainError`]
//! before returning.
//!
//! [`DomainError`]: crate::domain::DomainError

use async_trait::async_trait;

use crate::domain::{Chat, ChatId, DomainResult, Message, Presence, SentMessage, Team, User};

/// Teams API operations, expressed in domain terms.
///
/// Exactly five operations: list chats, read messages, send a message, and
/// get/set presence. Each returns a domain model (or `()`), never a partial
/// model, and reports failure as [`DomainError`](crate::domain::DomainError).
#[async_trait]
pub trait TeamsApiPort: Send + Sync {
    /// List up to `limit` chats.
    async fn list_chats(&self, limit: usize) -> DomainResult<Vec<Chat>>;

    /// Read up to `limit` messages from the given chat.
    async fn read_messages(&self, chat_id: &ChatId, limit: usize) -> DomainResult<Vec<Message>>;

    /// Send `text` to the given chat, returning the sent message.
    async fn send_message(&self, chat_id: &ChatId, text: &str) -> DomainResult<SentMessage>;

    /// Get the current user's presence.
    async fn get_presence(&self) -> DomainResult<Presence>;

    /// Set the current user's presence.
    async fn set_presence(&self, presence: &Presence) -> DomainResult<()>;
}

/// Microsoft Graph operations, expressed in domain terms.
///
/// Exactly two operations: identify the current user and list the user's teams.
#[async_trait]
pub trait GraphPort: Send + Sync {
    /// Return the current authenticated user.
    async fn whoami(&self) -> DomainResult<User>;

    /// List the teams the current user belongs to.
    async fn list_teams(&self) -> DomainResult<Vec<Team>>;
}
