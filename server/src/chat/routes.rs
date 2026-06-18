use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

use super::models::*;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/chat/sessions", get(list_sessions).post(create_session))
        .route(
            "/chat/sessions/{session_id}",
            get(get_session).put(update_session).delete(delete_session),
        )
        .route(
            "/chat/sessions/{session_id}/messages",
            get(list_messages).post(send_message),
        )
}

// ─── Sessions ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ListParams {
    limit: Option<i64>,
    offset: Option<i64>,
    agent_id: Option<Uuid>,
}

async fn list_sessions(
    State(state): State<AppState>,
    claims: Claims,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let limit = params.limit.unwrap_or(50).min(100);
    let offset = params.offset.unwrap_or(0);

    let sessions = if let Some(agent_id) = params.agent_id {
        sqlx::query_as::<_, ChatSessionView>(
            r#"SELECT cs.*,
                      a.name as agent_name,
                      (SELECT content FROM chat_messages WHERE session_id = cs.session_id ORDER BY timestamp DESC LIMIT 1) as last_message
               FROM chat_sessions cs
               LEFT JOIN agents a ON a.id = cs.agent_id
               WHERE cs.user_id = $1 AND cs.agent_id = $2
               ORDER BY cs.updated_at DESC
               LIMIT $3 OFFSET $4"#,
        )
        .bind(user_id)
        .bind(agent_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, ChatSessionView>(
            r#"SELECT cs.*,
                      a.name as agent_name,
                      (SELECT content FROM chat_messages WHERE session_id = cs.session_id ORDER BY timestamp DESC LIMIT 1) as last_message
               FROM chat_sessions cs
               LEFT JOIN agents a ON a.id = cs.agent_id
               WHERE cs.user_id = $1
               ORDER BY cs.updated_at DESC
               LIMIT $2 OFFSET $3"#,
        )
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
    };

    match sessions {
        Ok(s) => Json(s).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn create_session(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateSession>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let session_id = format!("ses_{}", Uuid::new_v4().simple());
    let title = body.title.unwrap_or_else(|| "New chat".into());

    let result = sqlx::query_as::<_, ChatSession>(
        r#"INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title)
           VALUES ($1, $2, $3, $4, $5)
           RETURNING *"#,
    )
    .bind(&session_id)
    .bind(user_id)
    .bind(body.agent_id)
    .bind(&body.agent_url)
    .bind(&title)
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(session) => (StatusCode::CREATED, Json(session)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_session(
    State(state): State<AppState>,
    claims: Claims,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    match sqlx::query_as::<_, ChatSession>(
        "SELECT * FROM chat_sessions WHERE session_id = $1 AND user_id = $2",
    )
    .bind(&session_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(s)) => Json(s).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn update_session(
    State(state): State<AppState>,
    claims: Claims,
    Path(session_id): Path<String>,
    Json(body): Json<UpdateSession>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let result = sqlx::query_as::<_, ChatSession>(
        r#"UPDATE chat_sessions
           SET title = COALESCE($3, title), updated_at = now()
           WHERE session_id = $1 AND user_id = $2
           RETURNING *"#,
    )
    .bind(&session_id)
    .bind(user_id)
    .bind(&body.title)
    .fetch_optional(&state.db)
    .await;

    match result {
        Ok(Some(s)) => Json(s).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn delete_session(
    State(state): State<AppState>,
    claims: Claims,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let result = sqlx::query(
        "DELETE FROM chat_sessions WHERE session_id = $1 AND user_id = $2",
    )
    .bind(&session_id)
    .bind(user_id)
    .execute(&state.db)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── Messages ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct MessageParams {
    limit: Option<i64>,
    before: Option<chrono::DateTime<chrono::Utc>>,
}

async fn list_messages(
    State(state): State<AppState>,
    claims: Claims,
    Path(session_id): Path<String>,
    Query(params): Query<MessageParams>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    // Verify session ownership
    let owns = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE session_id = $1 AND user_id = $2)",
    )
    .bind(&session_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);

    if !owns {
        return StatusCode::NOT_FOUND.into_response();
    }

    let limit = params.limit.unwrap_or(100).min(500);

    let messages = if let Some(before) = params.before {
        sqlx::query_as::<_, ChatMessage>(
            r#"SELECT * FROM chat_messages
               WHERE session_id = $1 AND timestamp < $2
               ORDER BY timestamp ASC
               LIMIT $3"#,
        )
        .bind(&session_id)
        .bind(before)
        .bind(limit)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, ChatMessage>(
            r#"SELECT * FROM chat_messages
               WHERE session_id = $1
               ORDER BY timestamp ASC
               LIMIT $2"#,
        )
        .bind(&session_id)
        .bind(limit)
        .fetch_all(&state.db)
        .await
    };

    match messages {
        Ok(m) => Json(m).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn send_message(
    State(state): State<AppState>,
    claims: Claims,
    Path(session_id): Path<String>,
    Json(body): Json<SendMessage>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    // Verify session ownership
    let owns = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE session_id = $1 AND user_id = $2)",
    )
    .bind(&session_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);

    if !owns {
        return StatusCode::NOT_FOUND.into_response();
    }

    let file_parts_json = body.file_parts.map(sqlx::types::Json);

    let result = sqlx::query_as::<_, ChatMessage>(
        r#"INSERT INTO chat_messages (session_id, role, content, file_parts)
           VALUES ($1, $2, $3, $4)
           RETURNING *"#,
    )
    .bind(&session_id)
    .bind(&body.role)
    .bind(&body.content)
    .bind(&file_parts_json)
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(msg) => {
            if let Err(e) = sqlx::query(
                "UPDATE chat_sessions SET updated_at = now() WHERE session_id = $1",
            )
            .bind(&session_id)
            .execute(&state.db)
            .await
            {
                tracing::warn!(session_id, %e, "failed to touch session updated_at");
            }

            (StatusCode::CREATED, Json(msg)).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
