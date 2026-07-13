use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;
use crate::Paginated;

/// Returns 409 if `target_id` is the only active admin left.
async fn check_last_admin(state: &AppState, target_id: Uuid) -> Option<axum::response::Response> {
    // "Is admin" deliberately ignores is_active: we're asking whether the TARGET
    // currently holds admin rights at all, since that's what's about to be
    // revoked/demoted — an already-inactive admin still holds the role.
    let is_admin: Option<bool> = sqlx::query_scalar(
        "SELECT (role = 'admin' OR is_superuser) FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(target_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    if is_admin == Some(true) {
        // Count OTHER active admins (explicitly excluding target_id by id, not
        // just relying on an is_active filter). Excluding only by is_active
        // happened to work when the target itself was active (it was naturally
        // included in the count, so "<=1" meant "no one but me"), but undercounted
        // "remaining" admins when the target was already inactive (e.g. demoting
        // role on an inactive admin, or calling deactivate twice) — in that case
        // the target was never in the count, so the same "<=1" threshold could
        // block an operation even when exactly one OTHER active admin remained.
        let other_active_admins: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM users WHERE id != $1 AND (role = 'admin' OR is_superuser) AND is_active = true AND deleted_at IS NULL",
        )
        .bind(target_id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

        if other_active_admins == 0 {
            return Some(
                (StatusCode::CONFLICT, Json(serde_json::json!({"error": "cannot deactivate the last admin"}))).into_response(),
            );
        }
    }
    None
}

/// Full user orchestrator — list, get, and all management routes including role changes.
/// Used by the OSS server. EE provides its own orchestrator (ee/server/src/users.rs)
/// that merges management_router() and supplies EE-aware handlers + the cascade
/// role-change endpoint.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(management_router())
        // Role change must come before /{id} to avoid being swallowed as an id segment.
        .route("/users/{id}/role", put(change_role))
        // Update is registered separately (see management_router()'s doc comment) so
        // EE can override it to layer department_id/team_id assignment on top.
        .route("/users/{id}", put(update_user))
        // Static sub-paths MUST come before /{id} to avoid being captured as IDs.
        .route("/users/me", get(get_me))
        .route("/users", get(list_users))
        .route("/users/{id}", get(get_user))
        .route("/users/me/accessible-agents", get(my_accessible_agents))
        .route("/users/{id}/accessible-agents", get(accessible_agents_for_user))
}

/// Management-only orchestrator — create/delete users and related operations.
/// The role-change and update endpoints are registered separately (in `router()`)
/// so each can be overridden without causing a duplicate-route panic: EE wraps
/// `change_role` with its leadership cascade, and wraps `update_user` to also
/// accept `department_id`/`team_id` (EE-only columns `oss/server`'s `users`
/// table doesn't have — see `ee/server/src/users.rs::ee_update_user`).
pub fn management_router() -> Router<AppState> {
    Router::new()
        .route("/users/admins", get(list_admins))
        .route("/users", post(create_user))
        .route("/users/{id}", delete(delete_user))
        .route("/users/{id}/deactivate", post(deactivate))
        .route("/users/{id}/reinstate", post(reinstate))
        .route("/users/{id}/regenerate-credentials", post(regenerate_credentials))
}

#[derive(Debug, Serialize, FromRow)]
struct UserRow {
    id: Uuid,
    username: String,
    email: String,
    display_name: Option<String>,
    is_superuser: bool,
    is_active: bool,
    role: Option<String>,
    created_at: DateTime<Utc>,
    last_login: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 {
    20
}

async fn list_users(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let users: Result<Vec<UserRow>, _> = if let Some(ref search) = q.q {
        let pattern = format!("%{}%", search);
        sqlx::query_as::<_, UserRow>(
            r#"SELECT u.id, u.username, u.email, u.display_name, u.is_superuser,
                      u.is_active, u.role::text as role,
                      u.created_at, u.last_login
               FROM users u
               WHERE u.deleted_at IS NULL
                 AND (u.username ILIKE $1 OR u.email ILIKE $1 OR u.display_name ILIKE $1)
               ORDER BY u.created_at DESC
               LIMIT $2 OFFSET $3"#,
        )
        .bind(&pattern)
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, UserRow>(
            r#"SELECT u.id, u.username, u.email, u.display_name, u.is_superuser,
                      u.is_active, u.role::text as role,
                      u.created_at, u.last_login
               FROM users u
               WHERE u.deleted_at IS NULL
               ORDER BY u.created_at DESC
               LIMIT $1 OFFSET $2"#,
        )
        .bind(q.limit)
        .bind(q.offset)
        .fetch_all(&state.db)
        .await
    };

    match users {
        Ok(data) => {
            // Count must match the page query's WHERE (deleted_at + search), else
            // the total is wrong for filtered/soft-deleted views (AUTH-6).
            let total: i64 = if let Some(ref search) = q.q {
                let pattern = format!("%{}%", search);
                sqlx::query_scalar(
                    "SELECT COUNT(*) FROM users u WHERE u.deleted_at IS NULL \
                     AND (u.username ILIKE $1 OR u.email ILIKE $1 OR u.display_name ILIKE $1)",
                )
                .bind(&pattern)
                .fetch_one(&state.db)
                .await
                .unwrap_or(0)
            } else {
                sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE deleted_at IS NULL")
                    .fetch_one(&state.db)
                    .await
                    .unwrap_or(0)
            };
            Json(Paginated { data, total: total as usize }).into_response()
        }
        Err(e) => {
            tracing::error!(%e, "list_users: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn get_user(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let result: Result<Option<UserRow>, _> = sqlx::query_as::<_, UserRow>(
        r#"SELECT u.id, u.username, u.email, u.display_name, u.is_superuser,
                  u.is_active, u.role::text as role,
                  u.created_at, u.last_login
           FROM users u
           WHERE u.id = $1 AND u.deleted_at IS NULL"#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;

    match result {
        Ok(Some(user)) => Json(user).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "user not found").into_response(),
        Err(e) => {
            tracing::error!(%e, %id, "get_user: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

#[derive(Deserialize)]
struct CreateUser {
    username: String,
    email: String,
    display_name: Option<String>,
    role: Option<String>,
}

async fn create_user(
    State(state): State<AppState>,
    Json(body): Json<CreateUser>,
) -> impl IntoResponse {
    let access_key = nasiko_auth::generate_access_key();
    let access_secret = nasiko_auth::generate_access_secret();
    let access_secret_hash = match nasiko_auth::hash_password_async(&access_secret).await {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(%e, "create_user: password hash failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };

    let role = body.role.as_deref().unwrap_or("member");
    let id = Uuid::new_v4();
    let result = sqlx::query(
        r#"INSERT INTO users (id, username, email, display_name, is_superuser, is_active, role)
           VALUES ($1, $2, $3, $4, false, true, $5::user_role)"#,
    )
    .bind(id)
    .bind(&body.username)
    .bind(&body.email)
    .bind(&body.display_name)
    .bind(role)
    .execute(&state.db)
    .await;

    match result {
        Ok(_) => {
            let cred_result = sqlx::query(
                r#"INSERT INTO user_credentials (user_id, access_key, access_secret_hash)
                   VALUES ($1, $2, $3)"#,
            )
            .bind(id)
            .bind(&access_key)
            .bind(&access_secret_hash)
            .execute(&state.db)
            .await;

            match cred_result {
                Ok(_) => (
                    StatusCode::CREATED,
                    Json(serde_json::json!({
                        "id": id,
                        "username": body.username,
                        "access_key": access_key,
                        "access_secret": access_secret,
                        "message": "Store access_secret securely — it won't be shown again.",
                    })),
                ).into_response(),
                Err(e) => {
                    tracing::error!(%e, %id, "create_user: insert credentials failed");
                    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
                }
            }
        }
        Err(e) => {
            if e.to_string().contains("duplicate key") {
                (StatusCode::CONFLICT, "username or email already exists").into_response()
            } else {
                tracing::error!(%e, %id, "create_user: insert user failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
            }
        }
    }
}

/// `pub` (fields included) so EE's `ee_update_user` can construct one directly
/// and delegate to `update_user` for the fields both editions share, then layer
/// its own `department_id`/`team_id` handling on top — see
/// `ee/server/src/users.rs::ee_update_user`.
#[derive(Deserialize)]
pub struct UpdateUser {
    pub username: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub password: Option<String>,
    pub is_active: Option<bool>,
}

#[derive(Deserialize)]
pub struct ChangeRoleRequest {
    pub role: String,
}

pub async fn update_user(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUser>,
) -> impl IntoResponse {
    // Validate inputs up front
    if body.username.as_deref() == Some("") {
        return (StatusCode::BAD_REQUEST, "username cannot be empty").into_response();
    }
    if body.email.as_deref() == Some("") {
        return (StatusCode::BAD_REQUEST, "email cannot be empty").into_response();
    }
    if let Some(ref p) = body.password
        && p.len() < 8 {
            return (StatusCode::BAD_REQUEST, "password must be at least 8 characters").into_response();
        }

    // AUTH-2: an `is_active: false` transition through this generic PUT must go
    // through the exact same guards as the dedicated /deactivate route — otherwise
    // a caller could deactivate the last admin, or themselves, by smuggling
    // `is_active: false` into an update instead of using /deactivate.
    if body.is_active == Some(false) {
        if claims.sub == id.to_string() {
            return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "cannot deactivate your own account"}))).into_response();
        }
        if let Some(err) = check_last_admin(&state, id).await {
            return err;
        }
    }

    // Hash password if provided. This is written to user_credentials.access_secret_hash
    // below — NOT users.password_hash — because `authenticate()` (oss/auth/src/service.rs)
    // verifies exclusively against user_credentials.access_secret_hash and never reads
    // users.password_hash. Writing there was dead code that looked like a working
    // "change password" flow but had zero effect on login.
    let access_secret_hash = match &body.password {
        Some(p) => match nasiko_auth::hash_password_async(p).await {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!(%e, %id, "update_user: password hash failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
            }
        },
        None => None,
    };

    // Single UPDATE with COALESCE — only provided fields change
    let result = sqlx::query(
        r#"UPDATE users SET
             username = COALESCE($2, username),
             email = COALESCE($3, email),
             display_name = COALESCE($4, display_name),
             is_active = COALESCE($5, is_active),
             updated_at = now()
           WHERE id = $1"#,
    )
    .bind(id)
    .bind(&body.username)
    .bind(&body.email)
    .bind(&body.display_name)
    .bind(body.is_active)
    .execute(&state.db)
    .await;

    match result {
        Ok(r) if r.rows_affected() == 0 => (StatusCode::NOT_FOUND, "user not found").into_response(),
        Ok(_) => {
            if let Some(hash) = access_secret_hash {
                match sqlx::query(
                    "UPDATE user_credentials SET access_secret_hash = $2, updated_at = now() WHERE user_id = $1",
                )
                .bind(id)
                .bind(&hash)
                .execute(&state.db)
                .await
                {
                    Ok(r) if r.rows_affected() == 0 => {
                        return (
                            StatusCode::CONFLICT,
                            Json(serde_json::json!({"error": "user has no local credentials to set a password for"})),
                        ).into_response();
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::error!(%e, %id, "update_user: credential update failed");
                        return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
                    }
                }
            }

            // Mirror deactivate's token revocation: stale JWTs must stop working
            // immediately, not linger until natural expiry.
            if body.is_active == Some(false) {
                let _ = state.auth.revoke_tokens_for_user(&id.to_string()).await;
            }

            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            if e.to_string().contains("duplicate key") {
                (StatusCode::CONFLICT, "username or email already taken").into_response()
            } else {
                tracing::error!(%e, %id, "update_user: db error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
            }
        }
    }
}

async fn delete_user(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    // Prevent self-deletion.
    if claims.sub == id.to_string() {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "cannot delete your own account"}))).into_response();
    }

    let is_super: Option<bool> = sqlx::query_scalar("SELECT is_superuser FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    if is_super == Some(true) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "cannot delete superuser"}))).into_response();
    }

    // Prevent deletion if the user owns any non-deleted agents.
    let owned_agents: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agents WHERE owner_id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    if owned_agents > 0 {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "user owns agents — reassign or delete them first",
                "agent_count": owned_agents,
            })),
        ).into_response();
    }

    match sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() == 0 => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "user not found"}))).into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(%e, %id, "delete_user: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "internal error"}))).into_response()
        }
    }
}

// ─── POST /users/{id}/deactivate ────────────────────────────────────────────

async fn deactivate(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    // Prevent self-deactivation.
    if claims.sub == id.to_string() {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "cannot deactivate your own account"}))).into_response();
    }

    // Prevent deactivating the last admin.
    if let Some(err) = check_last_admin(&state, id).await {
        return err;
    }

    match sqlx::query(
        "UPDATE users SET is_active = false WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .execute(&state.db)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            // Revoke all live tokens immediately so the gateway stops accepting them.
            let _ = state.auth.revoke_tokens_for_user(&id.to_string()).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %id, "deactivate: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "internal error"}))).into_response()
        }
    }
}

// ─── POST /users/{id}/reinstate ─────────────────────────────────────────────

async fn reinstate(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match sqlx::query(
        "UPDATE users SET is_active = true WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .execute(&state.db)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %id, "reinstate: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "internal error"}))).into_response()
        }
    }
}

// ─── POST /users/{id}/regenerate-credentials ────────────────────────────────

async fn regenerate_credentials(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let access_key = nasiko_auth::generate_access_key();
    let access_secret = nasiko_auth::generate_access_secret();
    let access_secret_hash = match nasiko_auth::hash_password_async(&access_secret).await {
        Ok(h) => h,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    match sqlx::query(
        r#"INSERT INTO user_credentials (user_id, access_key, access_secret_hash)
           VALUES ($1, $2, $3)
           ON CONFLICT (user_id) DO UPDATE
           SET access_key = EXCLUDED.access_key,
               access_secret_hash = EXCLUDED.access_secret_hash,
               updated_at = now()"#,
    )
    .bind(id)
    .bind(&access_key)
    .bind(&access_secret_hash)
    .execute(&state.db)
    .await
    {
        Ok(_) => {
            let _ = sqlx::query(
                "UPDATE auth_tokens SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL"
            )
            .bind(id)
            .execute(&state.db)
            .await;

            (StatusCode::OK, Json(serde_json::json!({
                "access_key": access_key,
                "access_secret": access_secret,
                "message": "Store access_secret securely — it won't be shown again.",
            }))).into_response()
        }
        Err(e) => {
            tracing::error!(%e, %id, "regenerate_credentials: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "internal error"}))).into_response()
        }
    }
}

// ─── PUT /users/{id}/role ────────────────────────────────────────────────────

/// Change a user's role and immediately revoke their live tokens.
pub async fn change_role(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Json(req): Json<ChangeRoleRequest>,
) -> impl IntoResponse {
    // Cannot change your own role through the admin API.
    if claims.sub == id.to_string() {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "cannot change your own role"}))).into_response();
    }

    let new_role = req.role.trim().to_lowercase();
    let valid_roles = ["admin", "member", "team_member", "team_lead", "department_manager"];
    if !valid_roles.contains(&new_role.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("invalid role '{}'; valid: {}", new_role, valid_roles.join(", "))})),
        ).into_response();
    }

    // Fetch current role — ensures the user exists and enables last-admin guard.
    let current_role: Option<String> = sqlx::query_scalar(
        "SELECT role::text FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let Some(current_role) = current_role else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "user not found"}))).into_response();
    };

    // Last-admin guard: prevent demoting the only remaining admin.
    if current_role == "admin" && new_role != "admin"
        && let Some(err) = check_last_admin(&state, id).await
    {
        return err;
    }

    // No-op if the role hasn't changed.
    if current_role == new_role {
        return StatusCode::NO_CONTENT.into_response();
    }

    match sqlx::query(
        "UPDATE users SET role = $2::user_role, updated_at = now() WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(&new_role)
    .execute(&state.db)
    .await
    {
        Ok(r) if r.rows_affected() == 0 => {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "user not found"}))).into_response();
        }
        Err(e) => {
            tracing::error!(%e, %id, "change_role: db error");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "internal error"}))).into_response();
        }
        Ok(_) => {}
    }

    // Revoke all live tokens — role is embedded in JWT so stale tokens would
    // carry the old (wrong) role until natural expiry.
    let _ = state.auth.revoke_tokens_for_user(&id.to_string()).await;

    StatusCode::NO_CONTENT.into_response()
}

// ─── GET /users/admins ───────────────────────────────────────────────────────

async fn list_admins(State(state): State<AppState>) -> impl IntoResponse {
    #[derive(Serialize, FromRow)]
    struct AdminUser {
        id: Uuid,
        username: String,
        email: Option<String>,
        is_active: bool,
        created_at: DateTime<Utc>,
    }
    match sqlx::query_as::<_, AdminUser>(
        "SELECT id, username, email, is_active, created_at FROM users WHERE role = 'admin' AND deleted_at IS NULL ORDER BY username",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(data) => Json(Paginated::new(data)).into_response(),
        Err(e) => {
            tracing::error!(%e, "list_admins: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

// ─── GET /users/{id}/accessible-agents ──────────────────────────────────────

async fn accessible_agents_for_user(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
) -> impl IntoResponse {
    accessible_agents_impl(&state.db, user_id).await
}

// ─── GET /users/me/accessible-agents ────────────────────────────────────────

async fn my_accessible_agents(
    State(state): State<AppState>,
    claims: Claims,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    accessible_agents_impl(&state.db, user_id).await
}

// ─── GET /users/me ──────────────────────────────────────────────────────────

async fn get_me(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    let result: Result<Option<UserRow>, _> = sqlx::query_as::<_, UserRow>(
        r#"SELECT u.id, u.username, u.email, u.display_name, u.is_superuser,
                  u.is_active, u.role::text as role,
                  u.created_at, u.last_login
           FROM users u
           WHERE u.id = $1 AND u.deleted_at IS NULL"#,
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;

    match result {
        Ok(Some(user)) => Json(user).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "user not found").into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, "get_me: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

// ─── shared helper ──────────────────────────────────────────────────────────

#[derive(Serialize, FromRow)]
struct AccessibleAgent {
    id: Uuid,
    name: String,
    description: Option<String>,
    status: Option<String>,
    owner_id: Option<Uuid>,
}

async fn accessible_agents_impl(db: &sqlx::PgPool, user_id: Uuid) -> axum::response::Response {
    // OSS: owner, public, or a direct user grant.
    // EE overrides this in ee/server/src/users.rs to also check team and department grants.
    let rows = sqlx::query_as::<_, AccessibleAgent>(
        r#"SELECT DISTINCT a.id, a.name, a.description, a.status, a.owner_id
           FROM agents a
           WHERE a.deleted_at IS NULL
             AND (
               a.owner_id = $1
               OR a.is_public = true
               OR EXISTS (
                   SELECT 1 FROM agent_grants ag
                   WHERE ag.agent_id = a.id
                     AND ag.grant_type = 'user' AND ag.grantee_id = $1::text
               )
             )
           ORDER BY a.name"#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await;

    match rows {
        Ok(data) => Json(Paginated::new(data)).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, "accessible_agents: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}