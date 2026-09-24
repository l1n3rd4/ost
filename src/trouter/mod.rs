//! Trouter v4 WebSocket push notification client
//!
//! Connects to Microsoft Teams' Trouter service to receive real-time
//! push notifications (messages, presence, calls, etc.).

pub mod registrar;
pub mod session;
pub mod websocket;

use anyhow::{Context, Result};
use std::time::{Duration, Instant};
use tokio::time;

use crate::config::Config;

/// Reason the inner connection loop exited.
enum DisconnectReason {
    /// Clean shutdown (Ctrl+C). Do not reconnect.
    Shutdown,
    /// Error or server-initiated close. Should reconnect.
    Error(anyhow::Error),
}

/// Run the Trouter connection with automatic reconnection.
///
/// On transient errors or server-initiated disconnects, reconnects with
/// exponential backoff (1s, 2s, 4s, ... capped at 64s). On clean shutdown
/// (Ctrl+C), exits immediately.
pub async fn connect_and_run() -> Result<()> {
    let mut backoff = 1u64;

    loop {
        match connect_and_run_inner().await {
            Ok(DisconnectReason::Shutdown) => {
                return Ok(());
            }
            Ok(DisconnectReason::Error(e)) => {
                // Connection was stable (>60s), reset backoff before reconnecting.
                backoff = 1;
                tracing::warn!(
                    "Trouter disconnected after stable session: {:#}. Reconnecting in 1s...",
                    e,
                );

                tokio::select! {
                    _ = time::sleep(Duration::from_secs(1)) => {}
                    _ = tokio::signal::ctrl_c() => {
                        println!("Shutting down...");
                        return Ok(());
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Trouter disconnected: {:#}. Reconnecting in {}s...",
                    e,
                    backoff
                );

                tokio::select! {
                    _ = time::sleep(Duration::from_secs(backoff)) => {}
                    _ = tokio::signal::ctrl_c() => {
                        println!("Shutting down...");
                        return Ok(());
                    }
                }

                backoff = (backoff * 2).min(64);
            }
        }
    }
}

/// Run one full Trouter session: negotiate, connect, event loop.
///
/// Returns `DisconnectReason::Shutdown` on clean Ctrl+C, or
/// `DisconnectReason::Error` when the connection should be retried.
async fn connect_and_run_inner() -> Result<DisconnectReason> {
    // Reload config each attempt so we pick up refreshed tokens.
    let config = Config::load().context("Failed to load config")?;

    let skype_token = config
        .get_skype_token()
        .context("No skype token found. Run `teams-cli login` first.")?;
    anyhow::ensure!(
        !skype_token.is_expired(),
        "Skype token expired. Run `teams-cli login` to refresh."
    );

    let skype_token_str = &skype_token.token;
    let http = reqwest::Client::new();

    // 1. Negotiate session (returns session info + epid)
    let (session, epid) = session::negotiate(&http, skype_token_str).await?;

    // 2. Get socket.io session ID (authenticated via X-Skypetoken header)
    let session_id = session::get_session_id(&http, &session, skype_token_str, &epid).await?;

    // 3. Connect WebSocket (auth is via session ID in URL, no headers needed)
    let mut ws = websocket::TrouterSocket::connect(&session, &session_id, &epid).await?;

    // 4. Wait for handshake frame (1::)
    let frame = ws
        .recv_frame()
        .await?
        .context("Connection closed before handshake")?;

    if !frame.starts_with("1::") {
        tracing::warn!("Expected 1:: handshake, got: {}", frame);
    } else {
        tracing::info!("Received handshake frame");
    }

    // 5. Register with registrar
    let registrar_ttl_secs: u64 = 86400;
    if let Some(ref reg_url) = session.registrar_url {
        if let Err(e) = registrar::register(&http, skype_token_str, reg_url, &session.surl).await {
            tracing::warn!("Initial registrar registration failed: {:#}", e);
        }
    }

    // 6. Event loop: recv frames, send heartbeat, re-register before TTL,
    //    force reconnect after session max age.
    let connected_at = Instant::now();
    let mut heartbeat = time::interval(Duration::from_secs(30));
    heartbeat.tick().await; // skip first immediate tick

    // Re-register 30s before TTL expires.
    let re_register_interval = Duration::from_secs(registrar_ttl_secs.saturating_sub(30));
    let mut re_register_deadline = Box::pin(time::sleep(re_register_interval));

    // Force full reconnect after 1 hour to refresh the session.
    // The session TTL is typically ~589000s but rotating more frequently
    // keeps tokens and registrations fresh.
    let session_max_age = Duration::from_secs(3600);
    let mut session_deadline = Box::pin(time::sleep(session_max_age));

    // Stability threshold: reset backoff after 60s of successful connection.
    // We communicate this via the return value — the caller checks timing.
    let stability_threshold = Duration::from_secs(60);

    println!("Trouter connected. Listening for events... (Ctrl-C to stop)");

    let disconnect_reason = loop {
        tokio::select! {
            frame = ws.recv_frame() => {
                match frame {
                    Ok(Some(text)) => handle_frame(&text).await,
                    Ok(None) => {
                        break DisconnectReason::Error(anyhow::anyhow!("WebSocket closed by server"));
                    }
                    Err(e) => {
                        break DisconnectReason::Error(e.context("WebSocket recv error"));
                    }
                }
            }
            _ = heartbeat.tick() => {
                if let Err(e) = ws.send_text("2::").await {
                    break DisconnectReason::Error(e.context("Heartbeat send failed"));
                }
            }
            _ = &mut re_register_deadline => {
                tracing::info!("Re-registering with registrar (TTL refresh)");
                if let Some(ref reg_url) = session.registrar_url {
                    let http2 = http.clone();
                    let tok = skype_token_str.to_string();
                    let surl = session.surl.clone();
                    let reg = reg_url.clone();
                    tokio::spawn(async move {
                        if let Err(e) = registrar::register(&http2, &tok, &reg, &surl).await {
                            tracing::warn!("Re-registration failed: {:#}", e);
                        }
                    });
                }
                // Reset the timer for another cycle.
                re_register_deadline = Box::pin(time::sleep(re_register_interval));
            }
            _ = &mut session_deadline => {
                tracing::info!("Session max age reached (1h), forcing reconnect for fresh session");
                break DisconnectReason::Error(anyhow::anyhow!("Session max age reached"));
            }
            _ = tokio::signal::ctrl_c() => {
                println!("Shutting down...");
                break DisconnectReason::Shutdown;
            }
        }
    };

    // If we were connected long enough, signal stability so caller resets backoff.
    // We do this by returning Ok (the caller pattern-matches on it).
    if connected_at.elapsed() >= stability_threshold {
        // Reset backoff indirectly: caller sees Ok and resets.
        // But we still need to convey the reason.
        // Use Ok for both shutdown and stable-error cases.
        return Ok(disconnect_reason);
    }

    match disconnect_reason {
        DisconnectReason::Shutdown => Ok(DisconnectReason::Shutdown),
        DisconnectReason::Error(e) => Err(e),
    }
}

/// Handle an incoming socket.io frame.
async fn handle_frame(frame: &str) {
    // socket.io framing:
    // 1:: — handshake (handled above)
    // 2:: — heartbeat ping (server)
    // 3::: — ephemeral message
    // 5:X::{json} — event
    // 6:X+::{json} — ack event

    if frame.starts_with("2::") {
        tracing::debug!("Heartbeat ping from server");
        return;
    }

    if frame.starts_with("5:::") || frame.starts_with("5:") {
        // Socket.IO v1 event frame: 5:ACK_ID:ENDPOINT:JSON
        // Ack (6:ID::) is sent automatically by recv_frame() in websocket.rs.
        // Here we just extract the JSON payload after the `::` separator.
        let after_5 = &frame[2..]; // skip "5:"
        let json_str = after_5
            .find("::")
            .map(|pos| &after_5[pos + 2..])
            .filter(|s| s.starts_with('{'));

        if let Some(json_str) = json_str {
            let prefix = if frame.contains("SkypeSpacesWeb") {
                "[CALL-INFO]"
            } else {
                "[MSG]"
            };
            println!("{} Event: {}", prefix, json_str);
        } else {
            println!("Frame: {}", frame);
        }
        return;
    }

    if frame.starts_with("6:") {
        if let Some(json_start) = frame.find(":::{") {
            let json_str = &frame[json_start + 3..];
            println!("Ack: {}", json_str);
        } else {
            println!("Ack frame: {}", frame);
        }
        return;
    }

    println!("Frame: {}", frame);
}

