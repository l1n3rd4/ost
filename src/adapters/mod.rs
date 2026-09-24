//! Adapters (ports & adapters).
//!
//! Concrete implementations of the [`ports`](crate::ports) traits, wiring the
//! domain to infrastructure (filesystem, HTTP, WebSocket, terminal). Adapters
//! may use infra crates internally but keep them out of public signatures.
//!
//! Sibling adapter tasks add further `pub mod` lines here. `config`, `http`,
//! `presenter`, and `realtime` are declared.

pub mod config;
pub mod http;
pub mod presenter;
pub mod realtime;
