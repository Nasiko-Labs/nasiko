//! Axum route handlers for the GitHub integration.
//!
//! Enable with `features = ["routes"]`.
//!
//! ## Wiring into the server
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use nasiko_github::{GitHubConfig, GitHubService, routes::github_router};
//!
//! let cfg = GitHubConfig::from_env().expect("GitHub config");
//! let svc = Arc::new(GitHubService::new(cfg).expect("GitHub service"));
//! let app = Router::new()
//!     .nest("/api", github_router(svc))
//!     // apply require_auth middleware so X-User-Id is present
//!     .layer(middleware::from_fn_with_state(state.clone(), require_auth));
//! ```
//!
//! ## Authentication seam
//!
//! These handlers extract the calling user's identity from the **`X-User-Id`
//! header**, which the Nasiko gateway injects after validating the bearer
//! token.  This keeps the crate free of any dependency on `nasiko-server` types.
//!
//! ## Token persistence contract
//!
//! This crate contains no database access.  For routes that require a stored
//! token (`/status`, `/repos`, `/clone`), the caller must supply the
//! **decrypted** token via the **`X-GitHub-Token`** request header.  The
//! control plane is responsible for:
//! 1. Encrypting and persisting the token returned by `/callback`.
//! 2. Decrypting and injecting it as `X-GitHub-Token` for subsequent requests.
//! 3. Deleting the stored credential on `/logout`.

use std::sync::Arc;

use axum::{
    Json,
    Router,
    extract::{ Query, State },
    http::{ HeaderMap, StatusCode },
    response::{ IntoResponse, Redirect, Response },
    routing::{ delete, get, post },
};
use serde_json::json;
use tracing::warn;

use crate::{
    Error,
    models::{ CallbackResponse, CloneRequest, ReposResponse, StatusResponse },
    service::GitHubService,
};

// ── Router ────────────────────────────────────────────────────────────────────

/// Return an Axum `Router` for all GitHub endpoints.
///
/// Routes:
/// ```text
/// GET    /github/login    → start OAuth flow
/// GET    /github/callback → OAuth callback (exchange code, return token)
/// GET    /github/status   → verify a stored token (requires X-GitHub-Token)
/// DELETE /github/logout   → caller clears stored credential (returns 200)
/// GET    /github/repos    → list repos (requires X-GitHub-Token)
/// POST   /github/clone    → clone repo and return archive info (requires X-GitHub-Token)
/// ```
pub fn github_router(svc: Arc<GitHubService>) -> Router {
    Router::new()
        .route("/github/login", get(github_login))
        .route("/github/callback", get(github_callback))
        .route("/github/status", get(github_status))
        .route("/github/logout", delete(github_logout))
        .route("/github/repos", get(github_repos))
        .route("/github/clone", post(github_clone))
        .with_state(svc)
}

// ── Header extraction error ───────────────────────────────────────────────────

/// Errors produced by the header-extraction helpers.
///
/// This is a small enum (one pointer-width discriminant) rather than returning
/// `Result<_, Response>` directly, which would trigger `clippy::result_large_err`
/// because `axum::response::Response` is a large type.  The pattern mirrors
/// `ProxyError` in `oss/server/src/proxy/middleware.rs`.
#[derive(Debug)]
enum HeaderError {
    MissingUserId,
    MissingToken,
}

impl IntoResponse for HeaderError {
    fn into_response(self) -> Response {
        let msg = match self {
            Self::MissingUserId => "missing user identity",
            Self::MissingToken =>
                "missing X-GitHub-Token header — provide the decrypted OAuth token",
        };
        (StatusCode::UNAUTHORIZED, Json(json!({"error": msg}))).into_response()
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract the authenticated `user_id` from the `X-User-Id` header injected by
/// the gateway.  Returns `HeaderError::MissingUserId` (→ 401) if absent or
/// non-UTF-8.
fn extract_user_id(headers: &HeaderMap) -> Result<String, HeaderError> {
    headers
        .get("x-user-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or(HeaderError::MissingUserId)
}

/// Extract a stored GitHub OAuth token from the `X-GitHub-Token` header.
/// Returns `HeaderError::MissingToken` (→ 401) if absent or non-UTF-8.
fn extract_github_token(headers: &HeaderMap) -> Result<String, HeaderError> {
    headers
        .get("x-github-token")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or(HeaderError::MissingToken)
}

/// Map a crate [`Error`] to an HTTP response with an appropriate status code.
fn error_response(e: Error) -> Response {
    let (status, message) = match &e {
        Error::InvalidOAuthState(_) => (StatusCode::BAD_REQUEST, e.to_string()),
        Error::GitHubOAuth(_) => (StatusCode::BAD_REQUEST, e.to_string()),
        Error::Auth(_) => (StatusCode::UNAUTHORIZED, e.to_string()),
        Error::GitHubApi { status, message } =>
            (StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY), message.clone()),
        Error::GitClone(_) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
        Error::NotFound(_) => (StatusCode::NOT_FOUND, e.to_string()),
        Error::HttpStatus { status, body } =>
            (StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY), body.clone()),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    (status, Json(json!({"error": message}))).into_response()
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /github/login`
///
/// Start the GitHub OAuth connect flow.  Redirects the browser to GitHub's
/// consent page.  Requires `X-User-Id` to be present (set by gateway auth).
async fn github_login(State(svc): State<Arc<GitHubService>>, headers: HeaderMap) -> Response {
    let user_id = match extract_user_id(&headers) {
        Ok(id) => id,
        Err(e) => {
            return e.into_response();
        }
    };
    match svc.authorization_url(&user_id) {
        Ok(url) => Redirect::temporary(&url).into_response(),
        Err(e) => {
            warn!(%e, "failed to build GitHub authorization URL");
            error_response(e)
        }
    }
}

/// Query parameters expected on the OAuth callback.
#[derive(serde::Deserialize)]
struct CallbackParams {
    code: String,
    state: String,
}

/// `GET /github/callback`
///
/// Receives the GitHub OAuth callback.  Verifies the `state`, exchanges the
/// `code` for an access token, and returns the token + user profile in the
/// response body.
///
/// **The caller is responsible for encrypting and persisting the
/// `access_token` keyed by `user_id`.**  A future request to `/status`,
/// `/repos`, or `/clone` must supply the decrypted token via
/// `X-GitHub-Token`.
async fn github_callback(
    State(svc): State<Arc<GitHubService>>,
    Query(params): Query<CallbackParams>
) -> Response {
    // Verify state — extracts user_id without needing the X-User-Id header
    // (the state was signed with user_id when the flow started).
    let claims = match svc.verify_state(&params.state) {
        Ok(c) => c,
        Err(e) => {
            return error_response(e);
        }
    };

    let (token, user) = match svc.exchange_code(&params.code).await {
        Ok(t) => t,
        Err(e) => {
            warn!(user_id = %claims.user_id, %e, "GitHub code exchange failed");
            return error_response(e);
        }
    };

    (
        StatusCode::OK,
        Json(CallbackResponse {
            access_token: token.access_token,
            token_type: token.token_type,
            user,
        }),
    ).into_response()
}

/// `GET /github/status`
///
/// Verifies whether the supplied token is still valid.
/// Requires `X-GitHub-Token` header with the decrypted OAuth token.
async fn github_status(State(svc): State<Arc<GitHubService>>, headers: HeaderMap) -> Response {
    let token = match extract_github_token(&headers) {
        Ok(t) => t,
        // If no token header, the user is simply not connected.
        Err(_) => {
            return (
                StatusCode::OK,
                Json(StatusResponse { connected: false, valid: false }),
            ).into_response();
        }
    };

    match svc.verify_token(&token).await {
        Ok(valid) =>
            (StatusCode::OK, Json(StatusResponse { connected: true, valid })).into_response(),
        Err(e) => {
            warn!(%e, "GitHub token verification failed");
            error_response(e)
        }
    }
}

/// `DELETE /github/logout`
///
/// Signals that the caller should delete the stored GitHub credential for the
/// authenticated user.  This crate holds no state — the `200` response
/// instructs the control plane to carry out the deletion.
async fn github_logout(headers: HeaderMap) -> Response {
    match extract_user_id(&headers) {
        Ok(_) =>
            (
                StatusCode::OK,
                Json(json!({"message": "GitHub credentials cleared"})),
            ).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /github/repos`
///
/// List the user's GitHub repositories (up to 100, sorted by last-updated).
/// Requires `X-GitHub-Token` header.
async fn github_repos(State(svc): State<Arc<GitHubService>>, headers: HeaderMap) -> Response {
    let token = match extract_github_token(&headers) {
        Ok(t) => t,
        Err(e) => {
            return e.into_response();
        }
    };

    match svc.list_repos(&token).await {
        Ok(repos) => {
            let total = repos.len();
            (StatusCode::OK, Json(ReposResponse { repositories: repos, total })).into_response()
        }
        Err(e) => {
            warn!(%e, "failed to list GitHub repositories");
            error_response(e)
        }
    }
}

/// `POST /github/clone`
///
/// Validates clone inputs and returns the suggested S3 key.
///
/// **Status: NOT_IMPLEMENTED** — input validation is complete but the MinIO
/// upload and runtime-deploy trigger are not yet wired into this crate (see
/// implementation plan §16.1).  The control plane must call
/// [`GitHubService::clone_to_archive`] directly, upload `archive_bytes` to
/// MinIO at `s3_key`, and trigger the runtime deploy.
///
/// This handler returns 422 on bad inputs and 501 on valid inputs until the
/// upload path is wired, so callers never receive a fake `s3_key` that
/// doesn't exist in object storage.
async fn github_clone(
    State(_svc): State<Arc<GitHubService>>,
    headers: HeaderMap,
    Json(body): Json<CloneRequest>,
) -> Response {
    // Auth check first — 401 before any validation.
    if let Err(e) = extract_github_token(&headers) {
        return e.into_response();
    }

    let branch = body.branch.as_deref().unwrap_or("main");

    // Validate repo and branch without performing any network or filesystem
    // operations.  Returns 422 on invalid inputs.
    if let Err(e) = GitHubService::validate_clone_request(&body.repo_full_name, branch) {
        return error_response(e);
    }

    // TODO (blocker §16.1): call `svc.clone_to_archive`, upload `archive_bytes`
    // to MinIO at the returned `s3_key`, trigger runtime deploy, then return
    // CloneResponse.  Until then, 501 prevents callers from acting on a
    // non-existent S3 key.
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": "MinIO upload not yet wired; use GitHubService::clone_to_archive directly",
            "suggested_s3_key": format!("github/{}/{branch}.tar.gz", body.repo_full_name),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests;
