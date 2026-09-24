//! WebSocket realtime adapter implementing [`RealtimePort`] over `tokio-tungstenite`.
//!
//! [`TungsteniteRealtime`] wraps the existing Trouter stack
//! ([`TrouterSocket`](crate::trouter::websocket::TrouterSocket) plus the
//! `trouter::session`/`trouter::registrar` helpers) to deliver a stream of
//! domain [`RealtimeEvent`]s. It performs one full negotiate → connect →
//! handshake → register sequence in [`subscribe`](RealtimeReceiver), then
//! spawns a background task that pumps incoming socket.io frames through a
//! channel; the receiver side is handed back as a
//! [`BoxStream`](futures_core::stream::BoxStream) of domain events.
//!
//! No `tokio-tungstenite` (or `reqwest`/`anyhow`) type appears in any public
//! signature — `subscribe` returns only [`DomainResult`] and a stream of the
//! domain [`RealtimeEvent`] enum (Requirement 7.7). Every incoming WebSocket
//! text frame is translated to a [`RealtimeEvent`] by the pure
//! [`frame_to_event`] mapper (Requirement 7.8), keeping the socket.io framing
//! knowledge inside this adapter.
//!
//! Reconnect/backoff policy is intentionally *not* implemented here; that is
//! the `RealtimeService`'s concern (task 10.2). When the underlying connection
//! closes, the spawned task ends and the stream simply terminates.

use async_trait::async_trait;
use futures::stream::StreamExt;
use futures_core::stream::BoxStream;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::domain::{ChatId, DomainError, DomainResult, Message, RealtimeEvent};
use crate::ports::RealtimePort;
use crate::trouter::{registrar, session, websocket::TrouterSocket};

/// Trouter registrar TTL used when (re)registering the endpoint (seconds).
const REGISTRAR_TTL_SECS: u64 = 86400;

/// `tokio-tungstenite`-backed adapter for [`RealtimePort`].
///
/// Holds no live connection itself; each [`subscribe`](RealtimePort::subscribe)
/// call negotiates a fresh Trouter session and returns an independent event
/// stream. The `tokio-tungstenite` dependency never escapes a public signature.
#[derive(Debug, Default, Clone)]
pub struct TungsteniteRealtime {
    _private: (),
}

impl TungsteniteRealtime {
    /// Build the adapter. Construction performs no I/O; the connection is
    /// established lazily on [`subscribe`](RealtimePort::subscribe).
    pub fn new() -> Self {
        Self { _private: () }
    }
}

/// Classify an infra (`anyhow`) failure from the Trouter stack into a
/// [`DomainError`], preserving the message and leaking no infra type.
///
/// Trouter negotiation/connection failures are transport-level, so they map to
/// [`DomainError::Transport`]. Missing/expired credentials are surfaced
/// separately as [`DomainError::Unauthenticated`] before this is reached.
fn map_err(err: anyhow::Error) -> DomainError {
    DomainError::Transport(format!("{:#}", err))
}

/// Translate one incoming socket.io text frame into a domain [`RealtimeEvent`].
///
/// Pure and I/O-free so it can be unit-tested without a live socket. Mirrors
/// the socket.io framing understood by the legacy `trouter::handle_frame`:
///
/// - `1::` handshake, `2::` heartbeat, and `6:` acks carry no domain event →
///   `None`.
/// - `5:ACK_ID:ENDPOINT:JSON` event frames carry a payload. A frame whose
///   endpoint/routing names `SkypeSpacesWeb` (the calling channel) becomes
///   [`RealtimeEvent::CallInfo`]; any other event frame with a JSON payload is
///   surfaced as [`RealtimeEvent::Unknown`] carrying the raw JSON, since this
///   adapter does not yet parse Teams message envelopes into
///   [`RealtimeEvent::MessageReceived`].
/// - Anything else (including malformed frames) → [`RealtimeEvent::Unknown`]
///   carrying the raw frame, so no delivery is silently dropped.
///
/// Returns `None` for frames that are pure protocol chatter (handshake,
/// heartbeat, ack) and therefore should not appear on the domain stream.
fn frame_to_event(frame: &str) -> Option<RealtimeEvent> {
    // Protocol frames with no domain meaning.
    if frame.starts_with("1::") || frame.starts_with("2::") || frame.starts_with("6:") {
        return None;
    }

    // Socket.IO v1 event frame: `5:ACK_ID:ENDPOINT:JSON`.
    if frame.starts_with("5:") {
        let after_5 = &frame[2..]; // skip "5:"
        let json_str = after_5
            .find("::")
            .map(|pos| &after_5[pos + 2..])
            .filter(|s| s.starts_with('{'));

        return match json_str {
            Some(payload) if frame.contains("SkypeSpacesWeb") => {
                Some(RealtimeEvent::CallInfo(payload.to_string()))
            }
            Some(payload) => Some(event_from_payload(payload)),
            // Event frame without a JSON payload: keep the raw frame.
            None => Some(RealtimeEvent::Unknown(frame.to_string())),
        };
    }

    // `3:::` Trouter data frames and any other framing: surface raw so the
    // caller still observes the traffic.
    Some(RealtimeEvent::Unknown(frame.to_string()))
}

/// Best-effort mapping of a socket.io event JSON payload to a domain event.
///
/// Teams message envelopes vary; when a payload looks like a chat message
/// (carries a recognizable conversation id and body) it becomes
/// [`RealtimeEvent::MessageReceived`], otherwise the raw JSON is preserved as
/// [`RealtimeEvent::Unknown`]. Any parse miss falls back to `Unknown` rather
/// than failing the stream.
fn event_from_payload(payload: &str) -> RealtimeEvent {
    let value: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => return RealtimeEvent::Unknown(payload.to_string()),
    };

    // Trouter message notifications nest the resource under `body`/`resource`.
    // Probe a few common shapes; on any miss, preserve the raw payload.
    let resource = value
        .get("body")
        .and_then(|b| b.get("resource"))
        .or_else(|| value.get("resource"))
        .unwrap_or(&value);

    let chat_raw = resource
        .get("to")
        .and_then(|v| v.as_str())
        .or_else(|| resource.get("conversationId").and_then(|v| v.as_str()))
        .or_else(|| resource.get("conversationLink").and_then(|v| v.as_str()));

    let content = resource
        .get("content")
        .and_then(|v| v.as_str())
        .or_else(|| resource.get("body").and_then(|v| v.as_str()));

    match (chat_raw, content) {
        (Some(chat), Some(text)) => match ChatId::new(chat) {
            Ok(chat_id) => {
                let sender = resource
                    .get("imdisplayname")
                    .and_then(|v| v.as_str())
                    .or_else(|| resource.get("from").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                RealtimeEvent::MessageReceived {
                    chat_id,
                    message: Message {
                        sender,
                        timestamp: None,
                        content: text.to_string(),
                    },
                }
            }
            // A present-but-empty conversation id is not a valid domain ChatId;
            // preserve the raw payload rather than dropping the notification.
            Err(_) => RealtimeEvent::Unknown(payload.to_string()),
        },
        _ => RealtimeEvent::Unknown(payload.to_string()),
    }
}

/// Establish one Trouter session and return a connected [`TrouterSocket`].
///
/// Reuses the existing negotiate → session-id → connect → handshake → register
/// sequence from `trouter`. Credential problems map to
/// [`DomainError::Unauthenticated`]; all other infra failures map through
/// [`map_err`] to [`DomainError::Transport`].
async fn connect() -> DomainResult<TrouterSocket> {
    let config =
        Config::load().map_err(|e| DomainError::Storage(format!("failed to load config: {e:#}")))?;

    let skype_token = config.get_skype_token().ok_or_else(|| {
        DomainError::Unauthenticated(
            "no skype token found. Run `teams-cli login` first.".to_string(),
        )
    })?;
    if skype_token.is_expired() {
        return Err(DomainError::TokenExpired(
            "skype token expired. Run `teams-cli login` to refresh.".to_string(),
        ));
    }

    let skype_token_str = &skype_token.token;
    let http = reqwest::Client::new();

    let (session, epid) = session::negotiate(&http, skype_token_str)
        .await
        .map_err(map_err)?;

    let session_id = session::get_session_id(&http, &session, skype_token_str, &epid)
        .await
        .map_err(map_err)?;

    let mut ws = TrouterSocket::connect(&session, &session_id, &epid)
        .await
        .map_err(map_err)?;

    // Consume the handshake frame (1::) so the stream starts on real traffic.
    let frame = ws
        .recv_frame()
        .await
        .map_err(map_err)?
        .ok_or_else(|| DomainError::Transport("connection closed before handshake".to_string()))?;
    if !frame.starts_with("1::") {
        tracing::warn!("Expected 1:: handshake, got: {}", frame);
    }

    // Register the endpoint so the service starts routing notifications to us.
    // A registration failure is non-fatal to establishing the stream (mirrors
    // the legacy `connect_and_run_inner` behavior), so it is only logged.
    if let Some(ref reg_url) = session.registrar_url {
        if let Err(e) =
            registrar::register(&http, skype_token_str, reg_url, &session.surl).await
        {
            tracing::warn!("Initial registrar registration failed: {:#}", e);
        }
    }
    let _ = REGISTRAR_TTL_SECS; // TTL refresh loop is the service's concern.

    Ok(ws)
}

#[async_trait]
impl RealtimePort for TungsteniteRealtime {
    async fn subscribe(&self) -> DomainResult<BoxStream<'static, RealtimeEvent>> {
        let mut ws = connect().await?;

        // Bridge the blocking recv loop to a stream via an unbounded channel:
        // the spawned task owns the socket and forwards mapped domain events;
        // the returned stream is the receiver side. When the socket closes (or
        // errors), the task ends, the sender drops, and the stream terminates —
        // reconnect is left to the RealtimeService.
        let (tx, rx) = mpsc::unbounded_channel::<RealtimeEvent>();

        tokio::spawn(async move {
            loop {
                match ws.recv_frame().await {
                    Ok(Some(frame)) => {
                        if let Some(event) = frame_to_event(&frame) {
                            if tx.send(event).is_err() {
                                // Receiver (stream) dropped: stop pumping.
                                break;
                            }
                        }
                    }
                    Ok(None) => {
                        tracing::info!("Trouter WebSocket closed by server");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("Trouter WebSocket recv error: {:#}", e);
                        break;
                    }
                }
            }
        });

        // Adapt the channel receiver into a `Stream` using `futures::unfold`,
        // avoiding an extra `tokio-stream` feature. Each poll awaits the next
        // forwarded event; `recv` returning `None` (all senders dropped) ends
        // the stream.
        let stream = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        });

        Ok(stream.boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_heartbeat_and_ack_frames_are_dropped() {
        assert!(frame_to_event("1::").is_none());
        assert!(frame_to_event("2::").is_none());
        assert!(frame_to_event("6:12::").is_none());
    }

    #[test]
    fn call_info_frame_maps_to_call_info() {
        // A SkypeSpacesWeb calling notification: socket.io v1 event frames carry
        // an empty endpoint (`5:ACK::JSON`) and the SkypeSpacesWeb routing marker
        // lives in the JSON payload (mirrors `trouter::handle_frame`).
        let frame = r#"5:1+::{"url":"SkypeSpacesWeb","foo":"bar"}"#;
        match frame_to_event(frame) {
            Some(RealtimeEvent::CallInfo(payload)) => {
                assert_eq!(payload, r#"{"url":"SkypeSpacesWeb","foo":"bar"}"#);
            }
            other => panic!("expected CallInfo, got {other:?}"),
        }
    }

    #[test]
    fn generic_event_frame_without_message_shape_is_unknown() {
        let frame = r#"5:2+::{"hello":"world"}"#;
        match frame_to_event(frame) {
            Some(RealtimeEvent::Unknown(payload)) => {
                assert_eq!(payload, r#"{"hello":"world"}"#);
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn message_shaped_event_frame_maps_to_message_received() {
        // A payload carrying a conversation id + content becomes a domain
        // MessageReceived with a validated ChatId.
        let frame = r#"5:3+::{"resource":{"to":"19:abc@thread.v2","content":"hi there","imdisplayname":"Alice"}}"#;
        match frame_to_event(frame) {
            Some(RealtimeEvent::MessageReceived { chat_id, message }) => {
                assert_eq!(chat_id.as_str(), "19:abc@thread.v2");
                assert_eq!(message.content, "hi there");
                assert_eq!(message.sender, "Alice");
                assert!(message.timestamp.is_none());
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[test]
    fn event_frame_with_non_json_payload_is_unknown_raw() {
        // `5:` frame whose payload after `::` is not JSON: preserve the raw frame.
        let frame = "5:4+::not-json";
        match frame_to_event(frame) {
            Some(RealtimeEvent::Unknown(raw)) => assert_eq!(raw, frame),
            other => panic!("expected Unknown raw frame, got {other:?}"),
        }
    }

    #[test]
    fn trouter_data_frame_is_surfaced_raw_as_unknown() {
        let frame = r#"3:::{"id":7}"#;
        match frame_to_event(frame) {
            Some(RealtimeEvent::Unknown(raw)) => assert_eq!(raw, frame),
            other => panic!("expected Unknown raw frame, got {other:?}"),
        }
    }

    #[test]
    fn message_payload_with_empty_chat_id_falls_back_to_unknown() {
        let frame = r#"5:5+::{"resource":{"to":"","content":"hi"}}"#;
        match frame_to_event(frame) {
            Some(RealtimeEvent::Unknown(_)) => {}
            other => panic!("expected Unknown for empty chat id, got {other:?}"),
        }
    }

    #[test]
    fn map_err_preserves_message_as_transport() {
        let err = anyhow::anyhow!("negotiation timed out");
        match map_err(err) {
            DomainError::Transport(msg) => assert!(msg.contains("negotiation timed out")),
            other => panic!("expected Transport, got {other:?}"),
        }
    }
}
