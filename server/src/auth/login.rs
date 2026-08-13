use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

const COOKIE_MAX_AGE: u64 = 12 * 60 * 60; // 12 hours — aligned with JWT TTL

/// Routes shared by OSS and EE: initialize-admin and token validation.
/// Does not include /api/auth/login — each edition registers its own login
/// handler via `public_router`.
///
/// `login_limiter` bounds bcrypt cost from `initialize_admin`. `token_validate`
/// is cheap (JWT decode + one indexed lookup) and is not rate-limited.
pub fn non_login_public_router(login_limiter: crate::rate_limit::RateLimiter) -> Router<AppState> {
    let credential_routes = Router::new()
        .route("/api/auth/initialize-admin", post(initialize_admin))
        .layer(axum::middleware::from_fn_with_state(
            login_limiter,
            crate::rate_limit::limit_globally,
        ));

    Router::new()
        .merge(credential_routes)
        .route("/api/auth/tokens/validate", post(token_validate))
}

pub fn public_router(login_limiter: crate::rate_limit::RateLimiter) -> Router<AppState> {
    Router::new()
        .route("/api/auth/login", post(login))
        .layer(axum::middleware::from_fn_with_state(
            login_limiter,
            crate::rate_limit::limit_globally,
        ))
}

/// Protected auth routes — require X-User-* headers from the gateway.
pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/auth/logout", post(logout))
        .route("/auth/system/users-for-search", get(users_for_search))
        .route("/auth/users/{id}", get(get_user_profile))
}

/// Accepts either `{username, password}` or `{access_key, access_secret}`.
/// The auth service handles both via its credential lookup query.
#[derive(Deserialize)]
#[serde(untagged)]
enum LoginRequest {
    Credentials {
        username: String,
        password: String,
    },
    AccessKey {
        access_key: String,
        access_secret: String,
    },
}

#[derive(Serialize)]
struct LoginResponse {
    token: String,
    user_id: String,
    username: String,
    is_superuser: bool,
    expires_in: u64,
}

/// Browsers only honor `Secure` cookies over HTTPS (localhost excepted).
/// This server never terminates TLS itself, so an HTTPS request can only have
/// arrived through a reverse proxy — which advertises it via
/// `X-Forwarded-Proto`. Setting `Secure` unconditionally makes browsers
/// silently drop the cookie on plain-HTTP deployments (e.g. `http://host:8080`):
/// login appears to succeed but every subsequent request is an
/// unauthenticated 401.
pub fn request_is_https(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|p| p.eq_ignore_ascii_case("https"))
}

pub fn set_token_cookie(token: &str, secure: bool) -> header::HeaderValue {
    let secure_attr = if secure { " Secure;" } else { "" };
    header::HeaderValue::from_str(&format!(
        "access_token={}; HttpOnly;{} Path=/; SameSite=Strict; Max-Age={}",
        token, secure_attr, COOKIE_MAX_AGE
    ))
    .unwrap()
}

fn clear_token_cookie(secure: bool) -> header::HeaderValue {
    let secure_attr = if secure { " Secure;" } else { "" };
    header::HeaderValue::from_str(&format!(
        "access_token=; HttpOnly;{} Path=/; SameSite=Strict; Max-Age=0",
        secure_attr
    ))
    .unwrap()
}

async fn login(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    let (key, secret) = match req {
        LoginRequest::Credentials { username, password } => (username, password),
        LoginRequest::AccessKey {
            access_key,
            access_secret,
        } => (access_key, access_secret),
    };
    match state.auth.authenticate(&key, &secret).await {
        Ok(result) => {
            let cookie = set_token_cookie(&result.token, request_is_https(&headers));
            (
                [(header::SET_COOKIE, cookie)],
                Json(LoginResponse {
                    token: result.token,
                    user_id: result.user_id,
                    username: result.username,
                    is_superuser: result.is_superuser,
                    expires_in: result.expires_in,
                }),
            )
                .into_response()
        }
        Err(nasiko_auth::AuthError::Disabled) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "account disabled"})),
        )
            .into_response(),
        // A backend failure must surface as 500 — never as an auth rejection (AUTH-10).
        // The raw error is logged, not returned in the body.
        Err(nasiko_auth::AuthError::Database(e)) => {
            tracing::error!(%e, "login: database error");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid credentials"})),
        )
            .into_response(),
    }
}

/// Logout: revoke the calling session's own token in the DB so it immediately
/// stops working, then clear the browser cookie.
///
/// Revokes by `jti` (this session only) — NOT `revoke_tokens_for_user`, which
/// would kill every other active session for this user (e.g. a CLI session
/// logging out would silently sign the browser out too, and vice versa).
async fn logout(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    _claims: Claims,
) -> impl IntoResponse {
    // Best-effort revocation — don't fail the logout if the DB write fails
    if let Some(token) = super::middleware::extract_token(&headers)
        && let Some(jti) = nasiko_auth::jwt::extract_jti(&token)
    {
        let _ = state.auth.revoke_token(&jti).await;
    }
    (
        [(
            header::SET_COOKIE,
            clear_token_cookie(request_is_https(&headers)),
        )],
        StatusCode::NO_CONTENT,
    )
}

#[derive(Deserialize)]
struct InitAdminRequest {
    username: String,
    email: String,
}

async fn initialize_admin(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<InitAdminRequest>,
) -> impl IntoResponse {
    // Creates the admin user + credentials and returns them (with a recorded token).
    match initialize_admin_inner(
        &state,
        &req.username,
        &req.email,
        request_is_https(&headers),
    )
    .await
    {
        Ok(resp) => resp,
        Err(resp) => resp,
    }
}

async fn initialize_admin_inner(
    state: &AppState,
    username: &str,
    email: &str,
    https: bool,
) -> Result<axum::response::Response, axum::response::Response> {
    // Check if admin exists
    let admin_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE role = 'admin' AND deleted_at IS NULL",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    if admin_count > 0 {
        // 409: initializing an admin when one already exists is a conflict, not an
        // authorization failure.
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "admin already exists"})),
        )
            .into_response());
    }

    let access_key = nasiko_auth::generate_access_key();
    let access_secret = nasiko_auth::generate_access_secret();
    let access_secret_hash = nasiko_auth::hash_password_async(&access_secret)
        .await
        .map_err(|e| {
            tracing::error!(%e, "initialize_admin: password hash failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response()
        })?;

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
            )
                .into_response());
        }
        Err(e) => {
            tracing::error!(%e, "initialize_admin: insert user failed");
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response());
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
    .map_err(|e| {
        tracing::error!(%e, %user_id, "initialize_admin: insert credentials failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "internal error"})),
        )
            .into_response()
    })?;

    // `issue_token` records the JWT's JTI in auth_tokens, so the admin token minted
    // here is revocable (previously init-admin tokens were unrevocable).
    let identity = nasiko_auth::Identity {
        user_id: user_id.to_string(),
        username: username.to_owned(),
        is_superuser: true,
    };

    let token = state.auth.issue_token(&identity).await.map_err(|e| {
        tracing::error!(%e, %user_id, "initialize_admin: issue token failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "internal error"})),
        )
            .into_response()
    })?;

    // Belt-and-suspenders record (ON CONFLICT DO NOTHING) so counting/revocation work.
    let _ = state
        .auth
        .record_user_token(&token, &user_id.to_string())
        .await;

    let cookie = set_token_cookie(&token, https);
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
    )
        .into_response())
}

fn extract_bearer_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.to_string())
}

fn extract_cookie_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|c| {
                let c = c.trim();
                c.strip_prefix("access_token=").map(|t| t.to_string())
            })
        })
}

async fn token_validate(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    // Extract token from Authorization: Bearer <token> or the access_token cookie.
    let token = extract_bearer_token(&headers).or_else(|| extract_cookie_token(&headers));
    let token = match token {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"valid": false, "error": "no token provided"})),
            )
                .into_response();
        }
    };

    // Step 1: verify signature + expiry
    let identity = match state.auth.validate_token(&token).await {
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

    // Step 2: check revocation — same logic as require_auth so validate is consistent.
    // Fail CLOSED (AUTH-5): if the lookup errors we cannot prove the token is
    // still valid, so we deny rather than let a possibly-revoked token through.
    if let Some(jti) = nasiko_auth::jwt::extract_jti(&token) {
        let hash = nasiko_auth::jwt::hash_jti(&jti);
        let revoked: bool = match sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM auth_tokens WHERE token_hash = $1 AND revoked_at IS NOT NULL)",
        )
        .bind(&hash)
        .fetch_one(&state.db)
        .await
        {
            Ok(revoked) => revoked,
            Err(e) => {
                tracing::error!(%e, "token_validate: revocation lookup failed; failing closed");
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({"valid": false, "error": "token validation unavailable"})),
                )
                    .into_response();
            }
        };

        if revoked {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"valid": false, "error": "token revoked"})),
            )
                .into_response();
        }
    }

    // Fetch the user's actual role from the DB so the Flutter sidebar can
    // gate admin-only tabs (access control) correctly. Fall back to
    // is_superuser-derived role on any error (user deleted, DB unavailable).
    let role: String =
        sqlx::query_scalar("SELECT role::text FROM users WHERE id = $1 AND deleted_at IS NULL")
            .bind(identity.user_id.parse::<uuid::Uuid>().unwrap_or_default())
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                if identity.is_superuser {
                    "admin".into()
                } else {
                    "member".into()
                }
            });

    Json(serde_json::json!({
        "valid": true,
        // subject_id / subject_type are the fields the Flutter client reads.
        "subject_id": identity.user_id,
        "subject_type": "user",
        "role": role,
        // Keep legacy fields so any existing integrations aren't broken.
        "user_id": identity.user_id,
        "username": identity.username,
        "is_superuser": identity.is_superuser,
    }))
    .into_response()
}

/// Directory-style user search (autocomplete for e.g. granting agent access).
/// Previously reachable by any authenticated principal (member, or an agent
/// token) with no gate beyond `require_auth`, leaking every user's email and
/// the full org roster — now requires `can_read_org` (team_lead+ in EE;
/// unrestricted in OSS's single-user model), is scoped in EE to the caller's
/// own team or department via `org_visible_user_ids` (never `None` unless
/// admin/superuser — see that method's doc for why this crate never touches
/// EE-only team_id/department_id columns directly), and never returns email
/// at all — this is a mention/autocomplete endpoint, not a directory lookup,
/// and email was never needed to disambiguate users by username (AUTH-6).
async fn users_for_search(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_read_org(&identity).await {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "requires team lead or above"})),
        )
            .into_response();
    }

    #[derive(serde::Serialize, sqlx::FromRow)]
    struct SearchUser {
        id: uuid::Uuid,
        username: String,
        display_name: Option<String>,
        is_active: bool,
    }

    let visible_ids = state.auth.org_visible_user_ids(&identity).await;

    let result = match &visible_ids {
        None => {
            sqlx::query_as::<_, SearchUser>(
                "SELECT id, username, display_name, is_active FROM users WHERE deleted_at IS NULL ORDER BY username",
            )
            .fetch_all(&state.db)
            .await
        }
        Some(ids) => {
            let uuids: Vec<uuid::Uuid> = ids.iter().filter_map(|s| s.parse().ok()).collect();
            sqlx::query_as::<_, SearchUser>(
                "SELECT id, username, display_name, is_active FROM users WHERE deleted_at IS NULL AND id = ANY($1) ORDER BY username",
            )
            .bind(&uuids)
            .fetch_all(&state.db)
            .await
        }
    };

    match result {
        Ok(users) => {
            let count = users.len();
            Json(serde_json::json!({"count": count, "users": users})).into_response()
        }
        Err(e) => {
            tracing::error!(%e, "users_for_search: db error");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

async fn get_user_profile(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    // Non-superusers may only fetch their own profile.
    let caller: Uuid = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    if !claims.is_superuser && caller != id {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    #[derive(serde::Serialize, sqlx::FromRow)]
    struct ProfileRow {
        id: Uuid,
        username: String,
        email: String,
        display_name: Option<String>,
        is_superuser: bool,
        is_active: bool,
        role: Option<String>,
        created_at: chrono::DateTime<chrono::Utc>,
        last_login: Option<chrono::DateTime<chrono::Utc>>,
    }

    let result: Result<Option<ProfileRow>, _> = sqlx::query_as(
        "SELECT id, username, email, display_name, is_superuser, is_active,
                role::text as role, created_at, last_login
         FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;

    match result {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "user not found").into_response(),
        Err(e) => {
            tracing::error!(%e, %id, "get_user_profile: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}
