//! Application use-case services (ports & adapters).
//!
//! The app layer orchestrates domain rules and ports. Each service holds its
//! ports as `Arc<dyn Port>` (Requirement 6.7), validates input via the domain
//! before any I/O, and forwards every result to a [`PresenterPort`]. Services
//! must never call `println!`/`print!`/`eprintln!`/`eprint!` (Requirement 5.2)
//! and must never exit the process; failures propagate as `DomainError`.
//!
//! This module may depend only on the `domain` and `ports` layers, never on a
//! concrete adapter.
//!
//! [`PresenterPort`]: crate::ports::PresenterPort

pub mod auth_service;
pub mod chat_service;
pub mod presence_service;
pub mod realtime_service;
pub mod team_service;

pub use auth_service::AuthService;
pub use chat_service::ChatService;
pub use presence_service::PresenceService;
pub use realtime_service::RealtimeService;
pub use team_service::TeamService;
