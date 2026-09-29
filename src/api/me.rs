//! User profile endpoint (/me)

use anyhow::{Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;

#[derive(Debug, Deserialize)]
struct MeResponse {
    id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    mail: Option<String>,
}

// ---------------------------------------------------------------------------
// Data-returning API functions for TUI integration
// ---------------------------------------------------------------------------

/// User info for TUI display.
#[allow(dead_code)]
pub struct UserInfo {
    pub display_name: String,
    pub mail: Option<String>,
    pub id: String,
}

/// Fetch current user info and return structured data.
pub async fn whoami_data(client: &TeamsClient) -> Result<UserInfo> {
    let resp = client.graph_get("/me").await?;
    let me: MeResponse = resp.json().await.context("Failed to parse /me response")?;

    Ok(UserInfo {
        display_name: me.display_name.unwrap_or_else(|| "User".to_string()),
        mail: me.mail,
        id: me.id,
    })
}
