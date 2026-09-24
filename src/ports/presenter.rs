//! Presenter output port.
//!
//! `PresenterPort` is the output port that turns domain models into
//! presentation. It removes direct stdout writes from `api/`: use-case
//! services forward every result to a `PresenterPort` implementation
//! (`ConsolePresenter` for the CLI, `TuiPresenter` for the TUI) chosen at the
//! composition root, so business logic never calls `println!`/`eprintln!`.
//!
//! Like every port, it depends only on the domain core; each method takes
//! domain models by reference and returns nothing, keeping presentation
//! concerns outside the app and domain layers.

use crate::domain::{Chat, DomainError, Message, Presence, RealtimeEvent, SentMessage, Team, User};

/// Output port: renders domain models to a presentation surface.
///
/// Provides the seven core output operations required by the design (chats,
/// messages, sent-message confirmation, presence, user, teams, realtime
/// events) plus [`show_error`](PresenterPort::show_error) so use-case services
/// can forward failures to the presenter instead of writing to stderr or
/// exiting the process (Requirement 5.4).
pub trait PresenterPort: Send + Sync {
    /// Present a list of chats.
    fn show_chats(&self, chats: &[Chat]);

    /// Present a list of messages.
    fn show_messages(&self, messages: &[Message]);

    /// Confirm that a message was sent.
    fn message_sent(&self, sent: &SentMessage);

    /// Present the current presence state.
    fn show_presence(&self, presence: &Presence);

    /// Present a user/identity.
    fn show_user(&self, user: &User);

    /// Present a list of teams.
    fn show_teams(&self, teams: &[Team]);

    /// Notify of a realtime event.
    fn notify_event(&self, event: &RealtimeEvent);

    /// Present a failure result forwarded from a use-case service.
    fn show_error(&self, error: &DomainError);
}
