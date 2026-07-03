use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::warn;
use uuid::Uuid;

use crate::{auth::Claims, secrets::crypto::SecretsCrypto, state::AppState};

/// Public routes — no auth required (GitHub redirects the browser here).
/// Merged at the root level in `lib.rs` so the callback URL is reachable
/// without a bearer token.
pub fn public_router() -> Router<AppState> {
    Router::new().route("/api/github/callback", get(github_callback))
}

/// Protected routes — served under /api/v1 with require_auth middleware.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/github/login", get(github_login))
        .route("/auth/github/login-user", get(github_login_user))
        .route("/auth/github/token", get(github_token))
        .route("/github/user", get(github_status))
        .route("/github/repositories", get(github_repos))
        .route("/github/logout", delete(github_logout))
        .route("/github/clone", post(github_clone))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub(crate) async fn load_github_token(db: &PgPool, user_id: Uuid) -> Option<String> {
    let row: Option<(serde_json::Value,)> = match sqlx::query_as(
        "SELECT provider_metadata FROM user_identities \
         WHERE user_id = $1 AND provider = 'github'",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(user_id = %user_id, %e, "DB error reading GitHub token");
            return None;
        }
    };

    let encrypted = row.and_then(|(meta,)| {
        meta.get("access_token")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
    })?;

    match SecretsCrypto::for_user(user_id).decrypt(&encrypted) {
        Ok(token) => Some(token),
        Err(e) => {
            warn!(user_id = %user_id, %e, "GitHub token decryption failed — re-auth required");
            None
        }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /api/github/login`
///
/// Builds the GitHub OAuth authorization URL for the authenticated user and
/// redirects the browser to GitHub's consent page.
async fn github_login(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let Some(svc) = state.github_svc.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "GitHub OAuth not configured").into_response();
    };

    match svc.authorization_url(&claims.sub) {
        Ok(url) => Redirect::temporary(&url).into_response(),
        Err(e) => {
            warn!(user = %claims.sub, %e, "failed to build GitHub authorization URL");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to generate authorization URL")
                .into_response()
        }
    }
}

/// `GET /api/v1/auth/github/login-user`
///
/// Returns the GitHub OAuth authorization URL as JSON so the client can open
/// it in a popup or new tab. Unlike `github_login`, this does not redirect
/// the browser directly — the client controls the navigation.
async fn github_login_user(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let Some(svc) = state.github_svc.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error": "GitHub OAuth not configured"}))).into_response();
    };

    match svc.authorization_url(&claims.sub) {
        Ok(url) => Json(serde_json::json!({"auth_url": url})).into_response(),
        Err(e) => {
            warn!(user = %claims.sub, %e, "failed to build GitHub authorization URL (login-user)");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "failed to generate authorization URL"}))).into_response()
        }
    }
}

/// `GET /api/v1/auth/github/token`
///
/// Polls whether the current user's GitHub OAuth flow has completed.
/// Returns `{connected: bool, valid: bool, login?: string}`.
/// The raw access token is never returned — callers check `connected` + `valid`
/// to determine whether GitHub features are available.
async fn github_token(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user identity").into_response(),
    };

    let Some(svc) = state.github_svc.as_ref() else {
        return Json(serde_json::json!({"connected": false, "configured": false})).into_response();
    };

    let Some(token) = load_github_token(&state.db, user_id).await else {
        return Json(serde_json::json!({"connected": false, "valid": false})).into_response();
    };

    // Fetch the GitHub login name from the stored identity row.
    let login: Option<String> = sqlx::query_scalar(
        "SELECT provider_username FROM user_identities WHERE user_id = $1 AND provider = 'github'",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let valid = svc.verify_token(&token).await.unwrap_or(false);
    Json(serde_json::json!({"connected": true, "valid": valid, "login": login})).into_response()
}

/// `GET /api/github/callback`  (public — registered in `public_router`)
///
/// GitHub redirects the browser here after the user grants access.
/// Verifies the HMAC-signed state (extracts `user_id` without needing
/// the auth header), exchanges the code, encrypts the token, and upserts
/// it into `user_identities`.  Redirects to `/add-agent.html` on success.
#[derive(Deserialize)]
struct CallbackQuery {
    code: String,
    state: String,
}

async fn github_callback(
    State(state): State<AppState>,
    Query(params): Query<CallbackQuery>,
) -> impl IntoResponse {
    let Some(svc) = state.github_svc.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "GitHub OAuth not configured").into_response();
    };

    // Verify the HMAC-signed state — gives us the user_id without auth headers.
    let oauth_claims = match svc.verify_state(&params.state) {
        Ok(c) => c,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid oauth state: {e}")).into_response()
        }
    };

    let user_id: Uuid = match oauth_claims.user_id.parse() {
        Ok(id) => id,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "invalid user_id in state").into_response()
        }
    };

    // Exchange the authorization code for an access token + user profile.
    let (token, user) = match svc.exchange_code(&params.code).await {
        Ok(t) => t,
        Err(e) => {
            warn!(user_id = %user_id, %e, "GitHub code exchange failed");
            return (StatusCode::BAD_GATEWAY, format!("GitHub OAuth failed: {e}")).into_response();
        }
    };

    // Encrypt before storing — provider_metadata is not encrypted at rest
    // by default, so we apply per-user AES-256-GCM here.
    let encrypted = SecretsCrypto::for_user(user_id).encrypt(&token.access_token);
    let meta = serde_json::json!({
        "access_token": encrypted,
        "login": user.login,
        "avatar_url": user.avatar_url,
    });

    // Upsert: ON CONFLICT on (provider, provider_id) so reconnecting the
    // same GitHub account refreshes the stored token.
    match sqlx::query(
        r#"INSERT INTO user_identities
               (user_id, provider, provider_id, provider_username, provider_metadata)
           VALUES ($1, 'github', $2, $3, $4)
           ON CONFLICT (provider, provider_id) DO UPDATE
               SET provider_metadata = EXCLUDED.provider_metadata,
                   provider_username  = EXCLUDED.provider_username"#,
    )
    .bind(user_id)
    .bind(user.id.to_string())
    .bind(&user.login)
    .bind(&meta)
    .execute(&state.db)
    .await
    {
        Ok(_) => Redirect::temporary("/add-agent.html?github_connected=true").into_response(),
        Err(e) => {
            warn!(%e, user_id = %user_id, "failed to persist GitHub token");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to save GitHub credentials")
                .into_response()
        }
    }
}

async fn github_status(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user identity").into_response(),
    };

    let Some(svc) = state.github_svc.as_ref() else {
        return (
            StatusCode::OK,
            Json(serde_json::json!({"connected": false, "configured": false})),
        )
            .into_response();
    };

    let Some(token) = load_github_token(&state.db, user_id).await else {
        return (
            StatusCode::OK,
            Json(serde_json::json!({"connected": false, "valid": false})),
        )
            .into_response();
    };

    let valid = svc.verify_token(&token).await.unwrap_or(false);
    (
        StatusCode::OK,
        Json(serde_json::json!({"connected": true, "valid": valid})),
    )
        .into_response()
}

async fn github_repos(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let Some(svc) = state.github_svc.as_ref() else {
        return (StatusCode::NOT_FOUND, "GitHub OAuth not configured").into_response();
    };

    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user identity").into_response(),
    };

    let Some(token) = load_github_token(&state.db, user_id).await else {
        return (StatusCode::FORBIDDEN, "GitHub not connected — visit /add-agent.html to connect")
            .into_response();
    };

    match svc.list_repos(&token).await {
        Ok(repos) => {
            let total = repos.len();
            (
                StatusCode::OK,
                Json(serde_json::json!({"repositories": repos, "total": total})),
            )
                .into_response()
        }
        Err(e) => {
            warn!(%e, "failed to list GitHub repositories");
            (StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
    }
}

async fn github_logout(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid user identity").into_response(),
    };

    match sqlx::query(
        "DELETE FROM user_identities WHERE user_id = $1 AND provider = 'github'",
    )
    .bind(user_id)
    .execute(&state.db)
    .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({"message": "GitHub credentials cleared"})),
        )
            .into_response(),
        Err(e) => {
            warn!(%e, user_id = %user_id, "failed to delete GitHub token from DB");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to clear GitHub credentials")
                .into_response()
        }
    }
}

/// `POST /api/github/clone`
///
/// Shallow-clones a GitHub repo using the user's stored OAuth token,
/// uploads the resulting `tar.gz` archive to S3 for future reference,
/// extracts it, reads `AgentCard.json`, and runs the full build+deploy
/// pipeline — the same pipeline as `POST /import/github` but using an
/// actual `git clone` instead of the GitHub API tarball endpoint.
#[derive(Deserialize)]
struct CloneBody {
    /// `"owner/repo"` identifier, e.g. `"acme/my-agent"`.
    repository: String,
    /// Branch to clone; defaults to `"main"`.
    branch: Option<String>,
}

#[derive(Serialize)]
struct CloneResult {
    agent_id: Uuid,
    build_id: Option<Uuid>,
    container_name: Option<String>,
    s3_key: String,
    archive_size_bytes: usize,
    status: String,
}

async fn github_clone(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CloneBody>,
) -> impl IntoResponse {
    let Some(svc) = state.github_svc.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "GitHub OAuth not configured").into_response();
    };

    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let Some(token) = load_github_token(&state.db, user_id).await else {
        return (
            StatusCode::FORBIDDEN,
            "GitHub not connected — visit /add-agent.html to connect",
        )
            .into_response();
    };

    let branch = body.branch.as_deref().unwrap_or("main");

    // Shallow-clone the repository into an in-memory tar.gz archive.
    let archive = match svc.clone_to_archive(&token, &body.repository, branch).await {
        Ok(a) => a,
        Err(e) => {
            warn!(repo = %body.repository, branch, %e, "git clone failed");
            return (StatusCode::BAD_GATEWAY, format!("clone failed: {e}")).into_response();
        }
    };

    let archive_size = archive.archive_bytes.len();
    let s3_key = archive.s3_key.clone();

    // Upload to S3 for future reference / re-builds without re-cloning.
    if let Err(e) = state
        .oci_storage
        .put_blob(&s3_key, bytes::Bytes::from(archive.archive_bytes.clone()))
        .await
    {
        warn!(s3_key, %e, "failed to upload clone archive to S3 — continuing with build");
    }

    // Extract to a temp directory and run the standard build+deploy pipeline.
    let tmp_dir = std::env::temp_dir().join(format!("nasiko-clone-{}", Uuid::new_v4()));
    if let Err(e) = crate::build::extract_tar_gzip(&archive.archive_bytes, &tmp_dir) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to extract archive: {e}"),
        )
            .into_response();
    }

    let meta = match crate::catalog::import::read_agent_card(&tmp_dir) {
        Ok(m) => m,
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
            return (StatusCode::BAD_REQUEST, e).into_response();
        }
    };

    let import_result =
        crate::catalog::import::build_and_deploy(&tmp_dir, &meta, user_id, &state).await;
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

    match import_result {
        Ok(r) => (
            StatusCode::CREATED,
            Json(CloneResult {
                agent_id: r.agent_id,
                build_id: r.build_id,
                container_name: r.container_name,
                s3_key,
                archive_size_bytes: archive_size,
                status: r.status,
            }),
        )
            .into_response(),
        Err((code, msg)) => (code, msg).into_response(),
    }
}
