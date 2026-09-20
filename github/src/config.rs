use nasiko_utils::required_env;

/// Configuration for the GitHub OAuth integration.
///
/// Load with [`GitHubConfig::from_env`] at server startup.  All secret fields
/// must never be logged.
#[derive(Clone)]
pub struct GitHubConfig {
    /// GitHub OAuth App `client_id`.
    pub client_id: String,

    /// GitHub OAuth App `client_secret`.  Never log this.
    pub client_secret: String,

    /// Callback URL registered with the GitHub OAuth App.
    /// Must match exactly — e.g. `https://api.example.com/api/github/callback`.
    pub callback_url: String,

    /// HMAC-SHA256 signing key for the OAuth state blob.
    /// Priority: `OAUTH_STATE_SIGNING_KEY` → `GITHUB_CLIENT_SECRET`.
    /// Never log this.
    pub oauth_state_secret: String,

    /// When set, all OAuth consents use this central URL as `redirect_uri` and
    /// the per-tenant callback URL is embedded inside the signed state (for
    /// multi-tenant routing).  Maps to `GITHUB_CENTRAL_CALLBACK_URL`.
    pub central_callback_url: Option<String>,

    /// Maximum seconds a shallow clone may run before being killed.
    /// Maps to `GITHUB_CLONE_TIMEOUT_SECS` (default 300).
    pub clone_timeout_secs: u64,

    /// Maximum cloned repository size in bytes before the archive is rejected.
    /// Maps to `GITHUB_CLONE_MAX_SIZE_MB` (default 500), converted to bytes.
    pub clone_max_size_bytes: u64,
}

impl GitHubConfig {
    /// Build from environment variables.
    ///
    /// Required: `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`, `GITHUB_CALLBACK_URL`.
    /// Optional: `OAUTH_STATE_SIGNING_KEY`, `GITHUB_CENTRAL_CALLBACK_URL`,
    ///           `GITHUB_CLONE_TIMEOUT_SECS`, `GITHUB_CLONE_MAX_SIZE_MB`.
    pub fn from_env() -> anyhow::Result<Self> {
        let client_secret = required_env("GITHUB_CLIENT_SECRET")?;
        let oauth_state_secret =
            std::env::var("OAUTH_STATE_SIGNING_KEY").unwrap_or_else(|_| client_secret.clone());

        let clone_timeout_secs: u64 = nasiko_utils::env_parse("GITHUB_CLONE_TIMEOUT_SECS", 300u64);
        let clone_max_size_mb: u64 = nasiko_utils::env_parse("GITHUB_CLONE_MAX_SIZE_MB", 500u64);

        Ok(Self {
            client_id: required_env("GITHUB_CLIENT_ID")?,
            client_secret,
            callback_url: required_env("GITHUB_CALLBACK_URL")?,
            oauth_state_secret,
            central_callback_url: std::env::var("GITHUB_CENTRAL_CALLBACK_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            clone_timeout_secs,
            clone_max_size_bytes: clone_max_size_mb * 1024 * 1024,
        })
    }
}
