use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "teams-cli")]
#[command(about = "Lightweight CLI client for Microsoft Teams", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    pub(crate) verbose: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Authenticate with Microsoft Teams
    Login {
        /// Force interactive login even if cached token exists
        #[arg(short, long)]
        force: bool,
    },

    /// Log out and clear cached credentials
    Logout,

    /// Show current authentication status
    Status,

    /// List recent chats
    Chats {
        /// Maximum number of chats to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Read messages from a chat
    Read {
        /// Chat thread ID (from `chats` output)
        chat_id: String,

        /// Maximum number of messages to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Send a message
    Send {
        /// Chat thread ID (from `chats` output)
        #[arg(short, long)]
        to: String,

        /// Message content
        message: String,
    },

    /// List joined teams and their channels
    Teams,

    /// Show current user info (verify auth works)
    Whoami,

    /// Connect to Trouter WebSocket push service
    Trouter,

    /// Get/set presence status
    Presence {
        /// New status: available, busy, dnd, away, offline
        #[arg(short, long)]
        set: Option<String>,
    },

    /// Launch the terminal user interface
    Tui,
}