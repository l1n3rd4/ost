//! User profile endpoint (/me)

use anyhow::{Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;
use crate::domain::User;

#[derive(Debug, Deserialize)]
struct MeResponse {
    id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    mail: Option<String>,
}

/// Fetch the current user and return a domain [`User`].
///
/// Parses the raw `/me` Graph response straight into the domain model; a
/// missing `displayName` falls back to `"User"`, matching the previous
/// behavior.
pub async fn whoami_data(client: &TeamsClient) -> Result<User> {
    let resp = client.graph_get("/me").await?;
    let me: MeResponse = resp.json().await.context("Failed to parse /me response")?;

    Ok(User {
        id: me.id,
        display_name: me.display_name.unwrap_or_else(|| "User".to_string()),
        email: me.mail,
    })
}
