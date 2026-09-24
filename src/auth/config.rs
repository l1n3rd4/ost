//! Azure AD client configuration for Teams authentication.

/// Azure AD client configuration for Teams
///
/// `redirect_uri`/`scope` are part of the public config surface but not read by
/// the current `build_client` path; retained for completeness/external use.
#[allow(dead_code)]
pub struct AuthConfig {
    /// OAuth2 client ID (public client)
    pub client_id: &'static str,
    /// OAuth2 redirect URI
    pub redirect_uri: &'static str,
    /// Azure AD tenant (common for multi-tenant)
    pub tenant: &'static str,
    /// Primary resource scope
    pub scope: &'static str,
}

impl AuthConfig {
    /// Config for work/school accounts (Teams desktop client_id)
    pub fn work() -> Self {
        Self {
            client_id: "1fec8e78-bce4-4aaf-ab1b-5451cc387264",
            redirect_uri: "https://login.microsoftonline.com/common/oauth2/nativeclient",
            tenant: "common",
            scope: "https://api.spaces.skype.com/.default offline_access",
        }
    }

    /// Config for personal (consumer) accounts
    #[allow(dead_code)]
    pub fn personal() -> Self {
        Self {
            client_id: "8ec6bc83-69c8-4392-8f08-b3c986009232",
            redirect_uri: "https://login.microsoftonline.com/common/oauth2/nativeclient",
            tenant: "consumers",
            scope: "https://api.spaces.skype.com/.default offline_access",
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self::work()
    }
}
