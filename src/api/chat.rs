//! Native Teams chat API (chatsvcagg / chat service)
//!
//! Uses the Skype token with `Authentication: skypetoken={token}` header,
//! bypassing Graph API which requires tenant admin consent for Chat.Read.

use anyhow::{Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;
use crate::domain::{html_escape, strip_html};

// -- Response types for the native chat API --

#[derive(Debug, Deserialize)]
struct ConversationsResponse {
    conversations: Option<Vec<Conversation>>,
}

#[derive(Debug, Deserialize)]
struct Conversation {
    id: Option<String>,
    #[serde(rename = "threadProperties")]
    thread_properties: Option<ThreadProperties>,
    #[serde(rename = "lastMessage")]
    last_message: Option<NativeMessage>,
}

#[derive(Debug, Deserialize)]
struct ThreadProperties {
    topic: Option<String>,
    #[serde(rename = "lastjoinat")]
    last_join_at: Option<String>,
    /// For 1:1 chats, contains member MRIs
    members: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NativeMessage {
    id: Option<String>,
    #[serde(rename = "composetime")]
    compose_time: Option<String>,
    #[serde(rename = "originalarrivaltime")]
    original_arrival_time: Option<String>,
    #[serde(rename = "imdisplayname")]
    im_display_name: Option<String>,
    content: Option<String>,
    messagetype: Option<String>,
    from: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    messages: Option<Vec<NativeMessage>>,
}

/// Display name for a conversation.
fn conversation_name(conv: &Conversation) -> String {
    if let Some(ref props) = conv.thread_properties {
        if let Some(ref topic) = props.topic {
            if !topic.is_empty() {
                return topic.clone();
            }
        }
    }
    // Fall back to last message sender or the thread ID
    if let Some(ref msg) = conv.last_message {
        if let Some(ref name) = msg.im_display_name {
            if !name.is_empty() {
                return name.clone();
            }
        }
    }
    conv.id.as_deref().unwrap_or("[unknown]").to_string()
}

/// List recent chats using the native Teams API (prints to stdout).
pub async fn list_chats(limit: usize) -> Result<()> {
    let client = TeamsClient::new().await?;
    let chats = list_chats_data(&client, limit).await?;

    println!("\nRecent Chats:");
    println!("{:-<60}", "");

    if chats.is_empty() {
        println!("  (no chats found)");
        return Ok(());
    }

    for chat in &chats {
        println!("{}", chat.name);
        println!("  ID: {}", chat.id);

        if let Some(ref time) = chat.last_message_time {
            println!("  Last: {}", time);
        }
        if let Some(ref preview) = chat.last_message_preview {
            if !preview.trim().is_empty() {
                let sender = chat.last_message_sender.as_deref().unwrap_or("?");
                println!("  [{}]: {}", sender, preview.trim());
            }
        }

        println!();
    }

    Ok(())
}

/// Read messages from a specific chat thread (prints to stdout).
pub async fn read_messages(chat_id: &str, limit: usize) -> Result<()> {
    let client = TeamsClient::new().await?;
    let msgs = read_messages_data(&client, chat_id, limit).await?;

    if msgs.is_empty() {
        println!("(no messages)");
        return Ok(());
    }

    for msg in &msgs {
        println!("[{}] {}: {}", msg.timestamp, msg.sender, msg.content);
    }

    Ok(())
}

/// Send a message to a chat thread using the native API.
pub async fn send_message(chat_id: &str, message: &str) -> Result<()> {
    let client = TeamsClient::new().await?;
    send_message_with_client(&client, chat_id, message).await?;
    println!("Message sent.");
    Ok(())
}

/// Send a message using an existing client (shared helper).
pub async fn send_message_with_client(
    client: &TeamsClient,
    chat_id: &str,
    message: &str,
) -> Result<()> {
    let base = client.chat_service_url();
    let url = format!("{}/v1/users/ME/conversations/{}/messages", base, chat_id);

    let escaped = html_escape(message);
    let body = serde_json::json!({
        "content": format!("<p>{}</p>", escaped),
        "messagetype": "RichText/Html",
        "contenttype": "text"
    });

    tracing::debug!("Sending message to {}", url);
    client.chat_post(&url, &body).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Data-returning API functions for TUI integration
// ---------------------------------------------------------------------------

/// Chat metadata for TUI display.
#[allow(dead_code)]
pub struct ChatInfo {
    pub id: String,
    pub name: String,
    pub is_group: bool,
    pub last_message_time: Option<String>,
    pub last_message_sender: Option<String>,
    pub last_message_preview: Option<String>,
}

/// A single message for TUI display.
pub struct MessageInfo {
    pub sender: String,
    pub timestamp: String,
    pub content: String,
}

/// List recent chats and return structured data.
pub async fn list_chats_data(client: &TeamsClient, limit: usize) -> Result<Vec<ChatInfo>> {
    // Strategy 1: CSA AFD endpoint with Bearer auth
    let csa_url = format!(
        "https://teams.microsoft.com/api/csa/api/v1/teams/users/ME/conversations?view=mychats&pageSize={}",
        limit
    );
    tracing::debug!("Trying CSA endpoint: {}", csa_url);
    let resp = match client.csa_get(&csa_url).await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("CSA endpoint failed: {:#}, trying chatsvcagg", e);
            // Strategy 2: chatsvcagg with skypetoken auth
            let base = client.chatsvcagg_url();
            let url = format!(
                "{}/api/v2/users/ME/conversations?view=mychats&pageSize={}",
                base, limit
            );
            tracing::debug!("Trying chatsvcagg: {}", url);
            match client.chat_get(&url).await {
                Ok(r) => r,
                Err(e2) => {
                    tracing::debug!("chatsvcagg failed: {:#}, trying chat service", e2);
                    // Strategy 3: chat service (amer.ng.msg) with skypetoken auth
                    let base = client.chat_service_url();
                    let url = format!(
                        "{}/v1/users/ME/conversations?view=mychats&pageSize={}",
                        base, limit
                    );
                    client.chat_get(&url).await?
                }
            }
        }
    };

    let body: ConversationsResponse = resp
        .json()
        .await
        .context("Failed to parse conversations response")?;

    let conversations = body.conversations.unwrap_or_default();

    let mut chats = Vec::new();
    for conv in &conversations {
        let id = conv.id.as_deref().unwrap_or("").to_string();
        if id.is_empty() {
            continue;
        }

        let name = conversation_name(conv);
        let is_group = id.contains("thread") || id.contains("meeting");

        let (last_time, last_sender, last_preview) = if let Some(ref msg) = conv.last_message {
            let time = msg
                .original_arrival_time
                .as_deref()
                .or(msg.compose_time.as_deref())
                .map(String::from);
            let sender = msg.im_display_name.clone();
            let preview = msg.content.as_deref().map(|c| {
                let text = strip_html(c);
                if text.len() > 80 {
                    let end = text
                        .char_indices()
                        .map(|(i, _)| i)
                        .take_while(|&i| i <= 77)
                        .last()
                        .unwrap_or(0);
                    format!("{}...", &text[..end])
                } else {
                    text
                }
            });
            (time, sender, preview)
        } else {
            (None, None, None)
        };

        chats.push(ChatInfo {
            id,
            name,
            is_group,
            last_message_time: last_time,
            last_message_sender: last_sender,
            last_message_preview: last_preview,
        });
    }

    Ok(chats)
}

/// Read messages from a specific chat thread and return structured data.
pub async fn read_messages_data(
    client: &TeamsClient,
    chat_id: &str,
    limit: usize,
) -> Result<Vec<MessageInfo>> {
    let base = client.chat_service_url();
    let url = format!(
        "{}/v1/users/ME/conversations/{}/messages?pageSize={}",
        base, chat_id, limit
    );

    tracing::debug!("Reading messages from {}", url);
    let resp = client.chat_get(&url).await?;
    let body: MessagesResponse = resp
        .json()
        .await
        .context("Failed to parse messages response")?;

    let messages = body.messages.unwrap_or_default();

    // Messages come newest-first; reverse for chronological display
    let mut msgs: Vec<&NativeMessage> = messages.iter().collect();
    msgs.reverse();

    let mut result = Vec::new();
    for msg in &msgs {
        let msgtype = msg.messagetype.as_deref().unwrap_or("");
        // Skip non-text messages (e.g. ThreadActivity/*)
        if !msgtype.contains("Text") && !msgtype.contains("RichText") {
            continue;
        }

        let sender = msg.im_display_name.as_deref().unwrap_or("?").to_string();
        let time = msg
            .original_arrival_time
            .as_deref()
            .or(msg.compose_time.as_deref())
            .unwrap_or("")
            .to_string();
        let content = msg.content.as_deref().unwrap_or("");
        let text = strip_html(content);

        if text.trim().is_empty() {
            continue;
        }

        result.push(MessageInfo {
            sender,
            timestamp: time,
            content: text.trim().to_string(),
        });
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    //! In-crate tests for the `conversations` JSON → chat parsing path.
    //!
    //! These tests support spec task 5.3 (HTTP→domain mapping). The serde types
    //! (`ConversationsResponse`, `Conversation`, ...) are private to this
    //! module, so a test outside `api::chat` cannot deserialize them. Deserializing
    //! a recorded fixture here verifies that the raw Teams `conversations`
    //! payload maps onto the fields `ChatInfo` is built from — id, display name
    //! (`threadProperties.topic` with sender fallback), and the HTML-stripped
    //! last-message preview — and that conversations without an id are skipped.
    //! No network: pure deserialization + the same pure conversion helpers used
    //! by `list_chats_data`. The `ChatInfo` → domain `Chat` half lives in the
    //! adapter's own test module (`adapters::http::reqwest_api`).

    use super::*;
    use crate::domain::strip_html;

    /// A recorded (trimmed) `conversations` response: one group thread with a
    /// topic and an HTML last message, one 1:1 chat whose name falls back to
    /// the sender display name, and one malformed conversation with no id that
    /// must be skipped.
    const CONVERSATIONS_JSON: &str = r#"{
        "conversations": [
            {
                "id": "19:abcThread@thread.v2",
                "threadProperties": { "topic": "Project Phoenix" },
                "lastMessage": {
                    "imdisplayname": "Alice Smith",
                    "content": "<p>Hello <b>team</b> &amp; welcome!</p>",
                    "originalarrivaltime": "2024-05-01T12:30:00Z",
                    "messagetype": "RichText/Html"
                }
            },
            {
                "id": "19:oneonone@unq.gbl.spaces",
                "threadProperties": {},
                "lastMessage": {
                    "imdisplayname": "Bob Jones",
                    "content": "hi there",
                    "composetime": "2024-05-02T09:00:00Z",
                    "messagetype": "Text"
                }
            },
            {
                "threadProperties": { "topic": "Ghost thread" },
                "lastMessage": { "imdisplayname": "Nobody", "content": "x" }
            }
        ]
    }"#;

    /// The recorded fixture deserializes into the private serde types and yields
    /// exactly the three conversations present in the payload.
    #[test]
    fn conversations_json_deserializes() {
        let parsed: ConversationsResponse =
            serde_json::from_str(CONVERSATIONS_JSON).expect("fixture must deserialize");
        let convs = parsed.conversations.expect("conversations field present");
        assert_eq!(convs.len(), 3);
    }

    /// The group thread maps to its `threadProperties.topic` name, and its HTML
    /// last message is stripped/entity-decoded for the preview.
    #[test]
    fn group_conversation_maps_topic_and_strips_html_preview() {
        let parsed: ConversationsResponse =
            serde_json::from_str(CONVERSATIONS_JSON).expect("fixture must deserialize");
        let convs = parsed.conversations.unwrap();
        let group = &convs[0];

        assert_eq!(conversation_name(group), "Project Phoenix");

        let raw = group
            .last_message
            .as_ref()
            .and_then(|m| m.content.as_deref())
            .expect("group has last-message content");
        // Same transform list_chats_data applies to build the preview.
        assert_eq!(strip_html(raw), "Hello team & welcome!");
    }

    /// A 1:1 chat with no topic falls back to the last-message sender display
    /// name, and a plain-text body passes through `strip_html` unchanged.
    #[test]
    fn direct_conversation_falls_back_to_sender_name() {
        let parsed: ConversationsResponse =
            serde_json::from_str(CONVERSATIONS_JSON).expect("fixture must deserialize");
        let convs = parsed.conversations.unwrap();
        let direct = &convs[1];

        assert_eq!(conversation_name(direct), "Bob Jones");

        let raw = direct
            .last_message
            .as_ref()
            .and_then(|m| m.content.as_deref())
            .unwrap();
        assert_eq!(strip_html(raw), "hi there");
    }

    /// Conversations without an id are skipped when building `ChatInfo`, exactly
    /// as `list_chats_data` filters them (`id.is_empty() => continue`). Running
    /// the same id-extraction + filter here proves the malformed third
    /// conversation contributes no chat.
    #[test]
    fn empty_id_conversation_is_skipped() {
        let parsed: ConversationsResponse =
            serde_json::from_str(CONVERSATIONS_JSON).expect("fixture must deserialize");
        let convs = parsed.conversations.unwrap();

        let kept: Vec<String> = convs
            .iter()
            .map(|c| c.id.as_deref().unwrap_or("").to_string())
            .filter(|id| !id.is_empty())
            .collect();

        assert_eq!(
            kept,
            vec![
                "19:abcThread@thread.v2".to_string(),
                "19:oneonone@unq.gbl.spaces".to_string(),
            ]
        );
        // The third (id-less) conversation is absent.
        assert_eq!(kept.len(), 2);
    }

    /// The group thread id contains `thread`, so it classifies as a group chat;
    /// the 1:1 id does not, so it is not a group — mirroring the `is_group`
    /// derivation in `list_chats_data`.
    #[test]
    fn is_group_derives_from_id_shape() {
        let group_id = "19:abcThread@thread.v2";
        let direct_id = "19:oneonone@unq.gbl.spaces";

        let is_group = |id: &str| id.contains("thread") || id.contains("meeting");
        assert!(is_group(group_id));
        assert!(!is_group(direct_id));
    }
}
