//! HTTP adapter implementing [`TeamsApiPort`] and [`GraphPort`] over `reqwest`.
//!
//! [`ReqwestTeamsApi`] wraps the existing [`TeamsClient`] (which itself wraps
//! `reqwest::Client`) and the `api::*_data` fetch/parse helpers. Those helpers
//! now parse the raw Teams/Graph responses straight into domain models
//! ([`Chat`], [`Message`], [`User`], [`Presence`], [`Team`]), so this adapter
//! simply forwards them; the only mapping it still owns is
//! infra-error → [`DomainError`] classification and the outbound presence-set
//! request. No `reqwest`/`serde_json`/`anyhow` type appears in any public
//! signature; every method returns [`DomainResult`].
//!
//! Infra failures are classified into the precise [`DomainError`] variant by
//! [`map_err`]: an HTTP `401` becomes [`DomainError::Unauthenticated`], `404`
//! becomes [`DomainError::NotFound`], and transport-level failures (timeout,
//! DNS, connection) or any other non-success status become
//! [`DomainError::Transport`]. The status is recovered by downcasting the
//! `anyhow` error to [`HttpStatusError`] (produced by
//! `TeamsClient::check_response`); no `reqwest::StatusCode` or `reqwest::Error`
//! type leaks through the port signatures.

use async_trait::async_trait;
use chrono::Utc;
use uuid::Uuid;

use crate::api::client::{HttpStatusError, TeamsClient};
use crate::api::{
    get_presence_data, list_chats_data, list_teams_data, read_messages_data,
    send_message_with_client, whoami_data,
};
use crate::domain::{
    Chat, ChatId, DomainError, DomainResult, Message, Presence, SentMessage, Team, User,
};
use crate::ports::{GraphPort, TeamsApiPort};

/// Concrete `reqwest`-backed adapter for the Teams and Graph ports.
///
/// Holds a [`TeamsClient`] internally; the `reqwest` dependency never escapes
/// through a public signature.
pub struct ReqwestTeamsApi {
    client: TeamsClient,
}

impl ReqwestTeamsApi {
    /// Build the adapter, constructing a [`TeamsClient`] (loads config and
    /// auto-refreshes tokens as needed).
    ///
    /// Any construction failure is classified via [`map_err`] (an HTTP status
    /// in the failure chain maps to the matching variant; otherwise
    /// [`DomainError::Transport`]).
    pub async fn new() -> DomainResult<Self> {
        let client = TeamsClient::new().await.map_err(map_err)?;
        Ok(Self { client })
    }

}

/// Classify an infra (`anyhow`) error into the precise [`DomainError`] variant.
///
/// If an [`HttpStatusError`] appears anywhere in the error chain, its numeric
/// status decides the variant: `401` → [`DomainError::Unauthenticated`],
/// `404` → [`DomainError::NotFound`], any other status → [`DomainError::Transport`].
/// Errors with no HTTP status (send failures: timeout, DNS, connection reset)
/// map to [`DomainError::Transport`]. In every case the message is preserved
/// and no `reqwest`/status type escapes.
fn map_err(err: anyhow::Error) -> DomainError {
    if let Some(http) = err.downcast_ref::<HttpStatusError>() {
        return status_to_domain(http.status, &err);
    }
    DomainError::Transport(format!("{:#}", err))
}

/// Map a raw HTTP status code to a [`DomainError`], using `err` for the message.
fn status_to_domain(status: u16, err: &anyhow::Error) -> DomainError {
    let msg = format!("{:#}", err);
    match status {
        401 => DomainError::Unauthenticated(msg),
        404 => DomainError::NotFound(msg),
        _ => DomainError::Transport(msg),
    }
}

/// Map a domain [`Presence`] to the status string understood by the Teams
/// presence API (the same vocabulary as `api::set_presence`).
fn presence_status_str(presence: &Presence) -> String {
    match presence {
        Presence::Available => "available".to_string(),
        Presence::Busy => "busy".to_string(),
        Presence::Away => "away".to_string(),
        Presence::DoNotDisturb => "dnd".to_string(),
        Presence::Offline => "offline".to_string(),
        Presence::Custom(s) => s.clone(),
    }
}

#[async_trait]
impl TeamsApiPort for ReqwestTeamsApi {
    async fn list_chats(&self, limit: usize) -> DomainResult<Vec<Chat>> {
        list_chats_data(&self.client, limit).await.map_err(map_err)
    }

    async fn read_messages(&self, chat_id: &ChatId, limit: usize) -> DomainResult<Vec<Message>> {
        read_messages_data(&self.client, chat_id.as_str(), limit)
            .await
            .map_err(map_err)
    }

    async fn send_message(&self, chat_id: &ChatId, text: &str) -> DomainResult<SentMessage> {
        // Domain validation of `text` lives in the service layer (task 7.2);
        // here we just perform the send.
        send_message_with_client(&self.client, chat_id.as_str(), text)
            .await
            .map_err(map_err)?;

        // The native send endpoint does not return a message id/timestamp we
        // parse here, so fabricate a client-side identity for the SentMessage.
        Ok(SentMessage {
            id: Uuid::new_v4().to_string(),
            chat_id: chat_id.clone(),
            timestamp: Some(Utc::now()),
        })
    }

    async fn get_presence(&self) -> DomainResult<Presence> {
        get_presence_data(&self.client).await.map_err(map_err)
    }

    async fn set_presence(&self, presence: &Presence) -> DomainResult<()> {
        // Reuse the same status vocabulary/mapping as api::set_presence, but
        // drive it through the adapter's own client (no stdout, no second
        // client construction).
        let status = presence_status_str(presence);
        let (availability, activity) = match status.to_lowercase().as_str() {
            "available" => ("Available", "Available"),
            "busy" => ("Busy", "InACall"),
            "dnd" | "donotdisturb" => ("DoNotDisturb", "Presenting"),
            "away" => ("Away", "Away"),
            "offline" => ("Offline", "OffWork"),
            other => {
                return Err(DomainError::Invalid(format!(
                    "unknown presence status: {}. Use: available, busy, dnd, away, offline",
                    other
                )))
            }
        };

        let body = serde_json::json!({
            "sessionId": "teams-cli",
            "availability": availability,
            "activity": activity,
            "expirationDuration": "PT1H"
        });

        self.client
            .graph_post("/me/presence/setUserPreferredPresence", &body)
            .await
            .map_err(map_err)?;
        Ok(())
    }
}

#[async_trait]
impl GraphPort for ReqwestTeamsApi {
    async fn whoami(&self) -> DomainResult<User> {
        whoami_data(&self.client).await.map_err(map_err)
    }

    async fn list_teams(&self) -> DomainResult<Vec<Team>> {
        list_teams_data(&self.client).await.map_err(map_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_status_str_roundtrips_known_variants() {
        assert_eq!(presence_status_str(&Presence::Available), "available");
        assert_eq!(presence_status_str(&Presence::Busy), "busy");
        assert_eq!(presence_status_str(&Presence::Away), "away");
        assert_eq!(presence_status_str(&Presence::DoNotDisturb), "dnd");
        assert_eq!(presence_status_str(&Presence::Offline), "offline");
        assert_eq!(
            presence_status_str(&Presence::Custom("busy".into())),
            "busy"
        );
    }

    #[test]
    fn map_err_401_is_unauthenticated() {
        let err: anyhow::Error = HttpStatusError {
            status: 401,
            url: "https://example.com/a".into(),
            body: String::new(),
        }
        .into();
        assert!(matches!(map_err(err), DomainError::Unauthenticated(_)));
    }

    #[test]
    fn map_err_404_is_not_found() {
        let err: anyhow::Error = HttpStatusError {
            status: 404,
            url: "https://example.com/b".into(),
            body: "missing".into(),
        }
        .into();
        assert!(matches!(map_err(err), DomainError::NotFound(_)));
    }

    #[test]
    fn map_err_other_status_is_transport() {
        let err: anyhow::Error = HttpStatusError {
            status: 500,
            url: "https://example.com/c".into(),
            body: "boom".into(),
        }
        .into();
        assert!(matches!(map_err(err), DomainError::Transport(_)));
    }

    #[test]
    fn map_err_status_survives_added_context() {
        // A downstream `.context(...)` wraps the HttpStatusError; the classifier
        // must still find it in the chain via downcast.
        let err: anyhow::Error = anyhow::Error::from(HttpStatusError {
            status: 401,
            url: "https://example.com/d".into(),
            body: String::new(),
        })
        .context("Teams GET failed");
        assert!(matches!(map_err(err), DomainError::Unauthenticated(_)));
    }

    #[test]
    fn map_err_non_http_error_is_transport() {
        // No HTTP status in the chain (e.g. a send-level timeout/DNS failure)
        // falls back to Transport.
        let err = anyhow::anyhow!("connection timed out");
        assert!(matches!(map_err(err), DomainError::Transport(_)));
    }
}
