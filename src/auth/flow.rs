//! High-level auth flows: login, refresh, logout, status.
//!
//! These delegate token construction to [`super::client::build_client`] and
//! derived-token acquisition to [`super::acquire::acquire_all`], preserving the
//! original `oauth.rs` behavior:
//!   * INV1 — every branch leaves identical `Config` state.
//!   * INV3 — partial-failure semantics: per-token failures warn (and, when
//!     verbose, `eprintln!`) but never abort.
//!
//! `login` runs `acquire_all(verbose=true)` (eprintln warnings); `refresh`
//! runs `acquire_all(verbose=false)` (warn-only), matching each caller today.
//!
//! Consolidated-log note: the original `refresh()` emitted per-token
//! `tracing::info!("... acquired"/"refreshed")` lines inline. Those success
//! info logs are consolidated away here (acquire_all emits only warn-on-failure
//! logs); the failure `tracing::warn!` logs are preserved by `acquire_all`, and
//! the "Refreshing AAD token..." / "Token refresh complete" info logs are kept.

use anyhow::{Context, Result};
use oauth2::{RefreshToken, Scope, StandardDeviceAuthorizationResponse, TokenResponse};

use super::acquire::acquire_all;
use super::client::build_client;
use super::config::AuthConfig;
use super::TokenStore;
use crate::config::Config;

/// Refresh the AAD access token using a stored refresh_token, then
/// re-acquire the derived (skype/graph/ic3/recorder) tokens. Returns Ok(true)
/// if refresh succeeded, Ok(false) if there was no stored refresh token.
pub async fn refresh() -> Result<bool> {
    let mut config = Config::load()?;
    let refresh_token_str = match config.get_refresh_token() {
        Some(rt) => rt,
        None => return Ok(false),
    };

    let auth_config = AuthConfig::default();
    let client = build_client(&auth_config)?;

    tracing::info!("Refreshing AAD token...");

    let token_response = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token_str))
        .add_scope(Scope::new(
            "https://api.spaces.skype.com/.default".to_string(),
        ))
        .add_scope(Scope::new("offline_access".to_string()))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .context("Failed to refresh AAD token")?;

    config.set_access_token(
        token_response.access_token().secret().to_string(),
        token_response.expires_in().map(|d| d.as_secs()),
    );

    if let Some(new_rt) = token_response.refresh_token() {
        config.set_refresh_token(new_rt.secret().to_string());
    }

    // Re-acquire derived tokens (verbose=false → warn-only, no eprintln).
    let aad_token = token_response.access_token().secret();
    let _report = acquire_all(&client, aad_token, &mut config, false).await;

    config.save()?;
    tracing::info!("Token refresh complete");
    Ok(true)
}

/// Perform OAuth2 login flow.
pub async fn login(force: bool) -> Result<()> {
    {
        let config = Config::load()?;

        // Check for existing valid token
        if !force {
            if let Some(token) = config.get_access_token() {
                if !token.is_expired() {
                    // Check if any derived tokens are missing; if so, refresh to acquire them
                    let missing_tokens =
                        config.get_recorder_token().is_none() || config.get_ic3_token().is_none();
                    if missing_tokens && config.get_refresh_token().is_some() {
                        tracing::info!(
                            "AAD token valid but some derived tokens missing, refreshing..."
                        );
                        if let Ok(true) = refresh().await {
                            println!("Tokens refreshed (acquired missing derived tokens).");
                            return Ok(());
                        }
                    }
                    println!(
                        "Already logged in (AAD token valid). Use --force to re-authenticate."
                    );
                    return Ok(());
                }
                // Try refresh before falling through to device code
                if config.get_refresh_token().is_some() {
                    tracing::info!("AAD token expired, attempting refresh...");
                    match refresh().await {
                        Ok(true) => {
                            println!("Token refreshed successfully.");
                            return Ok(());
                        }
                        Ok(false) => {}
                        Err(e) => {
                            tracing::warn!("Refresh failed, falling back to device code: {:#}", e);
                        }
                    }
                }
            }
        }
    }

    let auth_config = AuthConfig::default();
    let client = build_client(&auth_config)?;

    // Use device code flow for CLI
    tracing::info!("Initiating device code flow...");

    let device_auth_response: StandardDeviceAuthorizationResponse = client
        .exchange_device_code()?
        .add_scope(Scope::new(
            "https://api.spaces.skype.com/.default".to_string(),
        ))
        .add_scope(Scope::new("offline_access".to_string()))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .context("Failed to request device code")?;

    let verification_url = device_auth_response.verification_uri().as_str();
    let user_code = device_auth_response.user_code().secret();

    println!();
    println!("To sign in, visit: {}", verification_url);
    println!("Enter code:        {}", user_code);
    println!();

    // Poll for token
    tracing::info!("Waiting for authentication...");

    let token_response = client
        .exchange_device_access_token(&device_auth_response)
        .request_async(oauth2::reqwest::async_http_client, tokio::time::sleep, None)
        .await
        .context("Failed to exchange device code for token")?;

    // Save AAD tokens (single load-mutate-save)
    let mut config = Config::load()?;
    config.set_access_token(
        token_response.access_token().secret().to_string(),
        token_response.expires_in().map(|d| d.as_secs()),
    );

    if let Some(refresh_token) = token_response.refresh_token() {
        config.set_refresh_token(refresh_token.secret().to_string());
    }

    // Acquire derived tokens (verbose=true → eprintln warnings, matching login).
    let report = acquire_all(
        &client,
        token_response.access_token().secret(),
        &mut config,
        true,
    )
    .await;

    config.save()?;
    if report.skype_ok && report.graph_ok && report.ic3_ok && report.recorder_ok {
        println!("Login successful.");
    } else {
        println!(
            "Login partially successful (missing: {}).",
            report.missing().join(", ")
        );
    }
    Ok(())
}

/// Clear stored credentials.
///
/// Superseded at the composition root (task 8.1), which performs logout through
/// `ConfigRepositoryPort`. Retained as part of the public auth surface.
#[allow(dead_code)]
pub async fn logout() -> Result<()> {
    let mut config = Config::load()?;
    config.clear_tokens();
    config.save()?;
    println!("Logged out.");
    Ok(())
}

/// Display current auth status
pub async fn status() -> Result<()> {
    let config = Config::load()?;

    // AAD token status
    match config.get_access_token() {
        Some(token) if !token.is_expired() => {
            println!("AAD token:   valid");
            if let Some(exp) = token.expires_at {
                println!("  expires_at: {}", exp);
            }
        }
        Some(_) => {
            println!("AAD token:   expired");
        }
        None => {
            println!("AAD token:   none");
        }
    }

    // Refresh token
    match config.get_refresh_token() {
        Some(_) => println!("Refresh tok: present"),
        None => println!("Refresh tok: none"),
    }

    // Graph token status
    match config.get_graph_token() {
        Some(token) if !token.is_expired() => {
            println!("Graph token: valid");
            if let Some(exp) = token.expires_at {
                println!("  expires_at: {}", exp);
            }
        }
        Some(_) => {
            println!("Graph token: expired");
        }
        None => {
            println!("Graph token: none");
        }
    }

    // IC3 token status
    match config.get_ic3_token() {
        Some(token) if !token.is_expired() => {
            println!("IC3 token:   valid");
            if let Some(exp) = token.expires_at {
                println!("  expires_at: {}", exp);
            }
        }
        Some(_) => {
            println!("IC3 token:   expired");
        }
        None => {
            println!("IC3 token:   none");
        }
    }

    // Recorder token status
    match config.get_recorder_token() {
        Some(token) if !token.is_expired() => {
            println!("Recorder tk: valid");
            if let Some(exp) = token.expires_at {
                println!("  expires_at: {}", exp);
            }
        }
        Some(_) => {
            println!("Recorder tk: expired");
        }
        None => {
            println!("Recorder tk: none");
        }
    }

    // Skype token status
    match config.get_skype_token() {
        Some(token) if !token.is_expired() => {
            println!("Skype token: valid");
            if let Some(exp) = token.expires_at {
                println!("  expires_at: {}", exp);
            }
        }
        Some(_) => {
            println!("Skype token: expired");
        }
        None => {
            println!("Skype token: none");
        }
    }

    // Region GTMs
    if config.region_gtms.is_some() {
        println!("Region GTMs: present");
    } else {
        println!("Region GTMs: none");
    }

    if config.get_access_token().is_none() {
        println!("\nRun 'teams-cli login' to authenticate.");
    }

    Ok(())
}
