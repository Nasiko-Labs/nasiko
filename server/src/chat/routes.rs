use axum::{
    Json, Router,
    extract::{Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

use super::models::*;

const MAX_FILES_PER_UPLOAD: usize = 10;
const MAX_FILE_BYTES: usize = 50 * 1024 * 1024; // 50 MB

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
        .route(
            "/chat/sessions/{session_id}/files",
            axum::routing::post(upload_files),
        )
        .route(
            "/chat/sessions/{session_id}/messages/{message_id}/files",
            get(list_message_files),
        )
        .route("/chat/files/{file_id}/download", get(download_file))
        .route("/chat/files/{file_id}", axum::routing::delete(delete_file))
}

// ─── Cursor helpers ──────────────────────────────────────────────────────────

fn encode_cursor(ts: DateTime<Utc>, id: &str) -> String {
    let nanos = ts.timestamp_nanos_opt().unwrap_or(0);
    URL_SAFE_NO_PAD.encode(format!("{nanos}:{id}"))
}

fn decode_cursor(s: &str) -> Option<(DateTime<Utc>, String)> {
    let raw = URL_SAFE_NO_PAD.decode(s).ok()?;
    let txt = std::str::from_utf8(&raw).ok()?;
    let (nanos_str, id) = txt.split_once(':')?;
    let nanos: i64 = nanos_str.parse().ok()?;
    Some((DateTime::from_timestamp_nanos(nanos), id.to_owned()))
}

// ─── Sessions ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ListSessionsParams {
    #[serde(default = "default_session_limit")]
    limit: i64,
    cursor: Option<String>,
    agent_id: Option<Uuid>,
}
fn default_session_limit() -> i64 { 50 }

async fn list_sessions(
    State(state): State<AppState>,
    claims: Claims,
    Query(params): Query<ListSessionsParams>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let limit = params.limit.clamp(1, 100);
    let fetch = limit + 1;

    // Decode cursor into (timestamp, session_id) keyset anchor.
    let cursor_anchor = params.cursor.as_deref().and_then(decode_cursor);

    let mut rows: Vec<ChatSessionView> = match (cursor_anchor, params.agent_id) {
        (None, None) => sqlx::query_as::<_, ChatSessionView>(
            r#"SELECT cs.*,
                      a.name as agent_name,
                      (SELECT content FROM chat_messages WHERE session_id = cs.session_id ORDER BY timestamp DESC LIMIT 1) as last_message
               FROM chat_sessions cs
               LEFT JOIN agents a ON a.id = cs.agent_id
               WHERE cs.user_id = $1
               ORDER BY cs.updated_at DESC, cs.session_id DESC
               LIMIT $2"#,
        )
        .bind(user_id)
        .bind(fetch)
        .fetch_all(&state.db)
        .await,

        (None, Some(agent_id)) => sqlx::query_as::<_, ChatSessionView>(
            r#"SELECT cs.*,
                      a.name as agent_name,
                      (SELECT content FROM chat_messages WHERE session_id = cs.session_id ORDER BY timestamp DESC LIMIT 1) as last_message
               FROM chat_sessions cs
               LEFT JOIN agents a ON a.id = cs.agent_id
               WHERE cs.user_id = $1 AND cs.agent_id = $2
               ORDER BY cs.updated_at DESC, cs.session_id DESC
               LIMIT $3"#,
        )
        .bind(user_id)
        .bind(agent_id)
        .bind(fetch)
        .fetch_all(&state.db)
        .await,

        (Some((cursor_ts, cursor_sid)), None) => sqlx::query_as::<_, ChatSessionView>(
            r#"SELECT cs.*,
                      a.name as agent_name,
                      (SELECT content FROM chat_messages WHERE session_id = cs.session_id ORDER BY timestamp DESC LIMIT 1) as last_message
               FROM chat_sessions cs
               LEFT JOIN agents a ON a.id = cs.agent_id
               WHERE cs.user_id = $1
                 AND (cs.updated_at < $2 OR (cs.updated_at = $2 AND cs.session_id < $3))
               ORDER BY cs.updated_at DESC, cs.session_id DESC
               LIMIT $4"#,
        )
        .bind(user_id)
        .bind(cursor_ts)
        .bind(cursor_sid)
        .bind(fetch)
        .fetch_all(&state.db)
        .await,

        (Some((cursor_ts, cursor_sid)), Some(agent_id)) => sqlx::query_as::<_, ChatSessionView>(
            r#"SELECT cs.*,
                      a.name as agent_name,
                      (SELECT content FROM chat_messages WHERE session_id = cs.session_id ORDER BY timestamp DESC LIMIT 1) as last_message
               FROM chat_sessions cs
               LEFT JOIN agents a ON a.id = cs.agent_id
               WHERE cs.user_id = $1 AND cs.agent_id = $2
                 AND (cs.updated_at < $3 OR (cs.updated_at = $3 AND cs.session_id < $4))
               ORDER BY cs.updated_at DESC, cs.session_id DESC
               LIMIT $5"#,
        )
        .bind(user_id)
        .bind(agent_id)
        .bind(cursor_ts)
        .bind(cursor_sid)
        .bind(fetch)
        .fetch_all(&state.db)
        .await,
    }
    .unwrap_or_else(|_| vec![]);

    let has_more = rows.len() > limit as usize;
    if has_more { rows.pop(); }

    let next_cursor = if has_more {
        rows.last().map(|r| encode_cursor(r.updated_at, &r.session_id))
    } else {
        None
    };
    let prev_cursor = rows.first().map(|r| encode_cursor(r.updated_at, &r.session_id));

    Json(CursorPage { data: rows, has_more, next_cursor, prev_cursor }).into_response()
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
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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

    // FIX: verify ownership BEFORE touching any files — prevents IDOR where an
    // attacker deletes another user's S3 objects then receives 404.
    // Superusers bypass the ownership check (consistent with delete_file/download_file).
    // DB errors surface as 500, not silently as 404.
    if !claims.is_superuser {
        let owned = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE session_id = $1 AND user_id = $2)",
        )
        .bind(&session_id)
        .bind(user_id)
        .fetch_one(&state.db)
        .await;

        match owned {
            Ok(true) => {}
            Ok(false) => return StatusCode::NOT_FOUND.into_response(),
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }

    // Collect S3 keys — propagate DB errors so we never silently orphan file rows.
    let uris: Vec<String> = match sqlx::query_scalar(
        "SELECT storage_uri FROM chat_message_files WHERE session_id = $1",
    )
    .bind(&session_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if !uris.is_empty() {
        // DB-first: match cleanup_uploaded — if DB delete fails we abort before
        // touching S3, so no file rows survive pointing to deleted storage keys.
        if let Err(e) = sqlx::query("DELETE FROM chat_message_files WHERE session_id = $1")
            .bind(&session_id)
            .execute(&state.db)
            .await
        {
            tracing::warn!(%e, session_id, "failed to delete file records — aborting to preserve S3");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        for uri in &uris {
            if let Err(e) = state.oci_storage.delete_blob(uri).await {
                tracing::warn!(session_id, uri, %e, "failed to delete chat file from S3");
            }
        }
    }

    let result = sqlx::query(
        "DELETE FROM chat_sessions WHERE session_id = $1 AND (user_id = $2 OR $3::bool)",
    )
    .bind(&session_id)
    .bind(user_id)
    .bind(claims.is_superuser)
    .execute(&state.db)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ─── Messages ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ListMessagesParams {
    #[serde(default = "default_message_limit")]
    limit: i64,
    before: Option<DateTime<Utc>>,
    after: Option<DateTime<Utc>>,
    prev_cursor: Option<String>,
    next_cursor: Option<String>,
}
fn default_message_limit() -> i64 { 100 }

async fn list_messages(
    State(state): State<AppState>,
    claims: Claims,
    Path(session_id): Path<String>,
    Query(params): Query<ListMessagesParams>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let owns = match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE session_id = $1 AND user_id = $2)",
    )
    .bind(&session_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if !owns {
        return StatusCode::NOT_FOUND.into_response();
    }

    let limit = params.limit.clamp(1, 500);
    let fetch = limit + 1;

    // Cursor tokens take precedence over raw timestamps.
    let before = params.prev_cursor.as_deref()
        .and_then(|c| decode_cursor(c).map(|(ts, _)| ts))
        .or(params.before);
    let after = params.next_cursor.as_deref()
        .and_then(|c| decode_cursor(c).map(|(ts, _)| ts))
        .or(params.after);

    // Fetch DESC in all cases except `after`; reverse in Rust so client always sees ASC.
    let (mut rows, fetched_asc): (Vec<ChatMessage>, bool) = match (before, after) {
        (_, Some(after_ts)) => {
            // Load newer messages going forward.
            let r = sqlx::query_as::<_, ChatMessage>(
                r#"SELECT * FROM chat_messages
                   WHERE session_id = $1 AND timestamp > $2
                   ORDER BY timestamp ASC
                   LIMIT $3"#,
            )
            .bind(&session_id)
            .bind(after_ts)
            .bind(fetch)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
            (r, true)
        }
        (Some(before_ts), None) => {
            // Load older messages going backward.
            let r = sqlx::query_as::<_, ChatMessage>(
                r#"SELECT * FROM chat_messages
                   WHERE session_id = $1 AND timestamp < $2
                   ORDER BY timestamp DESC
                   LIMIT $3"#,
            )
            .bind(&session_id)
            .bind(before_ts)
            .bind(fetch)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
            (r, false)
        }
        (None, None) => {
            // First load: most recent N messages.
            let r = sqlx::query_as::<_, ChatMessage>(
                r#"SELECT * FROM chat_messages
                   WHERE session_id = $1
                   ORDER BY timestamp DESC
                   LIMIT $2"#,
            )
            .bind(&session_id)
            .bind(fetch)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
            (r, false)
        }
    };

    let has_more = rows.len() > limit as usize;
    if has_more { rows.pop(); }

    // Always present to client in ASC (chronological) order.
    if !fetched_asc { rows.reverse(); }

    let out_prev_cursor = rows.first()
        .map(|r| encode_cursor(r.timestamp, &r.id.to_string()));
    let out_next_cursor = if has_more {
        rows.last().map(|r| encode_cursor(r.timestamp, &r.id.to_string()))
    } else {
        None
    };

    Json(CursorPage {
        data: rows,
        has_more,
        next_cursor: out_next_cursor,
        prev_cursor: out_prev_cursor,
    })
    .into_response()
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

    let owns = match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE session_id = $1 AND user_id = $2)",
    )
    .bind(&session_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if !owns {
        return StatusCode::NOT_FOUND.into_response();
    }

    let file_ids = body.file_ids.as_deref().unwrap_or(&[]);
    let has_files = !file_ids.is_empty();
    // FIX: filter out JSON null so it stores as SQL NULL, not 'null'::jsonb.
    // 'null'::jsonb IS NOT NULL in Postgres, which would defeat the AND file_parts IS NULL
    // guard in delete_file and permanently stick has_file_parts = true.
    let file_parts_json = body.file_parts
        .filter(|v| !v.is_null())
        .map(sqlx::types::Json);
    let has_file_parts = has_files || file_parts_json.is_some();

    // FIX: wrap message insert + file claim in a transaction to eliminate the
    // TOCTOU race where two concurrent sends could both pass the unattached check.
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let msg = match sqlx::query_as::<_, ChatMessage>(
        r#"INSERT INTO chat_messages (session_id, role, content, file_parts, has_file_parts)
           VALUES ($1, $2, $3, $4, $5)
           RETURNING *"#,
    )
    .bind(&session_id)
    .bind(&body.role)
    .bind(&body.content)
    .bind(&file_parts_json)
    .bind(has_file_parts)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(m) => m,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if has_files {
        // Atomically claim unattached files; rows_affected < expected means a race was lost.
        let claimed = match sqlx::query(
            "UPDATE chat_message_files SET message_id = $1
             WHERE id = ANY($2) AND session_id = $3 AND message_id IS NULL",
        )
        .bind(msg.id)
        .bind(file_ids)
        .bind(&session_id)
        .execute(&mut *tx)
        .await
        {
            Ok(r) => r.rows_affected() as usize,
            Err(_) => {
                let _ = tx.rollback().await;
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

        if claimed != file_ids.len() {
            let _ = tx.rollback().await;
            return (
                StatusCode::BAD_REQUEST,
                "invalid or already attached file_ids",
            )
                .into_response();
        }
    }

    if tx.commit().await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

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

// ─── File upload ─────────────────────────────────────────────────────────────

/// Roll back all successfully uploaded files on a partial failure.
/// DB row is deleted first: if the DB delete succeeds and S3 delete later
/// fails, the object is unreachable but leaves no phantom DB reference.
/// If the DB delete itself fails, S3 is left untouched (consistent state).
async fn cleanup_uploaded(state: &AppState, uploaded: &[ChatMessageFile]) {
    for f in uploaded {
        if sqlx::query("DELETE FROM chat_message_files WHERE id = $1")
            .bind(f.id)
            .execute(&state.db)
            .await
            .is_ok()
        {
            let _ = state.oci_storage.delete_blob(&f.storage_uri).await;
        }
    }
}

async fn upload_files(
    State(state): State<AppState>,
    claims: Claims,
    Path(session_id): Path<String>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let owns = match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE session_id = $1 AND user_id = $2)",
    )
    .bind(&session_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if !owns {
        return StatusCode::NOT_FOUND.into_response();
    }

    let mut uploaded: Vec<ChatMessageFile> = Vec::new();
    let mut field_count: usize = 0;

    while let Ok(Some(field)) = multipart.next_field().await {
        field_count += 1;
        if field_count > MAX_FILES_PER_UPLOAD {
            cleanup_uploaded(&state, &uploaded).await;
            return (StatusCode::BAD_REQUEST, "too many files (max 10)").into_response();
        }

        let filename = field.file_name().unwrap_or("upload").to_string();
        let mime_type = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();

        let mut buf = bytes::BytesMut::new();
        let mut field = field;
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    buf.extend_from_slice(&chunk);
                    if buf.len() > MAX_FILE_BYTES {
                        cleanup_uploaded(&state, &uploaded).await;
                        return (
                            StatusCode::PAYLOAD_TOO_LARGE,
                            format!("file '{filename}' exceeds 50 MB limit"),
                        )
                            .into_response();
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    cleanup_uploaded(&state, &uploaded).await;
                    return StatusCode::BAD_REQUEST.into_response();
                }
            }
        }
        let data = buf.freeze();

        let file_id = Uuid::new_v4();
        let storage_key = format!("chat-files/{session_id}/{file_id}");
        let size_bytes = data.len() as i64;

        if let Err(e) = state.oci_storage.put_blob(&storage_key, data).await {
            tracing::warn!(file_id = %file_id, %e, "S3 put_blob failed; rolling back previous uploads");
            cleanup_uploaded(&state, &uploaded).await;
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }

        let record = sqlx::query_as::<_, ChatMessageFile>(
            r#"INSERT INTO chat_message_files (id, session_id, filename, mime_type, size_bytes, storage_uri)
               VALUES ($1, $2, $3, $4, $5, $6)
               RETURNING *"#,
        )
        .bind(file_id)
        .bind(&session_id)
        .bind(&filename)
        .bind(&mime_type)
        .bind(size_bytes)
        .bind(&storage_key)
        .fetch_one(&state.db)
        .await;

        match record {
            Ok(r) => uploaded.push(r),
            Err(e) => {
                tracing::warn!(file_id = %file_id, %e, "DB insert failed; rolling back S3 object and previous uploads");
                let _ = state.oci_storage.delete_blob(&storage_key).await;
                cleanup_uploaded(&state, &uploaded).await;
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    }

    (StatusCode::CREATED, Json(uploaded)).into_response()
}

// ─── File access ─────────────────────────────────────────────────────────────

async fn list_message_files(
    State(state): State<AppState>,
    claims: Claims,
    Path((session_id, message_id)): Path<(String, Uuid)>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let owns = match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE session_id = $1 AND user_id = $2)",
    )
    .bind(&session_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if !owns {
        return StatusCode::NOT_FOUND.into_response();
    }

    match sqlx::query_as::<_, ChatMessageFile>(
        "SELECT * FROM chat_message_files WHERE message_id = $1 AND session_id = $2 ORDER BY created_at ASC",
    )
    .bind(message_id)
    .bind(&session_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(files) => Json(files).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn download_file(
    State(state): State<AppState>,
    claims: Claims,
    Path(file_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let row = sqlx::query_as::<_, ChatMessageFile>(
        r#"SELECT cmf.* FROM chat_message_files cmf
           JOIN chat_sessions cs ON cs.session_id = cmf.session_id
           WHERE cmf.id = $1 AND (cs.user_id = $2 OR $3)"#,
    )
    .bind(file_id)
    .bind(user_id)
    .bind(claims.is_superuser)
    .fetch_optional(&state.db)
    .await;

    let file = match row {
        Ok(Some(f)) => f,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    match state
        .oci_storage
        .presigned_get_url(&file.storage_uri, 3600)
        .await
    {
        Ok(url) => (
            StatusCode::TEMPORARY_REDIRECT,
            [(header::LOCATION, url)],
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn delete_file(
    State(state): State<AppState>,
    claims: Claims,
    Path(file_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id: Uuid = match claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let row = sqlx::query_as::<_, ChatMessageFile>(
        r#"SELECT cmf.* FROM chat_message_files cmf
           JOIN chat_sessions cs ON cs.session_id = cmf.session_id
           WHERE cmf.id = $1 AND (cs.user_id = $2 OR $3)"#,
    )
    .bind(file_id)
    .bind(user_id)
    .bind(claims.is_superuser)
    .fetch_optional(&state.db)
    .await;

    let file = match row {
        Ok(Some(f)) => f,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // FIX: delete DB row first — if S3 delete fails, the row is gone and the
    // stale storage_uri can no longer produce presigned URLs to clients.
    // Opposite order (S3 then DB) risks a stale row surviving a transient DB error.
    if sqlx::query("DELETE FROM chat_message_files WHERE id = $1")
        .bind(file_id)
        .execute(&state.db)
        .await
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    if let Err(e) = state.oci_storage.delete_blob(&file.storage_uri).await {
        tracing::warn!(file_id = %file_id, %e, "S3 delete failed after DB row removed (orphaned object)");
    }

    // Clear has_file_parts atomically: only when no other files reference this
    // message AND the message has no inline file_parts JSONB. The EXISTS
    // sub-query and the UPDATE run in a single statement, eliminating the
    // COUNT then UPDATE race that existed when these were two separate queries.
    if let Some(msg_id) = file.message_id {
        let _ = sqlx::query(
            "UPDATE chat_messages SET has_file_parts = false
             WHERE id = $1
               AND file_parts IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM chat_message_files WHERE message_id = $1
               )",
        )
        .bind(msg_id)
        .execute(&state.db)
        .await;
    }

    StatusCode::NO_CONTENT.into_response()
}
