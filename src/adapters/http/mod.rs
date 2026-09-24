//! HTTP adapter (ports & adapters).
//!
//! Wraps the `reqwest`-backed [`TeamsClient`](crate::api::client::TeamsClient)
//! and the `api::*_data` fetch/parse helpers to implement
//! [`TeamsApiPort`](crate::ports::TeamsApiPort) and
//! [`GraphPort`](crate::ports::GraphPort). Infra types (`reqwest`,
//! `serde_json`, `anyhow`) stay internal to this module.

pub mod reqwest_api;

pub use reqwest_api::ReqwestTeamsApi;
