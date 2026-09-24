//! Async backend: bridges the sync TUI event loop with the use-case services.
//!
//! Uses an mpsc channel for commands. The TUI sends `BackendCommand` values,
//! and a background tokio task executes each command by invoking the shared
//! use-case services (`ChatService`, `TeamService`, `PresenceService`). Those
//! services forward their results to the injected [`TuiPresenter`], which emits
//! [`TuiUpdate`](crate::adapters::presenter::TuiUpdate) messages over its own
//! channel; the TUI event loop drains that receiver and folds each update into
//! `App` state.
//!
//! This is the TUI side of the ports & adapters wiring (Requirement 8.4): the
//! TUI obtains its data through the *same* services the CLI uses, differing only
//! in the injected `PresenterPort` (a [`TuiPresenter`] instead of a
//! `ConsolePresenter`). The backend no longer calls `api::*_data` directly and
//! constructs no concrete adapter itself — the composition root (`main.rs`)
//! builds the services and hands them in (Requirement 8.1/8.2, P5).

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::adapters::presenter::TuiPresenter;
use crate::app::{ChatService, PresenceService, TeamService};
use crate::domain::DomainResult;
use crate::ports::PresenterPort;

/// Commands sent from the TUI event loop to the async backend.
pub enum BackendCommand {
    LoadTeams,
    LoadChats { limit: usize },
    LoadMessages { chat_id: String, limit: usize },
    SendMessage { chat_id: String, message: String },
    LoadUserInfo,
    LoadPresence,
}

/// The shared use-case services the TUI drives, wired at the composition root.
///
/// Each service already holds the injected [`TuiPresenter`] as its
/// `PresenterPort`, so invoking a service pushes results onto the presenter's
/// update channel. Held behind `Arc` so command tasks can be spawned freely.
pub struct TuiServices {
    /// The chat/team/presence services, present when the HTTP adapter was built
    /// successfully at the composition root. `None` means adapter construction
    /// failed (an auth/token problem): the failure was already surfaced as a
    /// [`TuiUpdate::Error`](crate::adapters::presenter::TuiUpdate::Error) and
    /// every backend command becomes a no-op, matching the old `ClientError`
    /// path where the TUI kept running to display the error and let the user
    /// quit.
    pub services: Option<WiredServices>,
    /// The same presenter injected into the services above, retained so the
    /// backend can surface a service *error* the same way it surfaces a success:
    /// as a [`TuiUpdate`](crate::adapters::presenter::TuiUpdate). Services only
    /// forward on the success path (errors propagate as `DomainError`), so the
    /// backend forwards errors through `show_error`, mirroring how the CLI's
    /// `present_result` presents a failed `DomainResult`.
    pub presenter: Arc<TuiPresenter>,
}

/// The three use-case services the TUI drives, bundled for injection.
///
/// Each already holds the injected [`TuiPresenter`] as its `PresenterPort`, so
/// invoking a service pushes results onto the presenter's update channel. Held
/// behind `Arc` so command tasks can be spawned freely.
pub struct WiredServices {
    pub chat: Arc<ChatService>,
    pub team: Arc<TeamService>,
    pub presence: Arc<PresenceService>,
}

/// Handle for interacting with the backend from the TUI side.
pub struct Backend {
    cmd_tx: mpsc::UnboundedSender<BackendCommand>,
}

impl Backend {
    /// Start the backend from the wired services.
    ///
    /// Spawns a tokio task that processes commands by invoking the services.
    /// Returns the `Backend` handle used by the TUI to dispatch commands; the
    /// results flow back over the [`TuiPresenter`]'s own channel, not this
    /// handle.
    pub fn start(services: TuiServices) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        tokio::spawn(backend_loop(cmd_rx, services));
        Self { cmd_tx }
    }

    /// Send a command to the backend (non-blocking).
    pub fn send(&self, cmd: BackendCommand) {
        if self.cmd_tx.send(cmd).is_err() {
            tracing::error!("Backend channel closed -- command dropped");
        }
    }
}

/// Forward a service outcome's error (if any) to the presenter.
///
/// On success the service already pushed its result to the presenter; on
/// failure nothing was forwarded, so we present the `DomainError` here as a
/// [`TuiUpdate::Error`](crate::adapters::presenter::TuiUpdate::Error), matching
/// the CLI's error surfacing.
fn forward_error(result: DomainResult<()>, presenter: &Arc<TuiPresenter>) {
    if let Err(e) = result {
        presenter.show_error(&e);
    }
}

/// Background loop that processes commands by invoking the shared services.
///
/// Each command is spawned as a separate task so a slow request never blocks
/// the loop. The services own the injected presenter, so their results reach
/// the TUI over the presenter's channel; only errors need explicit forwarding.
async fn backend_loop(
    mut cmd_rx: mpsc::UnboundedReceiver<BackendCommand>,
    services: TuiServices,
) {
    // If the HTTP adapter failed to build, there are no services to drive; the
    // error was already surfaced. Drain and drop commands so the TUI stays
    // responsive (the user can read the error and quit).
    let wired = match services.services {
        Some(w) => Arc::new(w),
        None => {
            while cmd_rx.recv().await.is_some() {}
            return;
        }
    };

    while let Some(cmd) = cmd_rx.recv().await {
        let wired = Arc::clone(&wired);
        let presenter = Arc::clone(&services.presenter);

        tokio::spawn(async move {
            match cmd {
                BackendCommand::LoadTeams => {
                    forward_error(wired.team.list_teams().await, &presenter);
                }
                BackendCommand::LoadChats { limit } => {
                    forward_error(wired.chat.list_chats(limit).await, &presenter);
                }
                BackendCommand::LoadMessages { chat_id, limit } => {
                    forward_error(wired.chat.read_messages(&chat_id, limit).await, &presenter);
                }
                BackendCommand::SendMessage { chat_id, message } => {
                    forward_error(
                        wired.chat.send_message(&chat_id, &message).await,
                        &presenter,
                    );
                }
                BackendCommand::LoadUserInfo => {
                    forward_error(wired.team.whoami().await, &presenter);
                }
                BackendCommand::LoadPresence => {
                    forward_error(wired.presence.get_presence().await, &presenter);
                }
            }
        });
    }
}
