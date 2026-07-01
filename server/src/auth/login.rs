use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

const COOKIE_MAX_AGE: u64 = 7 * 24 * 60 * 60;

/// Public routes — no auth required (merged outside the protected router).
pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/login", post(login))
}

/// Protected auth routes — go through require_auth middleware (nested under /api).
pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/auth/logout", post(logout))
        .route("/auth/tokens/validate", post(token_validate))
        .route("/auth/system/users-for-search", get(users_for_search))
}

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    token: String,
    user_id: String,
    username: String,
    is_superuser: bool,
    role: String,
    expires_in: u64,
    department_id: Option<String>,
    team_id: Option<String>,
}

fn set_token_cookie(token: &str) -> header::HeaderValue {
    header::HeaderValue::from_str(&format!(
        "access_token={}; HttpOnly; Path=/; SameSite=Strict; Max-Age={}",
        token, COOKIE_MAX_AGE
    ))
    .unwrap()
}

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    match state.providers.user_auth.authenticate(&req.username, &req.password).await {
        Ok(result) => {
            let cookie = set_token_cookie(&result.token);
            (
                [(header::SET_COOKIE, cookie)],
                Json(LoginResponse {
                    token: result.token,
                    user_id: result.user_id,
                    username: result.username,
                    is_superuser: result.is_superuser,
                    role: result.role,
                    expires_in: result.expires_in,
                    department_id: result.department_id,
                    team_id: result.team_id,
                }),
            ).into_response()
        }
        Err(nasiko_auth::AuthError::InvalidToken(msg)) if msg == "account disabled" => {
            (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "account disabled"}))).into_response()
        }
        Err(_) => {
            (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "invalid credentials"}))).into_response()
        }
    }
}

async fn logout() -> impl IntoResponse {
    let clear = header::HeaderValue::from_static(
        "access_token=; HttpOnly; Path=/; SameSite=Strict; Max-Age=0",
    );
    ([(header::SET_COOKIE, clear)], StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ValidateRequest {
    token: String,
}

async fn token_validate(
    State(state): State<AppState>,
    Json(req): Json<ValidateRequest>,
) -> impl IntoResponse {
    match state.providers.auth.validate_token(&req.token).await {
        Ok(identity) => Json(serde_json::json!({
            "valid": true,
            "user_id": identity.user_id,
            "username": identity.username,
            "is_superuser": identity.is_superuser,
            "role": identity.role.as_ref().and_then(|r| serde_json::to_value(r).ok()),
            "team_id": identity.team_id,
            "department_id": identity.department_id,
        })).into_response(),
        Err(nasiko_auth::AuthError::Expired) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"valid": false, "error": "expired"})),
        ).into_response(),
        Err(nasiko_auth::AuthError::Revoked) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"valid": false, "error": "token revoked"})),
        ).into_response(),
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"valid": false, "error": "invalid"})),
        ).into_response(),
    }
}

async fn users_for_search(State(state): State<AppState>) -> impl IntoResponse {
    #[derive(serde::Serialize, sqlx::FromRow)]
    struct SearchUser {
        id: uuid::Uuid,
        username: String,
        email: Option<String>,
        display_name: Option<String>,
        is_active: bool,
    }

    match sqlx::query_as::<_, SearchUser>(
        "SELECT id, username, email, display_name, is_active FROM users WHERE deleted_at IS NULL ORDER BY username",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(users) => {
            let count = users.len();
            Json(serde_json::json!({"count": count, "users": users})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
