use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::auth::Claims;
use crate::state::AppState;

const COOKIE_MAX_AGE: u64 = 7 * 24 * 60 * 60;

/// Public routes — no auth required (merged outside the protected router).
/// token_validate is here because callers supply the token in the request body;
/// there is no authenticated "caller" to require.
pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/initialize-admin", post(initialize_admin))
        .route("/api/auth/tokens/validate", post(token_validate))
}

/// Protected auth routes — require X-User-* headers from the gateway.
pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/auth/logout", post(logout))
        .route("/auth/system/users-for-search", get(users_for_search))
}

#[derive(Deserialize)]
struct LoginRequest {
    access_key: String,
    access_secret: String,
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

fn clear_token_cookie() -> header::HeaderValue {
    header::HeaderValue::from_static(
        "access_token=; HttpOnly; Path=/; SameSite=Strict; Max-Age=0",
    )
}

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    match state.providers.user_auth.authenticate(&req.access_key, &req.access_secret).await {
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

/// Logout: revoke the active token in the DB so it immediately stops working
/// at the gateway, then clear the browser cookie.
async fn logout(
    State(state): State<AppState>,
    claims: Claims,
) -> impl IntoResponse {
    // Best-effort revocation — don't fail the logout if the DB write fails
    let _ = state.providers.token_svc.revoke_for_user(&claims.sub).await;
    ([(header::SET_COOKIE, clear_token_cookie())], StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct InitAdminRequest {
    username: String,
    email: String,
}

async fn initialize_admin(
    State(state): State<AppState>,
    Json(req): Json<InitAdminRequest>,
) -> impl IntoResponse {
    match state.providers.user_auth.initialize_admin(&req.username, &req.email).await {
        Ok(result) => {
            let cookie = set_token_cookie(&result.token);
            (
                StatusCode::CREATED,
                [(header::SET_COOKIE, cookie)],
                Json(serde_json::json!({
                    "user_id": result.user_id,
                    "username": result.username,
                    "token": result.token,
                    "access_key": result.access_key,
                    "access_secret": result.access_secret,
                    "message": "Admin created. Store access_secret securely — it won't be shown again.",
                })),
            ).into_response()
        }
        Err(nasiko_auth::AuthError::InvalidToken(msg)) if msg.contains("already exists") => {
            (StatusCode::CONFLICT, Json(serde_json::json!({"error": msg}))).into_response()
        }
        Err(nasiko_auth::AuthError::InvalidToken(msg)) if msg.contains("admin already exists") => {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": msg}))).into_response()
        }
        Err(nasiko_auth::AuthError::InvalidToken(msg)) => {
            (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": msg}))).into_response()
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response()
        }
    }
}

#[derive(Deserialize)]
struct ValidateRequest {
    token: String,
}

async fn token_validate(
    State(state): State<AppState>,
    Json(req): Json<ValidateRequest>,
) -> impl IntoResponse {
    // Step 1: verify signature + expiry
    let identity = match state.providers.auth.validate_token(&req.token).await {
        Ok(id) => id,
        Err(nasiko_auth::AuthError::Expired) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"valid": false, "error": "expired"})),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"valid": false, "error": "invalid"})),
            )
                .into_response();
        }
    };

    // Step 2: check revocation — same logic as the gateway so validate is consistent.
    if let Some(jti) = nasiko_auth::jwt::extract_jti(&req.token) {
        let hash = nasiko_auth::jwt::hash_jti(&jti);
        let revoked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM auth_tokens WHERE token_hash = $1 AND revoked_at IS NOT NULL)",
        )
        .bind(&hash)
        .fetch_one(&state.db)
        .await
        .unwrap_or(false);

        if revoked {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"valid": false, "error": "token revoked"})),
            )
                .into_response();
        }
    }

    Json(serde_json::json!({
        "valid": true,
        "user_id": identity.user_id,
        "username": identity.username,
        "is_superuser": identity.is_superuser,
        "role": identity.role.as_ref().and_then(|r| serde_json::to_value(r).ok()),
        "team_id": identity.team_id,
        "department_id": identity.department_id,
    }))
    .into_response()
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
