//! HTTP adapter implementing [`TeamsApiPort`] and [`GraphPort`] over `reqwest`.
//!
//! [`ReqwestTeamsApi`] wraps the existing [`TeamsClient`] (which itself wraps
//! `reqwest::Client`) and the `api::*_data` fetch/parse helpers. It fetches
//! infra-shaped `*Info` structs, then converts them into domain models
//! ([`Chat`], [`Message`], [`User`], [`Presence`], [`Team`], [`SentMessage`])
//! entirely inside this adapter. No `reqwest`/`serde_json`/`anyhow` type
//! appears in any public signature; every method returns [`DomainResult`].
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
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::api::client::{HttpStatusError, TeamsClient};
use crate::api::{
    get_presence_data, list_chats_data, list_teams_data, read_messages_data,
    send_message_with_client, whoami_data, ChatInfo, MessageInfo, PresenceInfo, TeamInfo, UserInfo,
};
use crate::domain::{
    Chat, ChatId, DomainError, DomainResult, Message, MessagePreview, Presence, SentMessage, Team,
    User,
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

    /// Build the adapter from an already-constructed [`TeamsClient`].
    pub fn from_client(client: TeamsClient) -> Self {
        Self { client }
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

/// Best-effort parse of an API timestamp string into `DateTime<Utc>`.
///
/// Returns `None` on any parse failure (RFC 3339 is tried); a timestamp miss
/// never fails the surrounding operation.
fn parse_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(trimmed)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

/// Convert a fetched [`ChatInfo`] into a domain [`Chat`].
///
/// An empty preview (no sender/text) yields `last_message = None`.
fn chat_from_info(info: ChatInfo) -> Chat {
    let last_message = match (&info.last_message_sender, &info.last_message_preview) {
        (None, None) => None,
        (sender, preview) => Some(MessagePreview {
            sender: sender.clone().unwrap_or_default(),
            timestamp: info
                .last_message_time
                .as_deref()
                .and_then(parse_timestamp),
            text: preview.clone().unwrap_or_default(),
        }),
    };

    Chat {
        // ChatInfo ids come from the API and are non-empty (empties are
        // filtered upstream); on the off chance one slips through, fall back to
        // an unvalidated ChatId rather than dropping the chat.
        id: ChatId::new(info.id.clone()).unwrap_or(ChatId(info.id)),
        name: info.name,
        is_group: info.is_group,
        last_message,
    }
}

/// Convert a fetched [`MessageInfo`] into a domain [`Message`].
fn message_from_info(info: MessageInfo) -> Message {
    Message {
        sender: info.sender,
        timestamp: parse_timestamp(&info.timestamp),
        content: info.content,
    }
}

/// Convert a fetched [`UserInfo`] into a domain [`User`].
fn user_from_info(info: UserInfo) -> User {
    User {
        id: info.id,
        display_name: info.display_name,
        email: info.mail,
    }
}

/// Convert a fetched [`TeamInfo`] into a domain [`Team`].
fn team_from_info(info: TeamInfo) -> Team {
    Team {
        id: info.id,
        display_name: info.name,
        description: None,
    }
}

/// Map a Teams `availability` string into a domain [`Presence`].
///
/// Recognized values map to the fixed variants; anything else becomes
/// [`Presence::Custom`] carrying the original string.
fn presence_from_info(info: PresenceInfo) -> Presence {
    match info.availability.as_str() {
        "Available" => Presence::Available,
        "Busy" => Presence::Busy,
        "Away" | "BeRightBack" => Presence::Away,
        "DoNotDisturb" => Presence::DoNotDisturb,
        "Offline" | "PresenceUnknown" => Presence::Offline,
        other => Presence::Custom(other.to_string()),
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
        let infos = list_chats_data(&self.client, limit)
            .await
            .map_err(map_err)?;
        Ok(infos.into_iter().map(chat_from_info).collect())
    }

    async fn read_messages(&self, chat_id: &ChatId, limit: usize) -> DomainResult<Vec<Message>> {
        let infos = read_messages_data(&self.client, chat_id.as_str(), limit)
            .await
            .map_err(map_err)?;
        Ok(infos.into_iter().map(message_from_info).collect())
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
        let info = get_presence_data(&self.client).await.map_err(map_err)?;
        Ok(presence_from_info(info))
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
        let info = whoami_data(&self.client).await.map_err(map_err)?;
        Ok(user_from_info(info))
    }

    async fn list_teams(&self) -> DomainResult<Vec<Team>> {
        let infos = list_teams_data(&self.client).await.map_err(map_err)?;
        Ok(infos.into_iter().map(team_from_info).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_timestamp_accepts_rfc3339() {
        let dt = parse_timestamp("2024-05-01T12:30:00Z");
        assert!(dt.is_some());
    }

    #[test]
    fn parse_timestamp_returns_none_on_garbage() {
        assert!(parse_timestamp("not-a-date").is_none());
        assert!(parse_timestamp("").is_none());
        assert!(parse_timestamp("   ").is_none());
    }

    #[test]
    fn presence_maps_known_and_unknown() {
        assert_eq!(
            presence_from_info(PresenceInfo {
                availability: "Available".into(),
                activity: "Available".into(),
            }),
            Presence::Available
        );
        assert_eq!(
            presence_from_info(PresenceInfo {
                availability: "DoNotDisturb".into(),
                activity: "Presenting".into(),
            }),
            Presence::DoNotDisturb
        );
        assert_eq!(
            presence_from_info(PresenceInfo {
                availability: "Something".into(),
                activity: "X".into(),
            }),
            Presence::Custom("Something".into())
        );
    }

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
    fn chat_from_info_without_preview_has_no_last_message() {
        let info = ChatInfo {
            id: "19:abc@thread.v2".into(),
            name: "Team Chat".into(),
            is_group: true,
            last_message_time: None,
            last_message_sender: None,
            last_message_preview: None,
        };
        let chat = chat_from_info(info);
        assert_eq!(chat.id.as_str(), "19:abc@thread.v2");
        assert!(chat.last_message.is_none());
        assert!(chat.is_group);
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

    // ---- Task 5.3: HTTP → domain mapping ----
    //
    // The `conversations` JSON → `ChatInfo` half is covered in-crate by
    // `api::chat`'s test module (it owns the private serde parse types). Here we
    // cover the second half of that path — `ChatInfo` → domain `Chat` via
    // `chat_from_info` — using `ChatInfo` values shaped exactly like the ones
    // `list_chats_data` produces from that recorded fixture (Requirement 7.2).
    // The 401/404 error-mapping tests below (shared with task 5.2) cover
    // Requirements 7.3, 12.3, 12.4; task 5.3 relies on them for the error half.

    /// A group thread `ChatInfo` (topic name, HTML-stripped preview) converts to
    /// a domain `Chat`: id becomes a `ChatId`, `is_group` is preserved, and the
    /// preview populates `last_message`.
    #[test]
    fn chat_from_info_maps_group_conversation_to_domain_chat() {
        let info = ChatInfo {
            id: "19:abcThread@thread.v2".into(),
            name: "Project Phoenix".into(),
            is_group: true,
            last_message_time: Some("2024-05-01T12:30:00Z".into()),
            last_message_sender: Some("Alice Smith".into()),
            // Preview as list_chats_data would deliver it: HTML already stripped.
            last_message_preview: Some("Hello team & welcome!".into()),
        };
        let chat = chat_from_info(info);

        assert_eq!(chat.id.as_str(), "19:abcThread@thread.v2");
        assert_eq!(chat.name, "Project Phoenix");
        assert!(chat.is_group);

        let preview = chat.last_message.expect("group chat has a last message");
        assert_eq!(preview.sender, "Alice Smith");
        assert_eq!(preview.text, "Hello team & welcome!");
        assert!(preview.timestamp.is_some());
    }

    /// A 1:1 `ChatInfo` (sender-derived name, plain preview) converts to a
    /// non-group domain `Chat` carrying the sender's preview.
    #[test]
    fn chat_from_info_maps_direct_conversation_to_domain_chat() {
        let info = ChatInfo {
            id: "19:oneonone@unq.gbl.spaces".into(),
            name: "Bob Jones".into(),
            is_group: false,
            last_message_time: Some("2024-05-02T09:00:00Z".into()),
            last_message_sender: Some("Bob Jones".into()),
            last_message_preview: Some("hi there".into()),
        };
        let chat = chat_from_info(info);

        assert_eq!(chat.id.as_str(), "19:oneonone@unq.gbl.spaces");
        assert_eq!(chat.name, "Bob Jones");
        assert!(!chat.is_group);

        let preview = chat.last_message.expect("direct chat has a last message");
        assert_eq!(preview.sender, "Bob Jones");
        assert_eq!(preview.text, "hi there");
    }

    /// The full fixture path: mapping the two id-bearing `ChatInfo` values (the
    /// id-less conversation is dropped upstream in `list_chats_data`) yields a
    /// `Vec<Chat>` of length 2 in order.
    #[test]
    fn chat_infos_map_to_vec_chat() {
        let infos = vec![
            ChatInfo {
                id: "19:abcThread@thread.v2".into(),
                name: "Project Phoenix".into(),
                is_group: true,
                last_message_time: Some("2024-05-01T12:30:00Z".into()),
                last_message_sender: Some("Alice Smith".into()),
                last_message_preview: Some("Hello team & welcome!".into()),
            },
            ChatInfo {
                id: "19:oneonone@unq.gbl.spaces".into(),
                name: "Bob Jones".into(),
                is_group: false,
                last_message_time: Some("2024-05-02T09:00:00Z".into()),
                last_message_sender: Some("Bob Jones".into()),
                last_message_preview: Some("hi there".into()),
            },
        ];

        let chats: Vec<Chat> = infos.into_iter().map(chat_from_info).collect();

        assert_eq!(chats.len(), 2);
        assert_eq!(chats[0].id.as_str(), "19:abcThread@thread.v2");
        assert!(chats[0].is_group);
        assert_eq!(chats[1].id.as_str(), "19:oneonone@unq.gbl.spaces");
        assert!(!chats[1].is_group);
    }

    #[test]
    fn message_from_info_parses_timestamp_best_effort() {
        let msg = message_from_info(MessageInfo {
            sender: "Alice".into(),
            timestamp: "2024-05-01T12:30:00Z".into(),
            content: "hi".into(),
        });
        assert_eq!(msg.sender, "Alice");
        assert!(msg.timestamp.is_some());

        let msg2 = message_from_info(MessageInfo {
            sender: "Bob".into(),
            timestamp: "".into(),
            content: "yo".into(),
        });
        assert!(msg2.timestamp.is_none());
    }
}
