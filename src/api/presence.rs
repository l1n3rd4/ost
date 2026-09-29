//! Presence API for Microsoft Teams

use anyhow::{Context, Result};
use serde::Deserialize;

use super::client::TeamsClient;
use crate::domain::Presence;

#[derive(Debug, Deserialize)]
struct PresenceResponse {
    availability: String,
    #[allow(dead_code)]
    activity: String,
}

/// Map a Teams `availability` string into a domain [`Presence`].
///
/// Recognized values map to the fixed variants; anything else becomes
/// [`Presence::Custom`] carrying the original string.
fn presence_from_availability(availability: &str) -> Presence {
    match availability {
        "Available" => Presence::Available,
        "Busy" => Presence::Busy,
        "Away" | "BeRightBack" => Presence::Away,
        "DoNotDisturb" => Presence::DoNotDisturb,
        "Offline" | "PresenceUnknown" => Presence::Offline,
        other => Presence::Custom(other.to_string()),
    }
}

/// Fetch current presence and return a domain [`Presence`].
pub async fn get_presence_data(client: &TeamsClient) -> Result<Presence> {
    let resp = client.graph_get("/me/presence").await?;
    let presence: PresenceResponse = resp
        .json()
        .await
        .context("Failed to parse presence response")?;

    Ok(presence_from_availability(&presence.availability))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_maps_known_and_unknown() {
        assert_eq!(presence_from_availability("Available"), Presence::Available);
        assert_eq!(presence_from_availability("Busy"), Presence::Busy);
        assert_eq!(presence_from_availability("Away"), Presence::Away);
        assert_eq!(presence_from_availability("BeRightBack"), Presence::Away);
        assert_eq!(
            presence_from_availability("DoNotDisturb"),
            Presence::DoNotDisturb
        );
        assert_eq!(presence_from_availability("Offline"), Presence::Offline);
        assert_eq!(
            presence_from_availability("PresenceUnknown"),
            Presence::Offline
        );
        assert_eq!(
            presence_from_availability("Something"),
            Presence::Custom("Something".into())
        );
    }
}
