//! API client module for Microsoft Teams

mod chat;
pub mod client;
mod me;
mod presence;
mod teams;

// Re-export data types for TUI integration
pub use chat::{ChatInfo, MessageInfo};
pub use me::UserInfo;
pub use presence::PresenceInfo;
pub use teams::TeamInfo;

// Re-export data-returning functions for TUI integration
pub use chat::{list_chats_data, read_messages_data, send_message_with_client};
pub use me::whoami_data;
pub use presence::get_presence_data;
pub use teams::list_teams_data;
