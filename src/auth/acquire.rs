//! Derived-token acquisition.
//!
//! Collapses the previously-duplicated per-audience token acquisitions
//! (skype/graph/ic3/recorder) into a single [`acquire_all`] entry point.
//! Graph/IC3/Recorder are obtained by exchanging the stored refresh token
//! for a scope via [`exchange_refresh_for_scope`]; the Skype token is
//! obtained separately from the AAD access token via [`exchange_skype_token`].
//!
//! Preserves INV3 partial-failure semantics: each acquisition failure logs a
//! `tracing::warn!` (and, when `verbose`, an `eprintln!`), never aborts, and
//! leaves the corresponding `*_ok` flag `false`.

use oauth2::basic::BasicClient;

use super::client::exchange_refresh_for_scope;
use super::skype::exchange_skype_token;
use super::TokenStore;
use crate::config::Config;

/// A derived token audience.
pub enum DerivedToken {
    /// Never constructed: the Skype token is acquired directly via
    /// [`exchange_skype_token`], not through a refresh-scope exchange.
    #[allow(dead_code)]
    Skype,
    Graph,
    Ic3,
    Recorder,
}

impl DerivedToken {
    /// The OAuth2 scope for a refresh-token exchange.
    ///
    /// Only Graph/Ic3/Recorder are routed through
    /// [`exchange_refresh_for_scope`]; Skype is handled separately via
    /// [`exchange_skype_token`] and therefore has no refresh scope.
    fn scope(&self) -> &'static str {
        match self {
            DerivedToken::Graph => "https://graph.microsoft.com/.default",
            DerivedToken::Ic3 => "https://ic3.teams.office.com/.default",
            DerivedToken::Recorder => "4580fd1d-e5a3-4f56-9ad1-aab0e3bf8f76/.default",
            DerivedToken::Skype => {
                unreachable!("Skype token is acquired via exchange_skype_token, not a refresh scope")
            }
        }
    }
}

/// Which derived tokens were successfully acquired.
#[derive(Default)]
pub struct AcquireReport {
    pub skype_ok: bool,
    pub graph_ok: bool,
    pub ic3_ok: bool,
    pub recorder_ok: bool,
}

impl AcquireReport {
    /// Labels of the tokens that were NOT acquired, in skype/graph/ic3/recorder
    /// order. Matches the labels used in `login()`'s partial-success message.
    pub fn missing(&self) -> Vec<&'static str> {
        [
            (!self.skype_ok).then_some("Skype"),
            (!self.graph_ok).then_some("Graph"),
            (!self.ic3_ok).then_some("IC3"),
            (!self.recorder_ok).then_some("Recorder"),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// Acquire all derived tokens, mutating `config` in place.
///
/// Order matches `login()`/`refresh()` today: skype, then graph, ic3, recorder.
/// Skype is exchanged from `aad_token`; graph/ic3/recorder each re-read
/// `config.get_refresh_token()` (empty => skip, `*_ok` stays false). Each
/// failure logs a warning (and an `eprintln!` only when `verbose`) and never
/// aborts.
pub async fn acquire_all(
    client: &BasicClient,
    aad_token: &str,
    config: &mut Config,
    verbose: bool,
) -> AcquireReport {
    let mut report = AcquireReport::default();

    // Skype token (from the AAD access token, not a refresh scope exchange).
    match exchange_skype_token(aad_token, false).await {
        Ok((skype_tok, expires_in, region_gtms)) => {
            config.set_skype_token(skype_tok, expires_in);
            if let Some(gtms) = region_gtms {
                config.set_region_gtms(gtms);
            }
            report.skype_ok = true;
        }
        Err(e) => {
            tracing::warn!("Skype token exchange failed: {:#}", e);
            if verbose {
                eprintln!("Warning: Skype token exchange failed; some operations may not work.");
            }
        }
    }

    // Graph API token (separate audience from the Skype token).
    let rt = config.get_refresh_token().unwrap_or_default();
    if !rt.is_empty() {
        match exchange_refresh_for_scope(client, &rt, DerivedToken::Graph.scope()).await {
            Ok((graph_tok, expires_in)) => {
                config.set_graph_token(graph_tok, expires_in);
                report.graph_ok = true;
            }
            Err(e) => {
                tracing::warn!("Graph token acquisition failed: {:#}", e);
                if verbose {
                    eprintln!(
                        "Warning: Graph token acquisition failed; whoami/chats may not work."
                    );
                }
            }
        }
    }

    // IC3 token (for Trouter WebSocket auth).
    let rt = config.get_refresh_token().unwrap_or_default();
    if !rt.is_empty() {
        match exchange_refresh_for_scope(client, &rt, DerivedToken::Ic3.scope()).await {
            Ok((ic3_tok, expires_in)) => {
                config.set_ic3_token(ic3_tok, expires_in);
                report.ic3_ok = true;
            }
            Err(e) => {
                tracing::warn!("IC3 token acquisition failed: {:#}", e);
                if verbose {
                    eprintln!("Warning: IC3 token acquisition failed; trouter may not work.");
                }
            }
        }
    }

    // Recorder service token (for call recording).
    let rt = config.get_refresh_token().unwrap_or_default();
    if !rt.is_empty() {
        match exchange_refresh_for_scope(client, &rt, DerivedToken::Recorder.scope()).await {
            Ok((rec_tok, expires_in)) => {
                config.set_recorder_token(rec_tok, expires_in);
                report.recorder_ok = true;
            }
            Err(e) => {
                tracing::warn!("Recorder token acquisition failed: {:#}", e);
                if verbose {
                    eprintln!(
                        "Warning: Recorder token acquisition failed; recording may not work."
                    );
                }
            }
        }
    }

    report
}
