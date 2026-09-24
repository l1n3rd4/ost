//! Presence use-case service.
//!
//! `PresenceService` orchestrates the [`TeamsApiPort`] and [`PresenterPort`]
//! for presence intents (get/set presence). It forwards every result to the
//! presenter and never writes to stdout/stderr (Requirement 5.2). Ports are
//! injected as `Arc<dyn Port>` (Requirement 6.7).

use std::sync::Arc;

use crate::domain::{DomainResult, Presence};
use crate::ports::{PresenterPort, TeamsApiPort};

/// Use-case service for presence operations.
pub struct PresenceService {
    api: Arc<dyn TeamsApiPort>,
    presenter: Arc<dyn PresenterPort>,
}

impl PresenceService {
    /// Construct a `PresenceService` from the ports it depends on.
    pub fn new(api: Arc<dyn TeamsApiPort>, presenter: Arc<dyn PresenterPort>) -> Self {
        Self { api, presenter }
    }

    /// Get the current presence and forward it to the presenter.
    pub async fn get_presence(&self) -> DomainResult<()> {
        let presence = self.api.get_presence().await?;
        self.presenter.show_presence(&presence);
        Ok(())
    }

    /// Set the current presence, then forward the applied value to the
    /// presenter as confirmation. Output is routed through the presenter, never
    /// via `println!`.
    pub async fn set_presence(&self, presence: &Presence) -> DomainResult<()> {
        self.api.set_presence(presence).await?;
        self.presenter.show_presence(presence);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! Use-case tests for `PresenceService` with an in-memory `TeamsApiPort`
    //! fake.
    //!
    //! Property 6: Testability — runs with 0 network calls and 0 filesystem
    //! operations (Requirements 9.1, 9.2, 9.3). The fake returns a fixed
    //! `Presence` and does no I/O; the recording presenter captures calls so we
    //! can assert forwarding (Requirement 6.3) and error propagation.

    use super::*;
    use crate::domain::{Chat, ChatId, DomainError, Message, SentMessage};
    use crate::ports::PresenterPort;
    use std::sync::Mutex;

    /// In-memory `TeamsApiPort` fake for presence. Returns a fixed presence and
    /// performs no I/O. Only the presence ops are used by `PresenceService`;
    /// the chat ops are unreachable and guard against accidental calls.
    struct FakeTeamsApi {
        presence: Presence,
        fail: bool,
    }

    impl FakeTeamsApi {
        fn new() -> Self {
            Self {
                presence: Presence::Available,
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                presence: Presence::Available,
                fail: true,
            }
        }
    }

    #[async_trait::async_trait]
    impl TeamsApiPort for FakeTeamsApi {
        async fn list_chats(&self, _limit: usize) -> DomainResult<Vec<Chat>> {
            unreachable!("PresenceService must not call list_chats");
        }
        async fn read_messages(
            &self,
            _chat_id: &ChatId,
            _limit: usize,
        ) -> DomainResult<Vec<Message>> {
            unreachable!("PresenceService must not call read_messages");
        }
        async fn send_message(&self, _chat_id: &ChatId, _text: &str) -> DomainResult<SentMessage> {
            unreachable!("PresenceService must not call send_message");
        }
        async fn get_presence(&self) -> DomainResult<Presence> {
            if self.fail {
                return Err(DomainError::Transport("presence read failed".to_string()));
            }
            Ok(self.presence.clone())
        }
        async fn set_presence(&self, _presence: &Presence) -> DomainResult<()> {
            if self.fail {
                return Err(DomainError::Transport("presence write failed".to_string()));
            }
            Ok(())
        }
    }

    /// Presenter fake that records presence calls instead of printing.
    #[derive(Default)]
    struct RecordingPresenter {
        presences: Mutex<Vec<Presence>>,
        errors: Mutex<Vec<String>>,
    }

    impl RecordingPresenter {
        fn shown_presences(&self) -> Vec<Presence> {
            self.presences.lock().unwrap().clone()
        }
    }

    impl PresenterPort for RecordingPresenter {
        fn show_chats(&self, _chats: &[Chat]) {
            unreachable!("PresenceService must not call show_chats");
        }
        fn show_messages(&self, _messages: &[Message]) {
            unreachable!("PresenceService must not call show_messages");
        }
        fn message_sent(&self, _sent: &SentMessage) {
            unreachable!("PresenceService must not call message_sent");
        }
        fn show_presence(&self, presence: &Presence) {
            self.presences.lock().unwrap().push(presence.clone());
        }
        fn show_user(&self, _user: &crate::domain::User) {
            unreachable!("PresenceService must not call show_user");
        }
        fn show_teams(&self, _teams: &[crate::domain::Team]) {
            unreachable!("PresenceService must not call show_teams");
        }
        fn notify_event(&self, _event: &crate::domain::RealtimeEvent) {
            unreachable!("PresenceService must not call notify_event");
        }
        fn show_error(&self, error: &DomainError) {
            self.errors.lock().unwrap().push(error.to_string());
        }
    }

    #[tokio::test]
    async fn get_presence_forwards_fake_presence_to_presenter() {
        let api = Arc::new(FakeTeamsApi::new());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = PresenceService::new(api, presenter.clone());

        service.get_presence().await.expect("get_presence should succeed");

        assert_eq!(presenter.shown_presences(), vec![Presence::Available]);
    }

    #[tokio::test]
    async fn set_presence_forwards_applied_value_to_presenter() {
        let api = Arc::new(FakeTeamsApi::new());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = PresenceService::new(api, presenter.clone());

        service
            .set_presence(&Presence::Busy)
            .await
            .expect("set_presence should succeed");

        assert_eq!(presenter.shown_presences(), vec![Presence::Busy]);
    }

    #[tokio::test]
    async fn get_presence_propagates_port_error() {
        let api = Arc::new(FakeTeamsApi::failing());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = PresenceService::new(api, presenter.clone());

        let result = service.get_presence().await;

        assert!(matches!(result, Err(DomainError::Transport(_))));
        assert!(presenter.shown_presences().is_empty());
    }

    #[tokio::test]
    async fn set_presence_propagates_port_error() {
        let api = Arc::new(FakeTeamsApi::failing());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = PresenceService::new(api, presenter.clone());

        let result = service.set_presence(&Presence::Away).await;

        assert!(matches!(result, Err(DomainError::Transport(_))));
        assert!(presenter.shown_presences().is_empty());
    }
}
