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

const COOKIE_MAX_AGE: u64 = 12 * 60 * 60; // 12 hours — aligned with JWT TTL

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

/// Accepts either `{username, password}` or `{access_key, access_secret}`.
/// The auth service handles both via its credential lookup query.
#[derive(Deserialize)]
#[serde(untagged)]
enum LoginRequest {
    Credentials { username: String, password: String },
    AccessKey { access_key: String, access_secret: String },
}

#[derive(Serialize)]
struct LoginResponse {
    token: String,
    user_id: String,
    username: String,
    is_superuser: bool,
    role: String,
    expires_in: u64,
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
    let (key, secret) = match req {
        LoginRequest::Credentials { username, password } => (username, password),
        LoginRequest::AccessKey { access_key, access_secret } => (access_key, access_secret),
    };
    match state.auth.authenticate(&key, &secret).await {
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
                }),
            )
                .into_response()
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
    let _ = state.auth.revoke_tokens_for_user(&claims.sub).await;
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
    // bootstrap_admin doesn't return credentials — use initialize_admin_full for that
    let _ = state.auth.authenticate(&req.username, &req.email).await;
    // The actual initialize-admin endpoint uses a different flow:
    // it creates the admin user and returns credentials.
    match initialize_admin_inner(&state, &req.username, &req.email).await {
        Ok(resp) => resp,
        Err(resp) => resp,
    }
}

async fn initialize_admin_inner(
    state: &AppState,
    username: &str,
    email: &str,
) -> Result<axum::response::Response, axum::response::Response> {
    // Check if admin exists
    let admin_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE role = 'admin' AND deleted_at IS NULL",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    if admin_count > 0 {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "admin already exists"})),
        ).into_response());
    }

    let access_key = nasiko_auth::generate_access_key();
    let access_secret = nasiko_auth::generate_access_secret();
    let access_secret_hash = nasiko_auth::hash_password_async(&access_secret).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response())?;

    let result: Result<(uuid::Uuid,), _> = sqlx::query_as(
        r#"INSERT INTO users (username, email, is_superuser, is_active, role)
           VALUES ($1, $2, true, true, 'admin'::user_role)
           RETURNING id"#,
    )
    .bind(username)
    .bind(email)
    .fetch_one(&state.db)
    .await;

    let user_id = match result {
        Ok((id,)) => id,
        Err(e) if e.to_string().contains("unique") || e.to_string().contains("duplicate") => {
            return Err((
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "username or email already exists"})),
            ).into_response());
        }
        Err(e) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e.to_string()})),
            ).into_response());
        }
    };

    sqlx::query(
        r#"INSERT INTO user_credentials (user_id, access_key, access_secret_hash)
           VALUES ($1, $2, $3)"#,
    )
    .bind(user_id)
    .bind(&access_key)
    .bind(&access_secret_hash)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response())?;

    let identity = nasiko_auth::Identity {
        user_id: user_id.to_string(),
        username: username.to_owned(),
        is_superuser: true,
        role: Some(nasiko_auth::Role::Admin),
    };

    let token = state.auth.issue_token(&identity).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response())?;

    // Record the issued token so revocation and counting work correctly.
    let _ = state.auth.record_user_token(&token, &user_id.to_string()).await;

    let cookie = set_token_cookie(&token);
    Ok((
        StatusCode::CREATED,
        [(header::SET_COOKIE, cookie)],
        Json(serde_json::json!({
            "user_id": user_id.to_string(),
            "username": username,
            "token": token,
            "access_key": access_key,
            "access_secret": access_secret,
            "message": "Admin created. Store access_secret securely — it won't be shown again.",
        })),
    ).into_response())
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
    let identity = match state.auth.validate_token(&req.token).await {
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
