//! Realtime port.
//!
//! Subscribes to a stream of domain [`RealtimeEvent`]s. The reconnect/backoff
//! policy lives in the `RealtimeService`, not here. No `tokio-tungstenite` or
//! other infra type appears in a public signature; failures are reported as
//! [`DomainError`](crate::domain::DomainError) via [`DomainResult`].
//!
//! [`RealtimeEvent`]: crate::domain::RealtimeEvent

use async_trait::async_trait;
use futures_core::stream::BoxStream;

use crate::domain::{DomainResult, RealtimeEvent};

/// Subscription to realtime domain events.
#[async_trait]
pub trait RealtimePort: Send + Sync {
    /// Subscribe to the realtime event stream.
    ///
    /// Returns a boxed stream of domain [`RealtimeEvent`]s. Reconnect and
    /// backoff policy is the caller's (`RealtimeService`) concern.
    ///
    /// [`RealtimeEvent`]: crate::domain::RealtimeEvent
    async fn subscribe(&self) -> DomainResult<BoxStream<'static, RealtimeEvent>>;
}
