//! Domain core (ports & adapters).
//!
//! Pure models and rules with no I/O. This layer must not import any
//! infrastructure crate (`reqwest`, `toml`, `tokio-tungstenite`, `clap`,
//! `ratatui`, `std::fs`); it may depend only on {serde, thiserror, chrono, uuid}.

mod error;
mod models;
mod rules;
mod token;

pub use error::{DomainError, DomainResult};
pub use models::{
    Chat, ChatId, Message, MessagePreview, Presence, RealtimeEvent, SentMessage, Team, User,
};
pub use rules::{html_escape, strip_html, SendMessageCommand, MAX_MESSAGE_CHARS};
pub use token::{Credentials, StoredToken};
