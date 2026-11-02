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
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::Paginated;
use crate::auth::Claims;
use crate::state::AppState;

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
                (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({"error": "cannot remove the last admin"})),
                )
                    .into_response(),
            );
        }
    }
    None
}

/// The superuser ceiling: a caller who is not a superuser may not grant
/// superuser, and may not act on an account that already is one.
///
/// User management is open to `role = 'admin'` callers (and in OSS, whose
/// `can_manage_users` is allow-all, to every authenticated caller). Without
/// this check such a caller could create a superuser, or reset a superuser's
/// credentials and sign in as them.
///
/// Returns `None` when the request may go ahead. A target that does not exist
/// passes, so the handler still answers 404 itself. A database error fails
/// closed (500).
async fn superuser_ceiling(
    state: &AppState,
    claims: &Claims,
    grants_superuser: bool,
    target_id: Option<Uuid>,
) -> Option<axum::response::Response> {
    if claims.is_superuser {
        return None;
    }
    let forbidden = || {
        Some(
            (
                StatusCode::FORBIDDEN,
                Json(
                    serde_json::json!({"error": "only a superuser can manage superuser accounts"}),
                ),
            )
                .into_response(),
        )
    };
    if grants_superuser {
        return forbidden();
    }
    let target_id = target_id?;
    let target_is_superuser: Result<Option<bool>, sqlx::Error> =
        sqlx::query_scalar("SELECT is_superuser FROM users WHERE id = $1")
            .bind(target_id)
            .fetch_optional(&state.db)
            .await;
    match target_is_superuser {
        Ok(Some(true)) => forbidden(),
        Ok(_) => None,
        Err(e) => {
            tracing::error!(%e, %target_id, "superuser_ceiling: target lookup failed");
            Some(
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "internal error"})),
                )
                    .into_response(),
            )
        }
    }
}

/// Revoke every live session for `user_id`.
///
/// Called wherever a credential is replaced: a password that no longer exists
/// must not leave behind sessions that were established with it. Best-effort by
/// design — the credential change has already committed by the time this runs,
/// so a revocation failure is logged rather than turned into a failed request
/// that would wrongly suggest the change did not happen.
///
/// Enforced in both editions: `auth::middleware::validate_session_token` runs its
/// own fail-closed `auth_tokens` lookup on every authenticated request,
/// independently of whichever `AuthService` impl is wired in.
pub(crate) async fn revoke_sessions(state: &AppState, user_id: Uuid) {
    if let Err(e) = state
        .auth
        .revoke_tokens_for_user(&user_id.to_string())
        .await
    {
        tracing::warn!(%e, %user_id, "failed to revoke sessions after a credential change");
    }
}

/// Full user orchestrator — list, get, and all management routes including role changes.
/// Used by the OSS server. EE provides its own orchestrator
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
        .route(
            "/users/{id}/accessible-agents",
            get(accessible_agents_for_user),
        )
}

/// Management-only orchestrator — create/delete users and related operations.
/// The role-change and update endpoints are registered separately (in `router()`)
/// so each can be overridden without causing a duplicate-route panic: EE wraps
/// `change_role` with its leadership cascade, and wraps `update_user` to also
/// accept `department_id`/`team_id` (EE-only columns `oss/server`'s `users`
/// table doesn't have — see the EE `ee_update_user` override).
pub fn management_router() -> Router<AppState> {
    Router::new()
        .route("/users/admins", get(list_admins))
        .route("/users", post(create_user))
        .route("/users/{id}", delete(delete_user))
        .route("/users/{id}/deactivate", post(deactivate))
        .route("/users/{id}/reinstate", post(reinstate))
        .route(
            "/users/{id}/regenerate-credentials",
            post(regenerate_credentials),
        )
}

#[derive(Debug, Serialize, ToSchema, FromRow)]
pub(crate) struct UserRow {
    id: Uuid,
    username: String,
    email: String,
    display_name: Option<String>,
    is_superuser: bool,
    is_active: bool,
    role: Option<String>,
    #[sqlx(default)]
    department_id: Option<Uuid>,
    #[sqlx(default)]
    team_id: Option<Uuid>,
    created_at: DateTime<Utc>,
    last_login: Option<DateTime<Utc>>,
}

#[derive(Deserialize, IntoParams)]
pub(crate) struct ListQuery {
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

/// Response envelope for `GET /users` — the JSON shape of `Paginated<UserRow>`,
/// spelled out concretely since `Paginated<T>`'s generic isn't monomorphized
/// for `ToSchema` here.
#[derive(Serialize, ToSchema)]
pub(crate) struct UserListResponse {
    data: Vec<UserRow>,
    total: usize,
}

/// List users (superuser-only; EE additionally exposes a role/org-scoped
/// listing at `/api/org/users` — see the EE org-users routes).
#[utoipa::path(
    get,
    path = "/api/users",
    tag = "users",
    params(ListQuery),
    responses(
        (status = 200, description = "Users, newest first", body = UserListResponse),
    ),
)]
pub(crate) async fn list_users(
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
            Json(Paginated {
                data,
                total: total as usize,
            })
            .into_response()
        }
        Err(e) => {
            tracing::error!(%e, "list_users: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

/// Get a user by id (superuser-only).
#[utoipa::path(
    get,
    path = "/api/users/{id}",
    tag = "users",
    params(
        ("id" = Uuid, Path, description = "User id"),
    ),
    responses(
        (status = 200, description = "User", body = UserRow),
        (status = 404, description = "No such user"),
    ),
)]
pub(crate) async fn get_user(
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

#[derive(Deserialize, ToSchema)]
pub(crate) struct CreateUser {
    username: String,
    email: String,
    display_name: Option<String>,
    role: Option<String>,
    /// Separate from `role` — `role` is a `user_role` enum value
    /// (admin/department_manager/team_lead/team_member/member), never
    /// "superuser". This is the one flag that actually grants unrestricted
    /// access (bypasses org-visibility scoping, gates superuser-only routes
    /// like MCP toolkit registration) — see CLAUDE.md's `role` vs
    /// `is_superuser` note.
    #[serde(default)]
    is_superuser: bool,
}

/// One-time credential material — the only time `access_secret` is ever
/// returned; the server stores only its hash from here on.
#[derive(Serialize, ToSchema)]
pub(crate) struct CreateUserResponse {
    id: Uuid,
    username: String,
    access_key: String,
    access_secret: String,
    message: String,
}

/// Create a user. No password is taken — a one-time `access_key`/
/// `access_secret` pair is minted and returned once; the secret doubles as
/// the user's login password (see `nasiko-user-creation-contract`).
#[utoipa::path(
    post,
    path = "/api/users",
    tag = "users",
    request_body = CreateUser,
    responses(
        (status = 201, description = "User created", body = CreateUserResponse),
        (status = 403, description = "Only a superuser can create a superuser"),
        (status = 409, description = "Username or email already exists"),
    ),
)]
pub(crate) async fn create_user(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateUser>,
) -> impl IntoResponse {
    if let Some(err) = superuser_ceiling(&state, &claims, body.is_superuser, None).await {
        return err;
    }
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
           VALUES ($1, $2, $3, $4, $5, true, $6::user_role)"#,
    )
    .bind(id)
    .bind(&body.username)
    .bind(&body.email)
    .bind(&body.display_name)
    .bind(body.is_superuser)
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
                )
                    .into_response(),
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

#[derive(Deserialize, Default, ToSchema)]
pub struct UpdateUser {
    pub username: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub password: Option<String>,
    pub is_active: Option<bool>,
    /// Basic role update without cascade (use /role for leadership promotions).
    pub role: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct ChangeRoleRequest {
    pub role: String,
}

/// Update a user's own-editable fields (superuser-only; EE overrides this
/// route to additionally accept `department_id`/`team_id` — see
/// the EE `ee_update_user` override). An `is_active: false`
/// transition here runs the same self-deactivate/last-admin guards as the
/// dedicated `/deactivate` route.
#[utoipa::path(
    put,
    path = "/api/users/{id}",
    tag = "users",
    params(
        ("id" = Uuid, Path, description = "User id"),
    ),
    request_body = UpdateUser,
    responses(
        (status = 200, description = "Updated user", body = UserRow),
        (status = 400, description = "Empty username/email or password too short"),
        (status = 403, description = "Cannot deactivate your own account, or the target is a superuser and the caller is not"),
        (status = 404, description = "No such user"),
        (status = 409, description = "Username/email already taken, cannot deactivate the last admin, or user has no local credentials to set a password for"),
    ),
)]
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
    // Same policy as the self-service route: an administrator setting someone
    // else's password must not be able to set one that user could not have
    // chosen themselves.
    if let Some(ref p) = body.password
        && let Err(policy) = nasiko_auth::validate_password(p)
    {
        return (StatusCode::BAD_REQUEST, policy.message()).into_response();
    }

    if let Some(err) = superuser_ceiling(&state, &claims, false, Some(id)).await {
        return err;
    }

    // AUTH-2: an `is_active: false` transition through this generic PUT must go
    // through the exact same guards as the dedicated /deactivate route — otherwise
    // a caller could deactivate the last admin, or themselves, by smuggling
    // `is_active: false` into an update instead of using /deactivate.
    if body.is_active == Some(false) {
        if claims.sub == id.to_string() {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": "cannot deactivate your own account"})),
            )
                .into_response();
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

    // Validate role if provided.
    if let Some(ref r) = body.role {
        let valid = [
            "admin",
            "member",
            "team_member",
            "team_lead",
            "department_manager",
        ];
        if !valid.contains(&r.as_str()) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("invalid role '{r}'")})),
            )
                .into_response();
        }
    }

    // Shared (edition-agnostic) columns only. department_id/team_id are
    // EE-only columns (created by an EE migration) — naming them here made
    // this statement fail on every pure-OSS database, 500ing all user
    // updates. Org placement is layered on by `ee_update_user`
    // (the EE users module), which delegates the shared fields here first.
    // UserRow's #[sqlx(default)] covers the columns RETURNING no longer names.
    let result = sqlx::query_as::<_, UserRow>(
        r#"UPDATE users SET
             username     = COALESCE($2, username),
             email        = COALESCE($3, email),
             display_name = COALESCE($4, display_name),
             is_active    = COALESCE($5, is_active),
             role         = COALESCE($6::user_role, role),
             updated_at   = now()
           WHERE id = $1 AND deleted_at IS NULL
           RETURNING id, username, email, display_name, is_superuser,
                     is_active, role::text AS role,
                     created_at, last_login"#,
    )
    .bind(id)
    .bind(&body.username)
    .bind(&body.email)
    .bind(&body.display_name)
    .bind(body.is_active)
    .bind(body.role.as_deref())
    .fetch_optional(&state.db)
    .await;

    match result {
        Ok(None) => (StatusCode::NOT_FOUND, "user not found").into_response(),
        Ok(Some(updated)) => {
            let password_changed = access_secret_hash.is_some();
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
            // A replaced password and a deactivated account both invalidate every
            // session the old credential established. Previously only the
            // deactivation branch revoked, so an admin resetting a compromised
            // user's password left the attacker's existing session alive.
            if password_changed || body.is_active == Some(false) {
                revoke_sessions(&state, id).await;
            }
            Json(updated).into_response()
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

/// Delete a user. Rejects self-deletion, deleting the last admin, or a user
/// who still owns agents (reassign or delete those first).
#[utoipa::path(
    delete,
    path = "/api/users/{id}",
    tag = "users",
    params(
        ("id" = Uuid, Path, description = "User id"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Cannot delete your own account, or the target is a superuser and the caller is not"),
        (status = 409, description = "Cannot delete the last admin"),
        (status = 404, description = "No such user"),
        (status = 409, description = "User still owns agents"),
    ),
)]
pub(crate) async fn delete_user(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    // Prevent self-deletion.
    if claims.sub == id.to_string() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "cannot delete your own account"})),
        )
            .into_response();
    }

    if let Some(err) = superuser_ceiling(&state, &claims, false, Some(id)).await {
        return err;
    }

    // Prevent deleting the last admin (covers both role='admin' and is_superuser).
    if let Some(err) = check_last_admin(&state, id).await {
        return err;
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
        )
            .into_response();
    }

    match sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() == 0 => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "user not found"})),
        )
            .into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(%e, %id, "delete_user: db error");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── POST /users/{id}/deactivate ────────────────────────────────────────────

/// Deactivate a user and revoke their live tokens. Rejects self-deactivation
/// and deactivating the last admin.
#[utoipa::path(
    post,
    path = "/api/users/{id}/deactivate",
    tag = "users",
    params(
        ("id" = Uuid, Path, description = "User id"),
    ),
    responses(
        (status = 204, description = "Deactivated"),
        (status = 403, description = "Cannot deactivate your own account, or the target is a superuser and the caller is not"),
        (status = 404, description = "No such user"),
        (status = 409, description = "Cannot deactivate the last admin"),
    ),
)]
pub(crate) async fn deactivate(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    // Prevent self-deactivation.
    if claims.sub == id.to_string() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "cannot deactivate your own account"})),
        )
            .into_response();
    }

    if let Some(err) = superuser_ceiling(&state, &claims, false, Some(id)).await {
        return err;
    }

    // Prevent deactivating the last admin.
    if let Some(err) = check_last_admin(&state, id).await {
        return err;
    }

    match sqlx::query("UPDATE users SET is_active = false WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            // Revoke all live tokens immediately so the gateway stops accepting them.
            revoke_sessions(&state, id).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %id, "deactivate: db error");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── POST /users/{id}/reinstate ─────────────────────────────────────────────

/// Reactivate a previously deactivated user.
#[utoipa::path(
    post,
    path = "/api/users/{id}/reinstate",
    tag = "users",
    params(
        ("id" = Uuid, Path, description = "User id"),
    ),
    responses(
        (status = 204, description = "Reinstated"),
        (status = 403, description = "The target is a superuser and the caller is not"),
        (status = 404, description = "No such user"),
    ),
)]
pub(crate) async fn reinstate(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Some(err) = superuser_ceiling(&state, &claims, false, Some(id)).await {
        return err;
    }
    match sqlx::query("UPDATE users SET is_active = true WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %id, "reinstate: db error");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── POST /users/{id}/regenerate-credentials ────────────────────────────────

/// One-time credential material — the only time `access_secret` is ever
/// returned; the server stores only its hash from here on.
#[derive(Serialize, ToSchema)]
pub(crate) struct RegenerateCredentialsResponse {
    access_key: String,
    access_secret: String,
    message: String,
}

/// Mint a fresh `access_key`/`access_secret` pair for a user, replacing any
/// existing credential, and revoke their live tokens.
#[utoipa::path(
    post,
    path = "/api/users/{id}/regenerate-credentials",
    tag = "users",
    params(
        ("id" = Uuid, Path, description = "User id"),
    ),
    responses(
        (status = 200, description = "New credentials", body = RegenerateCredentialsResponse),
        (status = 403, description = "The target is a superuser and the caller is not"),
    ),
)]
pub(crate) async fn regenerate_credentials(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Some(err) = superuser_ceiling(&state, &claims, false, Some(id)).await {
        return err;
    }
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
            revoke_sessions(&state, id).await;

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "access_key": access_key,
                    "access_secret": access_secret,
                    "message": "Store access_secret securely — it won't be shown again.",
                })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!(%e, %id, "regenerate_credentials: db error");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── PUT /users/{id}/role ────────────────────────────────────────────────────

/// The `user_role` values this database actually has, asked of Postgres.
///
/// A hardcoded list cannot be right here, because the set of roles is
/// edition-dependent: OSS's schema declares `('admin', 'member')` and
/// enterprise migrations `ALTER TYPE user_role ADD VALUE` on top. The list
/// this replaced had drifted in both directions at once — it named three
/// roles that no longer exist in either edition, and omitted one that does,
/// so an OSS-only deployment accepted a role it could not store (passing
/// validation, then failing the `::user_role` cast as a 500 rather than a
/// 400) while an enterprise build had a valid role rejected on its way
/// through this function.
///
/// Asking the database is correct in every edition, stays correct when a
/// migration changes the enum, and — the point — lets this stay ignorant of
/// any role name that isn't in OSS's own schema. Callers layer their own
/// policy on top: enterprise narrows this to the roles it wants offered,
/// since Postgres cannot remove an enum value once added, so retired names
/// linger in `enum_range` forever.
///
/// One extra round trip on a rare administrative action; not worth caching.
async fn valid_user_roles(state: &AppState) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT unnest(enum_range(NULL::user_role))::text")
        .fetch_all(&state.db)
        .await
}

/// Change a user's role and immediately revoke their live tokens. EE wraps
/// this with a leadership-displacement cascade — see
/// the EE `ee_change_role` override.
#[utoipa::path(
    put,
    path = "/api/users/{id}/role",
    tag = "users",
    params(
        ("id" = Uuid, Path, description = "User id"),
    ),
    request_body = ChangeRoleRequest,
    responses(
        (status = 204, description = "Role changed (or unchanged, a no-op)"),
        (status = 400, description = "Invalid role"),
        (status = 403, description = "Cannot change your own role, or the target is a superuser and the caller is not"),
        (status = 404, description = "No such user"),
        (status = 409, description = "Cannot demote the last admin"),
    ),
)]
pub async fn change_role(
    State(state): State<AppState>,
    claims: Claims,
    Path(id): Path<Uuid>,
    Json(req): Json<ChangeRoleRequest>,
) -> impl IntoResponse {
    // Cannot change your own role through the admin API.
    if claims.sub == id.to_string() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "cannot change your own role"})),
        )
            .into_response();
    }

    if let Some(err) = superuser_ceiling(&state, &claims, false, Some(id)).await {
        return err;
    }

    let new_role = req.role.trim().to_lowercase();
    let valid_roles = match valid_user_roles(&state).await {
        Ok(roles) => roles,
        Err(e) => {
            tracing::error!(%e, "change_role: could not read the user_role enum");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
                .into_response();
        }
    };
    if !valid_roles.iter().any(|r| r == &new_role) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("invalid role '{}'; valid: {}", new_role, valid_roles.join(", "))})),
        ).into_response();
    }

    // Fetch current role — ensures the user exists and enables last-admin guard.
    let current_role: Option<String> =
        sqlx::query_scalar("SELECT role::text FROM users WHERE id = $1 AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    let Some(current_role) = current_role else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "user not found"})),
        )
            .into_response();
    };

    // Last-admin guard: prevent demoting the only remaining admin.
    if current_role == "admin"
        && new_role != "admin"
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
    revoke_sessions(&state, id).await;

    StatusCode::NO_CONTENT.into_response()
}

// ─── GET /users/admins ───────────────────────────────────────────────────────

#[derive(Serialize, ToSchema, FromRow)]
pub(crate) struct AdminUser {
    id: Uuid,
    username: String,
    email: Option<String>,
    is_active: bool,
    created_at: DateTime<Utc>,
}

/// Response envelope for `GET /users/admins` — the JSON shape of
/// `Paginated<AdminUser>` spelled out concretely (see `UserListResponse`).
#[derive(Serialize, ToSchema)]
pub(crate) struct AdminListResponse {
    data: Vec<AdminUser>,
    total: usize,
}

/// List every admin (`role = 'admin'`) user.
#[utoipa::path(
    get,
    path = "/api/users/admins",
    tag = "users",
    responses(
        (status = 200, description = "Admin users", body = AdminListResponse),
    ),
)]
pub(crate) async fn list_admins(State(state): State<AppState>) -> impl IntoResponse {
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

/// Agents accessible to a user (owner ∪ public ∪ direct user-grant; EE's
/// EE override additionally checks team/department
/// grants).
#[utoipa::path(
    get,
    path = "/api/users/{id}/accessible-agents",
    tag = "users",
    params(
        ("id" = Uuid, Path, description = "User id"),
    ),
    responses(
        (status = 200, description = "Accessible agents", body = AccessibleAgentsResponse),
    ),
)]
pub(crate) async fn accessible_agents_for_user(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
) -> impl IntoResponse {
    accessible_agents_impl(&state.db, user_id).await
}

// ─── GET /users/me/accessible-agents ────────────────────────────────────────

/// Agents accessible to the caller — see `accessible_agents_for_user`.
#[utoipa::path(
    get,
    path = "/api/users/me/accessible-agents",
    tag = "users",
    responses(
        (status = 200, description = "Accessible agents", body = AccessibleAgentsResponse),
    ),
)]
pub(crate) async fn my_accessible_agents(
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

/// The caller's own user record (superuser-only route; EE overrides this
/// with the same EE-aware shape as `get_user` — see the EE users module).
#[utoipa::path(
    get,
    path = "/api/users/me",
    tag = "users",
    responses(
        (status = 200, description = "The caller's user record", body = UserRow),
        (status = 404, description = "No such user"),
    ),
)]
pub(crate) async fn get_me(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
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

#[derive(Serialize, ToSchema, FromRow)]
pub(crate) struct AccessibleAgent {
    id: Uuid,
    name: String,
    description: Option<String>,
    status: Option<String>,
    owner_id: Option<Uuid>,
    // Consumed by the EE CLI's access my-agents/user-agents PUBLIC column.
    is_public: bool,
}

/// Response envelope for the `accessible-agents` routes — the JSON shape of
/// `Paginated<AccessibleAgent>` spelled out concretely (see `UserListResponse`).
#[derive(Serialize, ToSchema)]
pub(crate) struct AccessibleAgentsResponse {
    data: Vec<AccessibleAgent>,
    total: usize,
}

async fn accessible_agents_impl(db: &sqlx::PgPool, user_id: Uuid) -> axum::response::Response {
    // OSS: owner, public, or a direct user grant.
    // EE overrides this to also check team and department grants.
    let rows = sqlx::query_as::<_, AccessibleAgent>(
        r#"SELECT DISTINCT a.id, a.name, a.description, a.status, a.owner_id, a.is_public
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
