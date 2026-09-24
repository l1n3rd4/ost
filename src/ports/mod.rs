//! Ports (ports & adapters).
//!
//! Ports are traits stating the app's needs in domain terms. A port may depend
//! only on the domain core (and `async-trait`); it must never reference a
//! concrete adapter. Every public signature uses only domain types and returns
//! `DomainResult<_>`, so no infrastructure type leaks across the boundary.
//!
//! Sibling port tasks add further `pub mod` lines here (token store, config
//! repository, realtime, presenter). Only `api` is declared for now.

pub mod api;
pub mod config_repo;
pub mod presenter;
pub mod realtime;
pub mod token_store;

pub use api::{GraphPort, TeamsApiPort};
pub use config_repo::ConfigRepositoryPort;
pub use presenter::PresenterPort;
pub use realtime::RealtimePort;
pub use token_store::TokenStorePort;
