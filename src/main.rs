//! Teams CLI - Lightweight Microsoft Teams client
//!
//! A terminal-based Teams client for Linux.

mod adapters;
mod api;
mod app;
mod auth;
mod config;
mod domain;
mod models;
mod ports;
mod trouter;
mod tui;
mod cli;

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use adapters::config::TomlConfigRepository;
use adapters::http::ReqwestTeamsApi;
use adapters::presenter::ConsolePresenter;
use adapters::realtime::TungsteniteRealtime;
use app::{ChatService, PresenceService, RealtimeService, TeamService};
use domain::{DomainError, Presence};
use ports::{ConfigRepositoryPort, GraphPort, PresenterPort, RealtimePort, TeamsApiPort};

/// Parse a CLI presence status string into a domain [`Presence`].
///
/// Mirrors the legacy CLI interface (`available`, `busy`, `dnd`/`donotdisturb`,
/// `away`, `offline`); anything else becomes [`Presence::Custom`].
fn parse_presence(status: &str) -> Presence {
    match status.trim().to_lowercase().as_str() {
        "available" => Presence::Available,
        "busy" => Presence::Busy,
        "dnd" | "donotdisturb" => Presence::DoNotDisturb,
        "away" => Presence::Away,
        "offline" => Presence::Offline,
        _ => Presence::Custom(status.to_string()),
    }
}

/// Convert a [`DomainError`] into an [`anyhow::Error`] so `?` works with
/// `main() -> anyhow::Result<()>`.
fn into_anyhow(error: DomainError) -> anyhow::Error {
    anyhow::anyhow!(error.to_string())
}

/// Construct the single [`ReqwestTeamsApi`] used by API-backed commands.
///
/// This is the only place the concrete HTTP adapter is built. On failure the
/// error is presented via the [`ConsolePresenter`] before being converted to
/// `anyhow` for `main`.
async fn build_api(presenter: &Arc<dyn PresenterPort>) -> Result<Arc<ReqwestTeamsApi>> {
    match ReqwestTeamsApi::new().await {
        Ok(api) => Ok(Arc::new(api)),
        Err(e) => {
            presenter.show_error(&e);
            Err(into_anyhow(e))
        }
    }
}

/// Forward a service's [`DomainResult`] outcome: on error, present it via the
/// [`ConsolePresenter`] before converting to `anyhow` for `main`.
fn present_result(
    result: domain::DomainResult<()>,
    presenter: &Arc<dyn PresenterPort>,
) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            presenter.show_error(&e);
            Err(into_anyhow(e))
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging differently for TUI vs CLI mode.
    // TUI mode captures logs to a buffer (displayed in debug pane).
    // CLI mode logs to stderr as usual.
    let filter_str = if cli.verbose { "debug" } else { "info" };

    if matches!(cli.command, Commands::Tui) {
        // TUI mode: capture logs to a buffer for in-TUI display.
        let log_buffer = tui::LogBuffer::new();
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| filter_str.into()),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(false)
                    .with_ansi(false)
                    .with_writer(log_buffer.clone()),
            )
            .init();

        // --- Composition root (TUI) ---
        //
        // The TUI is a primary adapter that drives the *same* use-case services
        // as the CLI, differing only in the injected output port: a single
        // `TuiPresenter` instead of a `ConsolePresenter` (Requirement 8.4/10.3).
        // `main.rs` is the sole site constructing the concrete `TuiPresenter`
        // and `ReqwestTeamsApi` (Requirement 8.1/8.2, P5).
        //
        // The presenter feeds domain models into TUI state over a channel:
        // `channel()` returns the presenter to inject into the services and the
        // receiver the TUI event loop drains. Building the HTTP adapter performs
        // token loading/refresh; on failure the error is surfaced as a
        // `TuiUpdate::Error` and the TUI still launches (no functional
        // services), matching the old in-TUI auth-failure display.
        let (tui_presenter, updates) = adapters::presenter::TuiPresenter::channel();
        let tui_presenter = Arc::new(tui_presenter);
        let presenter: Arc<dyn PresenterPort> = tui_presenter.clone();

        let wired = match ReqwestTeamsApi::new().await {
            Ok(api) => {
                let api = Arc::new(api);
                let teams_api: Arc<dyn TeamsApiPort> = api.clone();
                let graph: Arc<dyn GraphPort> = api;
                Some(tui::WiredServices {
                    chat: Arc::new(ChatService::new(teams_api.clone(), presenter.clone())),
                    team: Arc::new(TeamService::new(graph, presenter.clone())),
                    presence: Arc::new(PresenceService::new(teams_api, presenter.clone())),
                })
            }
            Err(e) => {
                // Surface the auth/transport failure into the TUI status bar.
                presenter.show_error(&e);
                None
            }
        };

        let services = tui::TuiServices {
            services: wired,
            presenter: tui_presenter,
        };

        return tui::run(log_buffer, services, updates).await;
    }

    // CLI mode: log to stderr.
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| filter_str.into()),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    // --- Composition root ---
    //
    // `main.rs` is the sole site that constructs concrete adapters (Requirement
    // 8.1/8.2). For a CLI invocation the single output adapter is exactly one
    // [`ConsolePresenter`] (Requirement 8.5 / 10.3); the driven config adapter
    // is a single [`TomlConfigRepository`] behind [`ConfigRepositoryPort`]. The
    // HTTP adapter ([`ReqwestTeamsApi`]) is built lazily per API-backed command
    // via `build_api`, since constructing it performs token loading/refresh that
    // must not run for commands that never touch the network.
    let presenter: Arc<dyn PresenterPort> = Arc::new(ConsolePresenter::new());
    let config_repo: Arc<dyn ConfigRepositoryPort> = Arc::new(TomlConfigRepository::default());

    match cli.command {
        // Auth commands own an interactive OAuth device-code + derived-token
        // acquisition flow that is not yet expressed as a port (documented seam
        // in `app::AuthService`). They keep delegating to `auth::flow` so their
        // observable stdout/stderr and exit codes stay byte-identical
        // (Requirement 10.1); the composition root still owns the config adapter
        // wired below for the ported slices.
        Commands::Login { force } => {
            tracing::info!("Starting authentication flow...");
            auth::login(force).await?;
        }
        Commands::Logout => {
            tracing::info!("Logging out...");
            // Logout is fully expressible through `ConfigRepositoryPort`: load
            // the persisted credentials, drop every token plus the regional
            // metadata, and save. Tenant id is preserved, matching the legacy
            // `Config::clear_tokens` semantics exactly, so the persisted file
            // and this command's stdout stay byte-identical (Requirement 10.1).
            let logout = || -> domain::DomainResult<()> {
                let mut creds = config_repo.load()?;
                creds.access_token = None;
                creds.refresh_token = None;
                creds.skype_token = None;
                creds.graph_token = None;
                creds.ic3_token = None;
                creds.recorder_token = None;
                creds.region_gtms = None;
                config_repo.save(&creds)
            };
            present_result(logout(), &presenter)?;
            println!("Logged out.");
        }
        Commands::Status => {
            auth::status().await?;
        }
        Commands::Teams => {
            let api = build_api(&presenter).await?;
            let graph: Arc<dyn GraphPort> = api;
            let service = TeamService::new(graph, presenter.clone());
            present_result(service.list_teams().await, &presenter)?;
        }
        Commands::Whoami => {
            let api = build_api(&presenter).await?;
            let graph: Arc<dyn GraphPort> = api;
            let service = TeamService::new(graph, presenter.clone());
            present_result(service.whoami().await, &presenter)?;
        }
        Commands::Chats { limit } => {
            tracing::info!("Fetching chats...");
            let api = build_api(&presenter).await?;
            let teams_api: Arc<dyn TeamsApiPort> = api;
            let service = ChatService::new(teams_api, presenter.clone());
            present_result(service.list_chats(limit).await, &presenter)?;
        }
        Commands::Read { chat_id, limit } => {
            let api = build_api(&presenter).await?;
            let teams_api: Arc<dyn TeamsApiPort> = api;
            let service = ChatService::new(teams_api, presenter.clone());
            present_result(service.read_messages(&chat_id, limit).await, &presenter)?;
        }
        Commands::Send { to, message } => {
            tracing::info!("Sending message...");
            let api = build_api(&presenter).await?;
            let teams_api: Arc<dyn TeamsApiPort> = api;
            let service = ChatService::new(teams_api, presenter.clone());
            present_result(service.send_message(&to, &message).await, &presenter)?;
        }
        Commands::Trouter => {
            // The realtime slice is fully expressed through `RealtimePort` +
            // `RealtimeService` (Requirements 8.1/8.3): the composition root is
            // the sole site constructing the concrete `TungsteniteRealtime`
            // adapter, and the service owns the subscribe + reconnect loop,
            // forwarding every `RealtimeEvent` to the single `ConsolePresenter`
            // via `PresenterPort::notify_event`. This supersedes the legacy
            // `trouter::connect_and_run` path (Requirement 10.1).
            let realtime: Arc<dyn RealtimePort> = Arc::new(TungsteniteRealtime::new());
            let service = RealtimeService::new(realtime, presenter.clone());
            present_result(service.run().await, &presenter)?;
        }
        Commands::Presence { set } => {
            let api = build_api(&presenter).await?;
            let teams_api: Arc<dyn TeamsApiPort> = api;
            let service = PresenceService::new(teams_api, presenter.clone());
            match set {
                Some(status) => {
                    tracing::info!("Setting presence to {}...", status);
                    let presence = parse_presence(&status);
                    present_result(service.set_presence(&presence).await, &presenter)?;
                }
                None => {
                    present_result(service.get_presence().await, &presenter)?;
                }
            }
        }
        // TUI is handled above with early return.
        Commands::Tui => unreachable!(),
    }

    Ok(())
}
