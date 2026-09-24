//! OAuth2 client construction and generic refresh-token exchange.
//!
//! Provides the shared `BasicClient` builder and a scope-parameterized
//! refresh-token exchange used to acquire per-audience access tokens.

use anyhow::{Context, Result};
use oauth2::{
    basic::BasicClient, AuthUrl, ClientId, DeviceAuthorizationUrl, RefreshToken, Scope,
    TokenResponse, TokenUrl,
};

use super::config::AuthConfig;

/// Build the OAuth2 client from an AuthConfig.
pub fn build_client(cfg: &AuthConfig) -> Result<BasicClient> {
    let auth_url = AuthUrl::new(format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/authorize",
        cfg.tenant
    ))?;
    let token_url = TokenUrl::new(format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
        cfg.tenant
    ))?;
    let device_url = DeviceAuthorizationUrl::new(format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/devicecode",
        cfg.tenant
    ))?;

    Ok(BasicClient::new(
        ClientId::new(cfg.client_id.to_string()),
        None,
        auth_url,
        Some(token_url),
    )
    .set_device_authorization_url(device_url))
}

/// Exchange a refresh token for an access token scoped to `scope`.
///
/// Adds the requested `scope` plus `offline_access` and returns the access
/// token secret along with its expiry (seconds), if provided.
pub async fn exchange_refresh_for_scope(
    client: &BasicClient,
    refresh_token: &str,
    scope: &str,
) -> Result<(String, Option<u64>)> {
    let token_response = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
        .add_scope(Scope::new(scope.to_string()))
        .add_scope(Scope::new("offline_access".to_string()))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .context("Failed to acquire token")?;

    Ok((
        token_response.access_token().secret().to_string(),
        token_response.expires_in().map(|d| d.as_secs()),
    ))
}
