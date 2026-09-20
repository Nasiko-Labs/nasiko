use serde::{Deserialize, Serialize};

// ── OAuth state ────────────────────────────────────────────────────────────

/// Decoded and verified claims from a GitHub OAuth `state` blob.
/// Returned by [`crate::service::GitHubService::verify_state`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthStateClaims {
    pub user_id: String,
    /// Unix timestamp (seconds) at which the state was issued.
    pub issued_at: u64,
    /// OAuth flow that initiated this state: `"connect"` (link GitHub to an
    /// existing authenticated user) or `"login"` (sign in via GitHub SSO).
    pub flow: Option<String>,
}

// ── Token exchange ─────────────────────────────────────────────────────────

/// Successful token-exchange response from GitHub.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessToken {
    pub access_token: String,
    pub token_type: String,
    #[serde(default)]
    pub scope: Option<String>,
}

/// Internal union for `POST /login/oauth/access_token`.
///
/// GitHub returns 200 in both success and error cases — detect by field presence.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum GitHubTokenResponse {
    Error {
        error: String,
        #[serde(default)]
        error_description: Option<String>,
    },
    Token(AccessToken),
}

// ── GitHub user ────────────────────────────────────────────────────────────

/// GitHub user profile returned by `GET https://api.github.com/user`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubUser {
    pub id: u64,
    pub login: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    pub avatar_url: Option<String>,
}

/// One entry from `GET https://api.github.com/user/emails`.
///
/// The `/user` profile `email` above is the user's *public* address — often
/// null and never a verification signal. The account's real, confirmed address
/// comes from this endpoint (scope `user:email`, already requested): the caller
/// takes the one that is both `primary` and `verified`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubEmail {
    pub email: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub verified: bool,
}

// ── Repository ────────────────────────────────────────────────────────────

/// A single GitHub repository returned by `GET /user/repos`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubRepo {
    pub id: u64,
    pub name: String,
    pub full_name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub private: bool,
    pub clone_url: String,
    pub ssh_url: String,
    pub html_url: String,
    pub default_branch: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

// ── Clone archive ──────────────────────────────────────────────────────────

/// Result of [`crate::service::GitHubService::clone_to_archive`].
///
/// The caller is responsible for uploading `archive_bytes` to object storage
/// (MinIO / S3) at `s3_key` and triggering the runtime deploy.
#[derive(Debug)]
pub struct CloneArchive {
    /// In-memory `tar.gz` of the cloned repository (`.git` directory stripped).
    pub archive_bytes: Vec<u8>,
    /// Suggested S3 key: `github/{repo_full_name}/{branch}.tar.gz`.
    pub s3_key: String,
}

// ── Route DTOs ─────────────────────────────────────────────────────────────

/// Request body for `POST /api/github/clone`.
#[derive(Debug, Deserialize)]
pub struct CloneRequest {
    /// `owner/repo` identifier, e.g. `"acme/my-agent"`.
    pub repo_full_name: String,
    /// Branch to clone.  Defaults to `"main"` if absent.
    pub branch: Option<String>,
    /// Optional agent name for the registry entry (caller decides how to use).
    pub agent_name: Option<String>,
}

/// Response body for `POST /api/github/clone`.
#[derive(Debug, Serialize)]
pub struct CloneResponse {
    pub s3_key: String,
    pub repo_full_name: String,
    pub branch: String,
    /// Size of the produced archive in bytes.
    pub archive_size_bytes: usize,
}

/// Response body for `GET /api/github/repos`.
#[derive(Debug, Serialize)]
pub struct ReposResponse {
    pub repositories: Vec<GitHubRepo>,
    pub total: usize,
}

/// Response body for `GET /api/github/callback`.
#[derive(Debug, Serialize)]
pub struct CallbackResponse {
    /// Raw OAuth access token.  The caller must encrypt and persist this.
    pub access_token: String,
    pub token_type: String,
    pub user: GitHubUser,
}

/// Response body for `GET /api/github/status`.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub connected: bool,
    pub valid: bool,
}
