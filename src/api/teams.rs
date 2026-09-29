//! Microsoft Graph API: joined teams and channels

use anyhow::{Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;
use crate::domain::Team;

#[derive(Debug, Deserialize)]
struct TeamsResponse {
    value: Vec<RawTeam>,
}

#[derive(Debug, Deserialize)]
struct RawTeam {
    id: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

/// List joined teams and return domain [`Team`] models.
///
/// A missing `displayName` falls back to the team id, matching the previous
/// behavior.
pub async fn list_teams_data(client: &TeamsClient) -> Result<Vec<Team>> {
    tracing::debug!("Fetching joined teams...");
    let resp = client.graph_get("/me/joinedTeams").await?;
    let teams: TeamsResponse = resp
        .json()
        .await
        .context("Failed to parse joinedTeams response")?;

    let result = teams
        .value
        .into_iter()
        .map(|team| Team {
            display_name: team.display_name.unwrap_or_else(|| team.id.clone()),
            id: team.id,
            description: None,
        })
        .collect();

    Ok(result)
}
