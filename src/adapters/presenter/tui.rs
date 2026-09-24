//! TUI presenter adapter.
//!
//! [`TuiPresenter`] implements [`PresenterPort`] by feeding domain models into
//! TUI state instead of printing them (Requirement 7.10, 5.3). It is the TUI
//! counterpart to [`ConsolePresenter`](super::console::ConsolePresenter): the
//! same use-case services can drive either surface by swapping the injected
//! presenter at the composition root.
//!
//! Unlike the console presenter, the TUI runs an async event loop that owns its
//! mutable state, so a presenter cannot mutate that state directly. Instead this
//! adapter holds the sending half of an `mpsc` channel and converts each
//! `PresenterPort` call into a [`TuiUpdate`] message. The TUI event loop drains
//! the receiving half and applies each update to its `App` state (that wiring is
//! task 11.2; this task defines the adapter, its port impl, and unit tests).
//!
//! Sending over an [`UnboundedSender`](tokio::sync::mpsc::UnboundedSender) is
//! non-blocking and `Send + Sync`, satisfying the `PresenterPort: Send + Sync`
//! bound so the presenter can be shared across the service/task boundary as an
//! `Arc<dyn PresenterPort>`.

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::domain::{
    Chat, DomainError, Message, Presence, RealtimeEvent, SentMessage, Team, User,
};
use crate::ports::PresenterPort;

/// A presentation update produced by [`TuiPresenter`], to be applied to TUI state.
///
/// Each variant mirrors one [`PresenterPort`] method and carries the owned
/// domain model(s) that method received. The TUI event loop receives these over
/// the presenter's channel and folds them into its `App` state (task 11.2),
/// keeping the presenter free of any `ratatui`/`App` dependency and preserving
/// the domain-typed boundary the ports layer requires.
#[derive(Debug, Clone)]
pub enum TuiUpdate {
    /// Replace the list of chats shown in the sidebar.
    Chats(Vec<Chat>),
    /// Replace the messages shown in the messages pane.
    Messages(Vec<Message>),
    /// A message was sent successfully.
    MessageSent(SentMessage),
    /// Update the presence/availability indicator.
    Presence(Presence),
    /// Update the current user/identity.
    User(User),
    /// Replace the list of teams shown in the sidebar.
    Teams(Vec<Team>),
    /// A realtime event arrived.
    Event(RealtimeEvent),
    /// A use-case service reported a failure.
    ///
    /// Carries the error's rendered message rather than the [`DomainError`]
    /// itself: `DomainError` is not `Clone`, and the TUI only needs the text to
    /// show in its status bar. The message is produced via `Display`, matching
    /// what [`ConsolePresenter`](super::console::ConsolePresenter) writes to
    /// stderr.
    Error(String),
}

/// A [`PresenterPort`] that feeds domain models into TUI state over a channel.
///
/// Holds the sending half of an [`UnboundedSender<TuiUpdate>`]. Every port
/// method wraps its argument (cloned into an owned domain model) in the matching
/// [`TuiUpdate`] variant and sends it; the TUI event loop owns the receiver and
/// applies the updates to its state.
///
/// The send is non-blocking and never panics: if the receiver has been dropped
/// (the TUI is shutting down) the update is silently discarded, mirroring how
/// [`Backend::send`](crate::tui::backend::Backend) tolerates a closed channel.
#[derive(Debug, Clone)]
pub struct TuiPresenter {
    tx: UnboundedSender<TuiUpdate>,
}

impl TuiPresenter {
    /// Construct a `TuiPresenter` that sends updates over `tx`.
    ///
    /// The paired [`UnboundedReceiver`] is owned by the TUI event loop, which
    /// drains it and applies each [`TuiUpdate`] to its state.
    pub fn new(tx: UnboundedSender<TuiUpdate>) -> Self {
        Self { tx }
    }

    /// Construct a `TuiPresenter` together with its paired receiver.
    ///
    /// Convenience for the composition root (task 11.2): returns the presenter
    /// to inject into the use-case services and the receiver to hand to the TUI
    /// event loop.
    pub fn channel() -> (Self, UnboundedReceiver<TuiUpdate>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (Self::new(tx), rx)
    }

    /// Send an update, silently discarding it if the receiver has been dropped.
    fn send(&self, update: TuiUpdate) {
        // A closed channel means the TUI has shut down; dropping the update is
        // the correct, non-panicking behavior here.
        let _ = self.tx.send(update);
    }
}

impl PresenterPort for TuiPresenter {
    fn show_chats(&self, chats: &[Chat]) {
        self.send(TuiUpdate::Chats(chats.to_vec()));
    }

    fn show_messages(&self, messages: &[Message]) {
        self.send(TuiUpdate::Messages(messages.to_vec()));
    }

    fn message_sent(&self, sent: &SentMessage) {
        self.send(TuiUpdate::MessageSent(sent.clone()));
    }

    fn show_presence(&self, presence: &Presence) {
        self.send(TuiUpdate::Presence(presence.clone()));
    }

    fn show_user(&self, user: &User) {
        self.send(TuiUpdate::User(user.clone()));
    }

    fn show_teams(&self, teams: &[Team]) {
        self.send(TuiUpdate::Teams(teams.to_vec()));
    }

    fn notify_event(&self, event: &RealtimeEvent) {
        self.send(TuiUpdate::Event(event.clone()));
    }

    fn show_error(&self, error: &DomainError) {
        self.send(TuiUpdate::Error(error.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ChatId, MessagePreview};
    use tokio::sync::mpsc::error::TryRecvError;

    /// Build a chat fixture with a non-empty validated id.
    fn sample_chat() -> Chat {
        Chat {
            id: ChatId::new("19:abc@thread.v2").unwrap(),
            name: "Team Chat".to_string(),
            is_group: true,
            last_message: Some(MessagePreview {
                sender: "Alice".to_string(),
                timestamp: None,
                text: "hello".to_string(),
            }),
        }
    }

    #[test]
    fn show_chats_sends_chats_update() {
        let (presenter, mut rx) = TuiPresenter::channel();
        presenter.show_chats(&[sample_chat()]);

        match rx.try_recv().expect("an update should have been sent") {
            TuiUpdate::Chats(chats) => {
                assert_eq!(chats.len(), 1);
                assert_eq!(chats[0].name, "Team Chat");
                assert_eq!(chats[0].id.as_str(), "19:abc@thread.v2");
            }
            other => panic!("expected Chats update, got {other:?}"),
        }
    }

    #[test]
    fn show_messages_sends_messages_update() {
        let (presenter, mut rx) = TuiPresenter::channel();
        presenter.show_messages(&[Message {
            sender: "Bob".to_string(),
            timestamp: None,
            content: "hi".to_string(),
        }]);

        match rx.try_recv().expect("an update should have been sent") {
            TuiUpdate::Messages(messages) => {
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].sender, "Bob");
                assert_eq!(messages[0].content, "hi");
            }
            other => panic!("expected Messages update, got {other:?}"),
        }
    }

    #[test]
    fn message_sent_sends_message_sent_update() {
        let (presenter, mut rx) = TuiPresenter::channel();
        presenter.message_sent(&SentMessage {
            id: "1".to_string(),
            chat_id: ChatId::new("19:abc@thread.v2").unwrap(),
            timestamp: None,
        });

        match rx.try_recv().expect("an update should have been sent") {
            TuiUpdate::MessageSent(sent) => {
                assert_eq!(sent.id, "1");
                assert_eq!(sent.chat_id.as_str(), "19:abc@thread.v2");
            }
            other => panic!("expected MessageSent update, got {other:?}"),
        }
    }

    #[test]
    fn show_presence_sends_presence_update() {
        let (presenter, mut rx) = TuiPresenter::channel();
        presenter.show_presence(&Presence::Busy);

        match rx.try_recv().expect("an update should have been sent") {
            TuiUpdate::Presence(presence) => assert_eq!(presence, Presence::Busy),
            other => panic!("expected Presence update, got {other:?}"),
        }
    }

    #[test]
    fn show_user_sends_user_update() {
        let (presenter, mut rx) = TuiPresenter::channel();
        presenter.show_user(&User {
            id: "id".to_string(),
            display_name: "Name".to_string(),
            email: Some("name@example.com".to_string()),
        });

        match rx.try_recv().expect("an update should have been sent") {
            TuiUpdate::User(user) => {
                assert_eq!(user.display_name, "Name");
                assert_eq!(user.email.as_deref(), Some("name@example.com"));
            }
            other => panic!("expected User update, got {other:?}"),
        }
    }

    #[test]
    fn show_teams_sends_teams_update() {
        let (presenter, mut rx) = TuiPresenter::channel();
        presenter.show_teams(&[Team {
            id: "t1".to_string(),
            display_name: "Engineering".to_string(),
            description: None,
        }]);

        match rx.try_recv().expect("an update should have been sent") {
            TuiUpdate::Teams(teams) => {
                assert_eq!(teams.len(), 1);
                assert_eq!(teams[0].display_name, "Engineering");
            }
            other => panic!("expected Teams update, got {other:?}"),
        }
    }

    #[test]
    fn notify_event_sends_event_update() {
        let (presenter, mut rx) = TuiPresenter::channel();
        presenter.notify_event(&RealtimeEvent::Unknown("ping".to_string()));

        match rx.try_recv().expect("an update should have been sent") {
            TuiUpdate::Event(RealtimeEvent::Unknown(raw)) => assert_eq!(raw, "ping"),
            other => panic!("expected Event(Unknown) update, got {other:?}"),
        }
    }

    #[test]
    fn show_error_sends_error_update() {
        let (presenter, mut rx) = TuiPresenter::channel();
        presenter.show_error(&DomainError::Invalid("bad".to_string()));

        match rx.try_recv().expect("an update should have been sent") {
            TuiUpdate::Error(msg) => assert_eq!(msg, "invalid input: bad"),
            other => panic!("expected Error update, got {other:?}"),
        }
    }

    #[test]
    fn send_after_receiver_dropped_does_not_panic() {
        let (presenter, rx) = TuiPresenter::channel();
        drop(rx);
        // Should silently discard rather than panic when the TUI has shut down.
        presenter.show_presence(&Presence::Offline);
    }

    #[test]
    fn each_method_sends_exactly_one_update() {
        let (presenter, mut rx) = TuiPresenter::channel();
        presenter.show_presence(&Presence::Available);
        // Exactly one update queued.
        assert!(matches!(rx.try_recv(), Ok(TuiUpdate::Presence(_))));
        assert_eq!(rx.try_recv().unwrap_err(), TryRecvError::Empty);
    }

    #[test]
    fn presenter_is_object_safe_as_presenter_port() {
        // Confirms TuiPresenter can be injected as `Arc<dyn PresenterPort>`,
        // the shape the composition root uses (task 11.2).
        let (presenter, _rx) = TuiPresenter::channel();
        let port: std::sync::Arc<dyn PresenterPort> = std::sync::Arc::new(presenter);
        port.show_presence(&Presence::Available);
    }
}
