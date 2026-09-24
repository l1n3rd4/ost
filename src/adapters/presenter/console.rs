//! Console presenter adapter.
//!
//! [`ConsolePresenter`] implements [`PresenterPort`] by writing domain models to
//! stdout (and errors to stderr). Its output preserves the textual shape of the
//! original `api/` `println!`s (Requirement 10, behavior preservation): the
//! headers, separators, indentation, and per-item lines match the pre-refactor
//! CLI so recorded-fixture equivalence tests (task 8.3) keep passing.
//!
//! This is a presenter *adapter*, so `println!`/`eprintln!` are used directly
//! here; the app and domain layers never write to stdout themselves.

use chrono::{DateTime, Utc};

use crate::domain::{
    Chat, DomainError, Message, Presence, RealtimeEvent, SentMessage, Team, User,
};
use crate::ports::PresenterPort;

/// A [`PresenterPort`] that renders domain models to the terminal.
///
/// Unit struct with no state: every method formats its argument and writes to
/// stdout (or stderr for [`show_error`](PresenterPort::show_error)).
#[derive(Debug, Default, Clone, Copy)]
pub struct ConsolePresenter;

impl ConsolePresenter {
    /// Construct a `ConsolePresenter`.
    pub fn new() -> Self {
        ConsolePresenter
    }
}

/// Render an optional timestamp the way the original `api/` code did.
///
/// The pre-refactor structs carried the arrival/compose time as an owned
/// string; the domain model now carries `Option<DateTime<Utc>>`. When present
/// we render RFC 3339 (the same shape Graph/Teams return); when absent we use an
/// empty string, mirroring the old `unwrap_or("")` behavior for messages.
fn render_timestamp(ts: &Option<DateTime<Utc>>) -> String {
    match ts {
        Some(dt) => dt.to_rfc3339(),
        None => String::new(),
    }
}

/// Human-readable availability label for a [`Presence`] value.
///
/// The original presence output printed the raw Graph `availability` string
/// (e.g. `Available`, `Busy`, `DoNotDisturb`). The domain enum encodes the same
/// states, so we map each variant back to that label; `Custom(s)` renders the
/// carried string verbatim.
fn presence_label(presence: &Presence) -> String {
    match presence {
        Presence::Available => "Available".to_string(),
        Presence::Busy => "Busy".to_string(),
        Presence::Away => "Away".to_string(),
        Presence::DoNotDisturb => "DoNotDisturb".to_string(),
        Presence::Offline => "Offline".to_string(),
        Presence::Custom(s) => s.clone(),
    }
}

impl PresenterPort for ConsolePresenter {
    fn show_chats(&self, chats: &[Chat]) {
        println!("\nRecent Chats:");
        println!("{:-<60}", "");

        if chats.is_empty() {
            println!("  (no chats found)");
            return;
        }

        for chat in chats {
            println!("{}", chat.name);
            println!("  ID: {}", chat.id.as_str());

            if let Some(ref preview) = chat.last_message {
                let time = render_timestamp(&preview.timestamp);
                if !time.is_empty() {
                    println!("  Last: {}", time);
                }
                if !preview.text.trim().is_empty() {
                    let sender = if preview.sender.is_empty() {
                        "?"
                    } else {
                        preview.sender.as_str()
                    };
                    println!("  [{}]: {}", sender, preview.text.trim());
                }
            }

            println!();
        }
    }

    fn show_messages(&self, messages: &[Message]) {
        if messages.is_empty() {
            println!("(no messages)");
            return;
        }

        for msg in messages {
            let time = render_timestamp(&msg.timestamp);
            println!("[{}] {}: {}", time, msg.sender, msg.content);
        }
    }

    fn message_sent(&self, _sent: &SentMessage) {
        println!("Message sent.");
    }

    fn show_presence(&self, presence: &Presence) {
        let label = presence_label(presence);
        println!("\nPresence Status:");
        println!("  Availability: {}", label);
        println!("  Activity: {}", label);
    }

    fn show_user(&self, user: &User) {
        println!();
        println!("Display Name: {}", user.display_name);
        println!("Mail:         {}", user.email.as_deref().unwrap_or("(none)"));
        println!("ID:           {}", user.id);
    }

    fn show_teams(&self, teams: &[Team]) {
        println!("\nTeams and Channels:");
        println!("{:-<60}", "");

        if teams.is_empty() {
            println!("  (no teams found)");
            return;
        }

        for team in teams {
            // The domain `Team` does not carry channels, so the channel count is
            // always 0 here; the "(N channels)" shape is preserved for parity
            // with the original teams listing.
            println!("Team: {} (0 channels)", team.display_name);
            if let Some(ref desc) = team.description {
                if !desc.trim().is_empty() {
                    println!("  {}", desc.trim());
                }
            }
            println!();
        }
    }

    fn notify_event(&self, event: &RealtimeEvent) {
        match event {
            RealtimeEvent::MessageReceived { chat_id, message } => {
                let time = render_timestamp(&message.timestamp);
                println!(
                    "[{}] {} in {}: {}",
                    time,
                    message.sender,
                    chat_id.as_str(),
                    message.content
                );
            }
            RealtimeEvent::PresenceChanged { user_id, presence } => {
                println!("Presence: {} is now {}", user_id, presence_label(presence));
            }
            RealtimeEvent::CallInfo(info) => {
                println!("Call: {}", info);
            }
            RealtimeEvent::Unknown(raw) => {
                println!("Event: {}", raw);
            }
        }
    }

    fn show_error(&self, error: &DomainError) {
        eprintln!("Error: {}", error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ChatId, MessagePreview};

    #[test]
    fn presence_label_maps_each_variant() {
        assert_eq!(presence_label(&Presence::Available), "Available");
        assert_eq!(presence_label(&Presence::Busy), "Busy");
        assert_eq!(presence_label(&Presence::Away), "Away");
        assert_eq!(presence_label(&Presence::DoNotDisturb), "DoNotDisturb");
        assert_eq!(presence_label(&Presence::Offline), "Offline");
        assert_eq!(
            presence_label(&Presence::Custom("BeRightBack".to_string())),
            "BeRightBack"
        );
    }

    #[test]
    fn render_timestamp_empty_when_none() {
        assert_eq!(render_timestamp(&None), "");
    }

    #[test]
    fn render_timestamp_rfc3339_when_some() {
        let dt = DateTime::parse_from_rfc3339("2024-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(render_timestamp(&Some(dt)), dt.to_rfc3339());
    }

    #[test]
    fn presenter_is_constructible_and_renders_without_panic() {
        let p = ConsolePresenter::new();
        // Exercises each method to ensure formatting compiles and runs.
        p.show_chats(&[]);
        p.show_chats(&[Chat {
            id: ChatId::new("19:abc@thread.v2").unwrap(),
            name: "Team Chat".to_string(),
            is_group: true,
            last_message: Some(MessagePreview {
                sender: "Alice".to_string(),
                timestamp: None,
                text: "hello".to_string(),
            }),
        }]);
        p.show_messages(&[]);
        p.show_messages(&[Message {
            sender: "Bob".to_string(),
            timestamp: None,
            content: "hi".to_string(),
        }]);
        p.message_sent(&SentMessage {
            id: "1".to_string(),
            chat_id: ChatId::new("19:abc@thread.v2").unwrap(),
            timestamp: None,
        });
        p.show_presence(&Presence::Available);
        p.show_user(&User {
            id: "id".to_string(),
            display_name: "Name".to_string(),
            email: None,
        });
        p.show_teams(&[]);
        p.notify_event(&RealtimeEvent::Unknown("x".to_string()));
        p.show_error(&DomainError::Invalid("bad".to_string()));
    }
}
