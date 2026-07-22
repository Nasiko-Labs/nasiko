use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::{delete, get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tracing::warn;
use uuid::Uuid;

use crate::{auth::Claims, secrets::crypto::SecretsCrypto, state::AppState};

/// Public routes — no auth required (GitHub redirects the browser here).
/// Merged at the root level in `lib.rs` so the callback URL is reachable
/// without a bearer token.
pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/github/callback", get(github_callback))
        // Unauthenticated SSO login: returns {"auth_url": "..."} so the client
        // can open GitHub consent in a new tab without holding a session token.
        .route("/api/auth/github/login-user", get(github_login_user))
}

/// Protected routes — served under /api/v1 with require_auth middleware.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/github/login", get(github_login))
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

/// Mirrors `oss/github`'s own `OAUTH_STATE_MAX_AGE_SECS` — the Redis single-use
/// marker never needs to outlive the window during which the signed state is
/// itself still considered valid.
const OAUTH_STATE_TTL_SECS: i64 = 600;

/// Atomically mark an OAuth `state` value as consumed so a captured/replayed
/// `state` cannot be used a second time even though it is still a validly
/// signed, non-expired token (the HMAC + expiry checks in
/// `GitHubService::verify_state` only prove the state hasn't been *tampered
/// with*, not that it hasn't been *reused*).
///
/// Uses a single `SET key 1 NX EX ttl` Redis command — atomic across
/// concurrent callback requests racing on the same `state`, so two requests
/// replaying one captured value cannot both win. The key is the SHA-256 of the
/// raw state string (bounded length, safe charset) rather than the state
/// itself, to keep Redis keys short and avoid depending on the state's own
/// encoding.
///
/// Returns `Ok(true)` on first use (proceed), `Ok(false)` if already consumed
/// (reject as a replay).
async fn consume_oauth_state(redis: &redis::Client, raw_state: &str) -> redis::RedisResult<bool> {
    let mut hasher = Sha256::new();
    hasher.update(raw_state.as_bytes());
    let key = format!(
        "oauth:github:state:used:{}",
        URL_SAFE_NO_PAD.encode(hasher.finalize())
    );

    let mut conn = redis.get_multiplexed_async_connection().await?;
    let set: Option<String> = redis::cmd("SET")
        .arg(&key)
        .arg("1")
        .arg("NX")
        .arg("EX")
        .arg(OAUTH_STATE_TTL_SECS)
        .query_async(&mut conn)
        .await?;

    Ok(set.is_some())
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /api/github/login`
///
/// Builds the GitHub OAuth authorization URL for the authenticated user and
/// redirects the browser to GitHub's consent page.
async fn github_login(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let Some(svc) = state.github_svc.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "GitHub OAuth not configured",
        )
            .into_response();
    };

    match svc.authorization_url(&claims.sub) {
        Ok(url) => Redirect::temporary(&url).into_response(),
        Err(e) => {
            warn!(user = %claims.sub, %e, "failed to build GitHub authorization URL");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to generate authorization URL",
            )
                .into_response()
        }
    }
}

/// `GET /api/v1/auth/github/login-user`  (public — no auth required)
///
/// Returns the GitHub OAuth authorization URL as JSON so the client can open
/// it in a new tab for SSO login. Uses `flow="login"` in the state so the
/// callback handler knows to find/create a user rather than linking an
/// existing one.
async fn github_login_user(State(state): State<AppState>) -> impl IntoResponse {
    let Some(svc) = state.github_svc.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "GitHub OAuth not configured"})),
        )
            .into_response();
    };

    match svc.login_authorization_url() {
        Ok(url) => Json(serde_json::json!({"auth_url": url})).into_response(),
        Err(e) => {
            warn!(%e, "failed to build GitHub login authorization URL");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "failed to generate authorization URL"})),
            )
                .into_response()
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
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "GitHub OAuth not configured",
        )
            .into_response();
    };

    // Verify the HMAC-signed state — gives us the user_id without auth headers.
    let oauth_claims = match svc.verify_state(&params.state) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(%e, "invalid oauth state");
            return (StatusCode::BAD_REQUEST, "invalid oauth state").into_response();
        }
    };

    let user_id: Uuid = match oauth_claims.user_id.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid user_id in state").into_response(),
    };

    // Single-use enforcement: a validly signed, non-expired `state` must still
    // only be usable ONCE. Without this, a captured `state` (e.g. via referrer
    // leakage or a logged URL) could be replayed to re-trigger the callback
    // flow for up to 10 minutes. Fail OPEN on a Redis error (log + continue) —
    // this is defense-in-depth on top of the HMAC + expiry checks already
    // performed by `verify_state`, not the only line of defense.
    match consume_oauth_state(&state.redis, &params.state).await {
        Ok(true) => {}
        Ok(false) => {
            warn!(user_id = %user_id, "GitHub OAuth callback: state replay detected — rejecting");
            return (StatusCode::BAD_REQUEST, "oauth state already used").into_response();
        }
        Err(e) => {
            warn!(%e, user_id = %user_id, "oauth state single-use check failed (redis error) — proceeding without replay protection");
        }
    }

    // Exchange the authorization code for an access token + user profile.
    let (token, github_user) = match svc.exchange_code(&params.code).await {
        Ok(t) => t,
        Err(e) => {
            warn!(%e, "GitHub code exchange failed");
            return (StatusCode::BAD_GATEWAY, "GitHub OAuth failed").into_response();
        }
    };

    // Dispatch on flow: "login" = SSO sign-in, anything else = connect existing account.
    let flow = oauth_claims.flow.as_deref().unwrap_or("connect");
    if flow == "login" {
        return github_callback_login(state, token, github_user).await;
    }

    // ── connect flow ─────────────────────────────────────────────────────────
    //
    // Encrypt before storing — provider_metadata is not encrypted at rest
    // by default, so we apply per-user AES-256-GCM here.
    let encrypted = SecretsCrypto::for_user(user_id).encrypt(&token.access_token);
    let meta = serde_json::json!({
        "access_token": encrypted,
        "login": github_user.login,
        "avatar_url": github_user.avatar_url,
    });

    // Upsert: ON CONFLICT on (provider, provider_id) so reconnecting the same
    // GitHub account refreshes the stored token — but ONLY when the existing
    // row already belongs to this same `user_id`. Without the `WHERE` guard,
    // a SECOND user linking a GitHub account already linked to a FIRST user
    // would silently overwrite provider_metadata (and thus the encrypted
    // access token) on the FIRST user's row while leaving `user_id` untouched
    // — leaving a row whose token is encrypted under user B's per-user key
    // (see `SecretsCrypto::for_user`) but keyed to user A's `user_id`, and
    // silently reassigning a GitHub identity to a different Nasiko account
    // with no audit trail. We reject that silently (rows_affected() == 0)
    // rather than reassigning, since reassignment is a bigger decision than a
    // token-refresh upsert should make implicitly.
    match sqlx::query(
        r#"INSERT INTO user_identities
               (user_id, provider, provider_id, provider_username, provider_metadata)
           VALUES ($1, 'github', $2, $3, $4)
           ON CONFLICT (provider, provider_id) DO UPDATE
               SET provider_metadata = EXCLUDED.provider_metadata,
                   provider_username  = EXCLUDED.provider_username
               WHERE user_identities.user_id = EXCLUDED.user_id"#,
    )
    .bind(user_id)
    .bind(github_user.id.to_string())
    .bind(&github_user.login)
    .bind(&meta)
    .execute(&state.db)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            Redirect::temporary("/add-agent.html?github_connected=true").into_response()
        }
        Ok(_) => {
            warn!(
                user_id = %user_id,
                github_login = %github_user.login,
                "GitHub account already linked to a different Nasiko user — refusing to reassign"
            );
            (
                StatusCode::CONFLICT,
                "this GitHub account is already linked to a different Nasiko account",
            )
                .into_response()
        }
        Err(e) => {
            warn!(%e, user_id = %user_id, "failed to persist GitHub token");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to save GitHub credentials",
            )
                .into_response()
        }
    }
}

/// Login flow callback: find or create the Nasiko user from the GitHub identity,
/// issue a JWT, and redirect with the token in query params.
async fn github_callback_login(
    state: AppState,
    _token: nasiko_github::AccessToken,
    github_user: nasiko_github::GitHubUser,
) -> axum::response::Response {
    let provider_id = github_user.id.to_string();

    let result = match state
        .auth
        .upsert_oauth_user("github", &provider_id, &github_user.login)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(%e, github_login = %github_user.login, "GitHub SSO login: upsert_oauth_user failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to complete GitHub login",
            )
                .into_response();
        }
    };

    // Use reqwest::Url for query-param encoding (handles any chars in username safely).
    // APP_BASE_URL overrides the redirect base — useful when the server and the app
    // run on different origins in dev. In prod both share the same base URL.
    let base = if state.config.app_base_url.is_empty() {
        "http://placeholder".to_string()
    } else {
        state.config.app_base_url.trim_end_matches('/').to_string()
    };
    let mut redirect = reqwest::Url::parse(&format!("{base}/")).expect("valid base URL");
    {
        let mut q = redirect.query_pairs_mut();
        q.append_pair("token", &result.token);
        q.append_pair("token_type", "bearer");
        q.append_pair("username", &result.username);
        q.append_pair("is_super_user", &result.is_superuser.to_string());
    }
    let redirect_target = if state.config.app_base_url.is_empty() {
        format!("/?{}", redirect.query().unwrap_or_default())
    } else {
        redirect.to_string()
    };

    Redirect::temporary(&redirect_target).into_response()
}

async fn github_status(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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

    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let Some(token) = load_github_token(&state.db, user_id).await else {
        return (
            StatusCode::FORBIDDEN,
            "GitHub not connected — visit /add-agent.html to connect",
        )
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
            (
                StatusCode::BAD_GATEWAY,
                "failed to list GitHub repositories",
            )
                .into_response()
        }
    }
}

async fn github_logout(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    match sqlx::query("DELETE FROM user_identities WHERE user_id = $1 AND provider = 'github'")
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
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to clear GitHub credentials",
            )
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
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "GitHub OAuth not configured",
        )
            .into_response();
    };

    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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
            return (StatusCode::BAD_GATEWAY, "clone failed").into_response();
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

    // Extract + parse on the blocking pool — a large repo archive must not
    // gzip-decompress + untar on a tokio worker thread.
    let tmp_dir = std::env::temp_dir().join(format!("nasiko-clone-{}", Uuid::new_v4()));
    let meta = {
        let bytes = archive.archive_bytes;
        let tmp = tmp_dir.clone();
        match tokio::task::spawn_blocking(move || {
            crate::build::extract_tar_gzip(&bytes, &tmp)?;
            crate::catalog::import::read_agent_card(&tmp)
        })
        .await
        {
            Ok(Ok(m)) => m,
            Ok(Err(e)) => {
                let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
                tracing::error!(%e, repo = %body.repository, branch, "failed to import cloned repo");
                return (StatusCode::BAD_REQUEST, "failed to import cloned repo").into_response();
            }
            Err(e) => {
                let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
                tracing::error!(%e, repo = %body.repository, branch, "extract task failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Requires a live Redis (see module-level test infra requirements in
    /// oss/server/tests/*). Verifies the OAuth `state` single-use marker
    /// (SEC fix #5): a fresh state may be consumed exactly once, and a second
    /// consumption attempt of the SAME state must be rejected as a replay.
    #[tokio::test]
    async fn oauth_state_is_single_use() {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
        let client = redis::Client::open(redis_url).expect("redis client");
        let state_value = format!("test-replay-{}", Uuid::new_v4());

        let first = consume_oauth_state(&client, &state_value)
            .await
            .expect("redis reachable");
        assert!(first, "first use of a fresh state must succeed");

        let second = consume_oauth_state(&client, &state_value)
            .await
            .expect("redis reachable");
        assert!(!second, "replaying the same state must be rejected");

        // A different state value must be independent (not accidentally
        // sharing a key with the first).
        let other_state_value = format!("test-replay-{}", Uuid::new_v4());
        let third = consume_oauth_state(&client, &other_state_value)
            .await
            .expect("redis reachable");
        assert!(
            third,
            "a distinct state value must not be affected by another's use"
        );
    }
}
