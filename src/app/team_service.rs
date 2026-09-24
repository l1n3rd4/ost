//! Team use-case service.
//!
//! `TeamService` orchestrates the [`GraphPort`] and [`PresenterPort`] for the
//! team-related intent (list teams). Per the design it centers on `list_teams`;
//! a small [`whoami`](TeamService::whoami) passthrough is provided for
//! convenience since the service already holds a [`GraphPort`]. It never writes
//! to stdout/stderr (Requirement 5.2). Ports are injected as `Arc<dyn Port>`
//! (Requirement 6.7).

use std::sync::Arc;

use crate::domain::DomainResult;
use crate::ports::{GraphPort, PresenterPort};

/// Use-case service for team/identity operations.
pub struct TeamService {
    graph: Arc<dyn GraphPort>,
    presenter: Arc<dyn PresenterPort>,
}

impl TeamService {
    /// Construct a `TeamService` from the ports it depends on.
    pub fn new(graph: Arc<dyn GraphPort>, presenter: Arc<dyn PresenterPort>) -> Self {
        Self { graph, presenter }
    }

    /// List the current user's teams and forward them to the presenter.
    pub async fn list_teams(&self) -> DomainResult<()> {
        let teams = self.graph.list_teams().await?;
        self.presenter.show_teams(&teams);
        Ok(())
    }

    /// Resolve the current user and forward it to the presenter.
    ///
    /// Convenience passthrough over [`GraphPort::whoami`]; the service already
    /// holds a [`GraphPort`], so the composition root can wire the `whoami`
    /// command here without a separate service.
    pub async fn whoami(&self) -> DomainResult<()> {
        let user = self.graph.whoami().await?;
        self.presenter.show_user(&user);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! Use-case tests for `TeamService` with an in-memory `GraphPort` fake.
    //!
    //! Property 6: Testability — runs with 0 network calls and 0 filesystem
    //! operations (Requirements 9.1, 9.2, 9.3). The fake returns fixed domain
    //! models and does no I/O; the recording presenter captures calls so we can
    //! assert forwarding (Requirement 6.1) and error propagation.

    use super::*;
    use crate::domain::{DomainError, Team, User};
    use crate::ports::PresenterPort;
    use std::sync::Mutex;

    /// In-memory `GraphPort` fake. Returns fixed models and performs no I/O.
    struct FakeGraph {
        user: User,
        teams: Vec<Team>,
        fail: bool,
    }

    impl FakeGraph {
        fn new() -> Self {
            Self {
                user: User {
                    id: "u-1".to_string(),
                    display_name: "Alice".to_string(),
                    email: Some("alice@example.com".to_string()),
                },
                teams: vec![
                    Team {
                        id: "t-1".to_string(),
                        display_name: "Engineering".to_string(),
                        description: None,
                    },
                    Team {
                        id: "t-2".to_string(),
                        display_name: "Design".to_string(),
                        description: Some("design team".to_string()),
                    },
                ],
                fail: false,
            }
        }

        fn failing() -> Self {
            let mut g = Self::new();
            g.fail = true;
            g
        }
    }

    #[async_trait::async_trait]
    impl GraphPort for FakeGraph {
        async fn whoami(&self) -> DomainResult<User> {
            if self.fail {
                return Err(DomainError::Unauthenticated("no session".to_string()));
            }
            Ok(self.user.clone())
        }

        async fn list_teams(&self) -> DomainResult<Vec<Team>> {
            if self.fail {
                return Err(DomainError::NotFound("no teams".to_string()));
            }
            Ok(self.teams.clone())
        }
    }

    /// Presenter fake that records calls instead of printing.
    #[derive(Default)]
    struct RecordingPresenter {
        teams: Mutex<Vec<Vec<String>>>,
        users: Mutex<Vec<String>>,
        errors: Mutex<Vec<String>>,
    }

    impl RecordingPresenter {
        fn shown_teams(&self) -> usize {
            self.teams.lock().unwrap().len()
        }
        fn last_team_names(&self) -> Vec<String> {
            self.teams.lock().unwrap().last().cloned().unwrap_or_default()
        }
        fn shown_users(&self) -> Vec<String> {
            self.users.lock().unwrap().clone()
        }
    }

    impl PresenterPort for RecordingPresenter {
        fn show_chats(&self, _chats: &[crate::domain::Chat]) {
            unreachable!("TeamService must not call show_chats");
        }
        fn show_messages(&self, _messages: &[crate::domain::Message]) {
            unreachable!("TeamService must not call show_messages");
        }
        fn message_sent(&self, _sent: &crate::domain::SentMessage) {
            unreachable!("TeamService must not call message_sent");
        }
        fn show_presence(&self, _presence: &crate::domain::Presence) {
            unreachable!("TeamService must not call show_presence");
        }
        fn show_user(&self, user: &User) {
            self.users.lock().unwrap().push(user.display_name.clone());
        }
        fn show_teams(&self, teams: &[Team]) {
            self.teams
                .lock()
                .unwrap()
                .push(teams.iter().map(|t| t.display_name.clone()).collect());
        }
        fn notify_event(&self, _event: &crate::domain::RealtimeEvent) {
            unreachable!("TeamService must not call notify_event");
        }
        fn show_error(&self, error: &DomainError) {
            self.errors.lock().unwrap().push(error.to_string());
        }
    }

    #[tokio::test]
    async fn list_teams_forwards_fake_teams_to_presenter() {
        let graph = Arc::new(FakeGraph::new());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = TeamService::new(graph, presenter.clone());

        service.list_teams().await.expect("list_teams should succeed");

        assert_eq!(presenter.shown_teams(), 1);
        assert_eq!(
            presenter.last_team_names(),
            vec!["Engineering".to_string(), "Design".to_string()]
        );
    }

    #[tokio::test]
    async fn whoami_forwards_fake_user_to_presenter() {
        let graph = Arc::new(FakeGraph::new());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = TeamService::new(graph, presenter.clone());

        service.whoami().await.expect("whoami should succeed");

        assert_eq!(presenter.shown_users(), vec!["Alice".to_string()]);
    }

    #[tokio::test]
    async fn list_teams_propagates_port_error() {
        let graph = Arc::new(FakeGraph::failing());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = TeamService::new(graph, presenter.clone());

        let result = service.list_teams().await;

        assert!(matches!(result, Err(DomainError::NotFound(_))));
        assert_eq!(presenter.shown_teams(), 0);
    }

    #[tokio::test]
    async fn whoami_propagates_port_error() {
        let graph = Arc::new(FakeGraph::failing());
        let presenter = Arc::new(RecordingPresenter::default());
        let service = TeamService::new(graph, presenter.clone());

        let result = service.whoami().await;

        assert!(matches!(result, Err(DomainError::Unauthenticated(_))));
        assert!(presenter.shown_users().is_empty());
    }
}
