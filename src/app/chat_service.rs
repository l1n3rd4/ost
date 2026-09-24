//! Chat use-case service.
//!
//! `ChatService` orchestrates the [`TeamsApiPort`] and [`PresenterPort`] for
//! the chat-related intents (list chats, read messages, send message). It
//! validates input through domain rules *before* any I/O and forwards every
//! result to the presenter; it never writes to stdout/stderr and never exits
//! the process (Requirement 5.2, 5.4). Ports are injected as `Arc<dyn Port>`
//! (Requirement 6.7), so the service is testable with fakes.

use std::sync::Arc;

use crate::domain::{ChatId, DomainResult, SendMessageCommand};
use crate::ports::{PresenterPort, TeamsApiPort};

/// Use-case service for chat operations.
pub struct ChatService {
    api: Arc<dyn TeamsApiPort>,
    presenter: Arc<dyn PresenterPort>,
}

impl ChatService {
    /// Construct a `ChatService` from the ports it depends on.
    pub fn new(api: Arc<dyn TeamsApiPort>, presenter: Arc<dyn PresenterPort>) -> Self {
        Self { api, presenter }
    }

    /// List up to `limit` chats and forward them to the presenter.
    pub async fn list_chats(&self, limit: usize) -> DomainResult<()> {
        let chats = self.api.list_chats(limit).await?;
        self.presenter.show_chats(&chats);
        Ok(())
    }

    /// Read up to `limit` messages from `chat_id` and forward them.
    ///
    /// The `chat_id` is validated via [`ChatId::new`] (domain validation)
    /// before any I/O is performed.
    pub async fn read_messages(&self, chat_id: &str, limit: usize) -> DomainResult<()> {
        let id = ChatId::new(chat_id)?;
        let messages = self.api.read_messages(&id, limit).await?;
        self.presenter.show_messages(&messages);
        Ok(())
    }

    /// Send `text` to `chat_id` and forward the sent-message confirmation.
    ///
    /// The command is validated via [`SendMessageCommand::new`] (which also
    /// validates the `chat_id` through [`ChatId::new`]) before any I/O.
    pub async fn send_message(&self, chat_id: &str, text: &str) -> DomainResult<()> {
        let cmd = SendMessageCommand::new(ChatId::new(chat_id)?, text)?;
        let sent = self.api.send_message(&cmd.chat_id, &cmd.text).await?;
        self.presenter.message_sent(&sent);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! Use-case tests for `ChatService` with in-memory port fakes.
    //!
    //! Property 6: Testability — the use-cases run with **0 network calls and
    //! 0 filesystem operations**. The fakes here return fixed in-memory domain
    //! models and record calls; they perform no I/O by construction, so every
    //! test below exercises the service without touching the network or disk
    //! (Requirements 9.1, 9.2, 9.3). We assert forwarding of results to the
    //! presenter (Requirements 6.1, 6.2, 6.3) and error propagation.

    use super::*;
    use crate::domain::{Chat, ChatId, DomainError, Message, SentMessage};
    use crate::ports::PresenterPort;
    use std::sync::Mutex;

    /// In-memory `TeamsApiPort` fake. Returns fixed models and performs no I/O.
    ///
    /// Setting `fail` makes every operation return `DomainError::NotFound`,
    /// which lets us test error propagation through the service.
    struct FakeTeamsApi {
        chats: Vec<Chat>,
        messages: Vec<Message>,
        sent: SentMessage,
        fail: bool,
    }

    impl FakeTeamsApi {
        fn new() -> Self {
            Self {
                chats: vec![
                    Chat {
                        id: ChatId::new("19:a@thread.v2").unwrap(),
                        name: "Alice".to_string(),
                        is_group: false,
                        last_message: None,
                    },
                    Chat {
                        id: ChatId::new("19:b@thread.v2").unwrap(),
                        name: "Team B".to_string(),
                        is_group: true,
                        last_message: None,
                    },
                ],
                messages: vec![Message {
                    sender: "Alice".to_string(),
                    timestamp: None,
                    content: "hello".to_string(),
                }],
                sent: SentMessage {
                    id: "msg-1".to_string(),
                    chat_id: ChatId::new("19:a@thread.v2").unwrap(),
                    timestamp: None,
                },
                fail: false,
            }
        }

        fn failing() -> Self {
            let mut api = Self::new();
            api.fail = true;
            api
        }
    }

    #[async_trait::async_trait]
    impl TeamsApiPort for FakeTeamsApi {
        async fn list_chats(&self, _limit: usize) -> DomainResult<Vec<Chat>> {
            if self.fail {
                return Err(DomainError::NotFound("no chats".to_string()));
            }
            Ok(self.chats.clone())
        }

        async fn read_messages(
            &self,
            _chat_id: &ChatId,
            _limit: usize,
        ) -> DomainResult<Vec<Message>> {
            if self.fail {
                return Err(DomainError::NotFound("no messages".to_string()));
            }
            Ok(self.messages.clone())
        }

        async fn send_message(&self, _chat_id: &ChatId, _text: &str) -> DomainResult<SentMessage> {
            if self.fail {
                return Err(DomainError::Transport("send failed".to_string()));
            }
            Ok(self.sent.clone())
        }

        async fn get_presence(&self) -> DomainResult<crate::domain::Presence> {
            unreachable!("ChatService must not call get_presence");
        }

        async fn set_presence(&self, _presence: &crate::domain::Presence) -> DomainResult<()> {
            unreachable!("ChatService must not call set_presence");
        }
    }

    /// Presenter fake that records every call instead of printing. Uses
    /// interior mutability so the `&self` port methods can record.
    #[derive(Default)]
    struct RecordingPresenter {
        chats: Mutex<Vec<Vec<String>>>,
        messages: Mutex<Vec<Vec<String>>>,
        sent: Mutex<Vec<String>>,
        errors: Mutex<Vec<String>>,
    }

    impl RecordingPresenter {
        fn shown_chats(&self) -> usize {
            self.chats.lock().unwrap().len()
        }
        fn last_chat_names(&self) -> Vec<String> {
            self.chats.lock().unwrap().last().cloned().unwrap_or_default()
        }
        fn shown_messages(&self) -> usize {
            self.messages.lock().unwrap().len()
        }
        fn last_message_contents(&self) -> Vec<String> {
            self.messages.lock().unwrap().last().cloned().unwrap_or_default()
        }
        fn sent_ids(&self) -> Vec<String> {
            self.sent.lock().unwrap().clone()
        }
        fn error_count(&self) -> usize {
            self.errors.lock().unwrap().len()
        }
    }

    impl PresenterPort for RecordingPresenter {
        fn show_chats(&self, chats: &[Chat]) {
            self.chats
                .lock()
                .unwrap()
                .push(chats.iter().map(|c| c.name.clone()).collect());
        }
        fn show_messages(&self, messages: &[Message]) {
            self.messages
                .lock()
                .unwrap()
                .push(messages.iter().map(|m| m.content.clone()).collect());
        }
        fn message_sent(&self, sent: &SentMessage) {
            self.sent.lock().unwrap().push(sent.id.clone());
        }
        fn show_presence(&self, _presence: &crate::domain::Presence) {
            unreachable!("ChatService must not call show_presence");
        }
        fn show_user(&self, _user: &crate::domain::User) {
            unreachable!("ChatService must not call show_user");
        }
        fn show_teams(&self, _teams: &[crate::domain::Team]) {
            unreachable!("ChatService must not call show_teams");
        }
        fn notify_event(&self, _event: &crate::domain::RealtimeEvent) {
            unreachable!("ChatService must not call notify_event");
        }
        fn show_error(&self, error: &DomainError) {
            self.errors.lock().unwrap().push(error.to_string());
        }
    }

    #[tokio::test]
    async fn list_chats_forwards_fake_chats_to_presenter() {
        let api = Arc::new(FakeTeamsApi::new());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = ChatService::new(api, presenter.clone());

        service.list_chats(10).await.expect("list_chats should succeed");

        // Forwarding: the fake's chats reached the presenter unchanged.
        assert_eq!(presenter.shown_chats(), 1);
        assert_eq!(
            presenter.last_chat_names(),
            vec!["Alice".to_string(), "Team B".to_string()]
        );
        // 0 net + 0 fs: the fake returned in-memory data with no I/O and no
        // error was forwarded.
        assert_eq!(presenter.error_count(), 0);
    }

    #[tokio::test]
    async fn read_messages_forwards_fake_messages_to_presenter() {
        let api = Arc::new(FakeTeamsApi::new());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = ChatService::new(api, presenter.clone());

        service
            .read_messages("19:a@thread.v2", 20)
            .await
            .expect("read_messages should succeed");

        assert_eq!(presenter.shown_messages(), 1);
        assert_eq!(presenter.last_message_contents(), vec!["hello".to_string()]);
    }

    #[tokio::test]
    async fn send_message_forwards_sent_confirmation_to_presenter() {
        let api = Arc::new(FakeTeamsApi::new());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = ChatService::new(api, presenter.clone());

        service
            .send_message("19:a@thread.v2", "hi there")
            .await
            .expect("send_message should succeed");

        assert_eq!(presenter.sent_ids(), vec!["msg-1".to_string()]);
    }

    #[tokio::test]
    async fn read_messages_rejects_empty_chat_id_before_io() {
        let api = Arc::new(FakeTeamsApi::new());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = ChatService::new(api, presenter.clone());

        let result = service.read_messages("", 20).await;

        // Domain validation fails before any port call; nothing is forwarded.
        assert!(matches!(result, Err(DomainError::Invalid(_))));
        assert_eq!(presenter.shown_messages(), 0);
    }

    #[tokio::test]
    async fn send_message_rejects_empty_text_before_io() {
        let api = Arc::new(FakeTeamsApi::new());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = ChatService::new(api, presenter.clone());

        let result = service.send_message("19:a@thread.v2", "   ").await;

        assert!(matches!(result, Err(DomainError::Invalid(_))));
        assert!(presenter.sent_ids().is_empty());
    }

    #[tokio::test]
    async fn list_chats_propagates_port_error() {
        let api = Arc::new(FakeTeamsApi::failing());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = ChatService::new(api, presenter.clone());

        let result = service.list_chats(10).await;

        // The service propagates the port's error via `?`.
        assert!(matches!(result, Err(DomainError::NotFound(_))));
        // Nothing was forwarded to the presenter on the failure path.
        assert_eq!(presenter.shown_chats(), 0);
    }

    #[tokio::test]
    async fn send_message_propagates_port_error() {
        let api = Arc::new(FakeTeamsApi::failing());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = ChatService::new(api, presenter.clone());

        let result = service.send_message("19:a@thread.v2", "hi").await;

        assert!(matches!(result, Err(DomainError::Transport(_))));
        assert!(presenter.sent_ids().is_empty());
    }
}
