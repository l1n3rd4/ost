//! API client module for Microsoft Teams

mod chat;
pub mod client;
mod me;
mod presence;
mod teams;

// Data-returning API functions. Each parses the raw Teams/Graph response
// straight into domain models; the HTTP adapter forwards them unchanged.
pub use chat::{list_chats_data, read_messages_data, send_message_with_client};
pub use me::whoami_data;
pub use presence::get_presence_data;
pub use teams::list_teams_data;
