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

use crate::state::AppState;
use crate::Paginated;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users", post(create_user))
        .route("/users/{id}", get(get_user))
        .route("/users/{id}", put(update_user))
        .route("/users/{id}", delete(delete_user))
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
                      u.is_active, tm.role::text as role, u.created_at, u.last_login
               FROM users u
               LEFT JOIN team_members tm ON tm.user_id = u.id
               WHERE u.username ILIKE $1 OR u.email ILIKE $1 OR u.display_name ILIKE $1
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
                      u.is_active, tm.role::text as role, u.created_at, u.last_login
               FROM users u
               LEFT JOIN team_members tm ON tm.user_id = u.id
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
            let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
                .fetch_one(&state.db)
                .await
                .unwrap_or(0);
            Json(Paginated { data, total: total as usize }).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_user(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let result: Result<Option<UserRow>, _> = sqlx::query_as::<_, UserRow>(
        r#"SELECT u.id, u.username, u.email, u.display_name, u.is_superuser,
                  u.is_active, tm.role::text as role, u.created_at, u.last_login
           FROM users u
           LEFT JOIN team_members tm ON tm.user_id = u.id
           WHERE u.id = $1"#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;

    match result {
        Ok(Some(user)) => Json(user).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "user not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct CreateUser {
    username: String,
    email: String,
    password: String,
    display_name: Option<String>,
    #[allow(dead_code)]
    role: Option<String>,
}

async fn create_user(
    State(state): State<AppState>,
    Json(body): Json<CreateUser>,
) -> impl IntoResponse {
    if body.password.len() < 8 {
        return (StatusCode::BAD_REQUEST, "password must be at least 8 characters").into_response();
    }

    let password_hash = match bcrypt::hash(&body.password, bcrypt::DEFAULT_COST) {
        Ok(h) => h,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let id = Uuid::new_v4();
    let result = sqlx::query(
        r#"INSERT INTO users (id, username, email, password_hash, display_name, is_superuser, is_active)
           VALUES ($1, $2, $3, $4, $5, false, true)"#,
    )
    .bind(id)
    .bind(&body.username)
    .bind(&body.email)
    .bind(&password_hash)
    .bind(&body.display_name)
    .execute(&state.db)
    .await;

    match result {
        Ok(_) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(e) => {
            if e.to_string().contains("duplicate key") {
                (StatusCode::CONFLICT, "username or email already exists").into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            }
        }
    }
}

#[derive(Deserialize)]
struct UpdateUser {
    username: Option<String>,
    email: Option<String>,
    display_name: Option<String>,
    password: Option<String>,
    is_active: Option<bool>,
    #[allow(dead_code)]
    role: Option<String>,
}

async fn update_user(
    State(state): State<AppState>,
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
    if let Some(ref p) = body.password {
        if p.len() < 8 {
            return (StatusCode::BAD_REQUEST, "password must be at least 8 characters").into_response();
        }
    }

    // Hash password if provided
    let password_hash = match &body.password {
        Some(p) => match bcrypt::hash(p, bcrypt::DEFAULT_COST) {
            Ok(h) => Some(h),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        None => None,
    };

    // Single UPDATE with COALESCE — only provided fields change
    let result = sqlx::query(
        r#"UPDATE users SET
             username = COALESCE($2, username),
             email = COALESCE($3, email),
             display_name = COALESCE($4, display_name),
             password_hash = COALESCE($5, password_hash),
             is_active = COALESCE($6, is_active),
             updated_at = now()
           WHERE id = $1"#,
    )
    .bind(id)
    .bind(&body.username)
    .bind(&body.email)
    .bind(&body.display_name)
    .bind(&password_hash)
    .bind(body.is_active)
    .execute(&state.db)
    .await;

    match result {
        Ok(r) if r.rows_affected() == 0 => (StatusCode::NOT_FOUND, "user not found").into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            if e.to_string().contains("duplicate key") {
                (StatusCode::CONFLICT, "username or email already taken").into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            }
        }
    }
}

async fn delete_user(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let is_super: Option<bool> = sqlx::query_scalar("SELECT is_superuser FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    if is_super == Some(true) {
        return (StatusCode::FORBIDDEN, "cannot delete superuser").into_response();
    }

    match sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() == 0 => (StatusCode::NOT_FOUND, "user not found").into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
