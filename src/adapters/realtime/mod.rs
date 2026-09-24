//! Realtime adapters (ports & adapters).
//!
//! Concrete [`RealtimePort`](crate::ports::RealtimePort) implementations.
//! Currently a single `tokio-tungstenite`-backed adapter wrapping the Trouter
//! WebSocket stack; the infra crate stays out of every public signature.

pub mod tungstenite;

pub use tungstenite::TungsteniteRealtime;
