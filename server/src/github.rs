use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get},
};
use tracing::warn;
use uuid::Uuid;

use nasiko_secrets::SecretsCrypto;

use crate::{auth::Claims, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/github/status", get(github_status))
        .route("/github/repos", get(github_repos))
        .route("/github/logout", delete(github_logout))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub(crate) async fn load_github_token(state: &AppState, user_id: Uuid) -> Option<String> {
    let row: Option<(serde_json::Value,)> = match sqlx::query_as(
        "SELECT provider_metadata FROM user_identities \
         WHERE user_id = $1 AND provider = 'github'",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
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

    let Some(token) = load_github_token(&state, user_id).await else {
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

    let Some(token) = load_github_token(&state, user_id).await else {
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
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to clear GitHub credentials").into_response()
        }
    }
}
