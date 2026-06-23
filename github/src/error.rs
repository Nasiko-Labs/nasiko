use thiserror::Error;

/// All errors produced by this crate.
///
/// `#[non_exhaustive]` ensures that adding future variants is not a breaking
/// change for callers who match on this enum.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum Error {
    // ── transport ─────────────────────────────────────────────────────────
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    // ── authentication ────────────────────────────────────────────────────
    /// 401 or 403 from any upstream — token invalid / insufficient scope.
    #[error("authentication failed: {0}")]
    Auth(String),

    // ── GitHub OAuth ──────────────────────────────────────────────────────
    /// The OAuth `state` parameter is missing, expired, tampered, or malformed.
    #[error("invalid OAuth state: {0}")]
    InvalidOAuthState(String),

    /// The authorization-code exchange with GitHub failed (network error or
    /// GitHub returned an `error` field in a 200 response body).
    #[error("GitHub OAuth error: {0}")]
    GitHubOAuth(String),

    /// A GitHub REST API call returned a non-2xx, non-auth status.
    #[error("GitHub API error {status}: {message}")]
    GitHubApi { status: u16, message: String },

    // ── git clone ─────────────────────────────────────────────────────────
    /// `git clone` subprocess returned a non-zero exit code, timed out, or
    /// the resulting directory exceeded the size cap.
    /// The message is scrubbed of OAuth tokens.
    #[error("git clone failed: {0}")]
    GitClone(String),

    // ── generic non-domain HTTP failures ─────────────────────────────────
    /// An upstream returned an unexpected non-2xx status that does not map to
    /// a domain variant.
    #[error("HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },

    // ── generic ───────────────────────────────────────────────────────────
    #[error("not found: {0}")]
    NotFound(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
