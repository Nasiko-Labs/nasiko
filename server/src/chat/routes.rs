use axum::{
    Json, Router,
    body::Body,
    extract::{Multipart, Path, Query, State, rejection::JsonRejection},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use nasiko_runtime::{ContainerId, WorkspaceRef};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

use super::external_turn::{PersistExternalTurnError, persist_external_turn};
use super::models::*;

const MAX_FILES_PER_UPLOAD: usize = 10;
const MAX_FILE_BYTES: usize = 50 * 1024 * 1024; // 50 MB

/// Upper bound on how long the per-turn workspace capture may hold the chat
/// persist path. The reader is a long-lived singleton, so after its first start
/// this is never approached; the cap only protects the cold-start / evicted-pod
/// case from blocking a completed reply.
const CAPTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

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
            "/chat/sessions/{session_id}/external-turns",
            axum::routing::post(save_external_turn),
        )
        .route(
            "/chat/sessions/{session_id}/files",
            // Bound the multipart body to the file policy (count × per-file cap +
            // small overhead) so a chat upload can't buffer unbounded bytes (SRV-4).
            axum::routing::post(upload_files).layer(axum::extract::DefaultBodyLimit::max(
                MAX_FILES_PER_UPLOAD * MAX_FILE_BYTES + 1024 * 1024,
            )),
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
    /// An embedded surface — a feature with its own chat history menu — keeps
    /// its sessions in this same table, namespaced as `<surface>_<contextId>`.
    /// Those chats belong to that surface's menu, not the Sessions list, the
    /// Orchestrator nav tree or `nasiko sessions`, so the general list leaves
    /// them out and a surface asks for its own by name (`?surface=<name>`).
    ///
    /// Which surfaces exist is the edition's business, not this module's, so
    /// "namespaced" has to be decidable here without knowing any surface's
    /// name. An underscore alone does not decide it: a session the platform
    /// mints carries the reserved [`PLATFORM_SESSION_PREFIX`], which is
    /// therefore excluded from the namespace test rather than read as a
    /// surface called "ses".
    surface: Option<String>,
}

/// Prefix on every session id the platform mints itself — here in
/// `create_session` and in `agent_proxy` for a message that arrives with no
/// contextId. It is not a surface: these are the ordinary sessions the general
/// list exists to show.
const PLATFORM_SESSION_PREFIX: &str = "ses";

/// A surface name is a path-safe slug and nothing else. Validating rather
/// than escaping keeps the `LIKE` pattern below free of anything a caller
/// could turn into a wildcard, and the name is bound as a parameter besides.
/// The platform's own prefix is refused so no surface can claim it.
fn valid_surface(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name != PLATFORM_SESSION_PREFIX
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}
fn default_session_limit() -> i64 {
    50
}

/// Fixed predicate appended to each keyset variant's `WHERE`. `\_` escapes the
/// `_` so `LIKE` matches the namespace separator literally rather than any
/// single character.
///
/// Asking for a surface matches its prefix, which arrives as `$1`; asking for
/// nothing returns the general list, which is every session no surface has
/// claimed — including the platform's own `ses_*` ids, which are not a
/// namespace. No caller input is interpolated either way; the only value in
/// the text is this module's own constant.
fn surface_predicate(surface: Option<&str>, bind: usize) -> String {
    match surface {
        // Appended after each variant's own binds, so the four keyset
        // numberings below are untouched. Placeholder order in the text does
        // not have to match their order in the statement.
        Some(_) => format!(r"AND cs.session_id LIKE ${bind} || '\_%'"),
        None => format!(
            r"AND (cs.session_id NOT LIKE '%\_%' OR cs.session_id LIKE '{PLATFORM_SESSION_PREFIX}\_%')"
        ),
    }
}

/// `SELECT`/`FROM` prefix shared by the four keyset variants below; each appends
/// its own `WHERE`/`ORDER BY`/`LIMIT`. No user input is interpolated — the
/// variants differ only in fixed predicates and bind-parameter numbering.
///
/// The second `LATERAL` is what keeps the sessions page off the trace store:
/// message/trace counts, billed tokens and p50 latency all come from
/// `chat_messages` columns (migration 041), covered by
/// `idx_messages_session(session_id, timestamp)`.
const SESSION_LIST_SELECT: &str = r#"
    SELECT cs.*,
           CASE
             WHEN a.coding_agent_integration_id IS NOT NULL
                  AND u.username IS NOT NULL
                  AND a.name NOT LIKE u.username || '-%'
               THEN u.username || '-' || a.name
             ELSE a.name
           END AS agent_name,
           (a.coding_agent_integration_id IS NOT NULL) AS is_coding_agent,
           lm.content AS last_message,
           agg.message_count,
           agg.trace_count,
           agg.total_tokens,
           agg.latency_p50_ms
    FROM chat_sessions cs
    LEFT JOIN agents a ON a.id = cs.agent_id
    LEFT JOIN users u ON u.id = cs.user_id
    LEFT JOIN LATERAL (
        SELECT content FROM chat_messages
        WHERE session_id = cs.session_id
        ORDER BY timestamp DESC LIMIT 1
    ) lm ON true
    LEFT JOIN LATERAL (
        SELECT COUNT(*) AS message_count,
               COUNT(DISTINCT m.trace_id) AS trace_count,
               -- Aggregate only rows that actually carry usage, so NULL means
               -- "nothing recorded" (BYO-key agent, or a message predating
               -- migration 041) and a genuine 0 stays 0. `NULLIF(SUM(...), 0)`
               -- would conflate those two, since SUM over all-NULL columns
               -- coalesces to 0.
               SUM(COALESCE(m.input_tokens, 0) + COALESCE(m.output_tokens, 0)
                   + COALESCE(m.cache_read_tokens, 0)
                   + COALESCE(m.cache_creation_tokens, 0))
                   FILTER (
                       WHERE m.input_tokens IS NOT NULL OR m.output_tokens IS NOT NULL
                   ) AS total_tokens,
               -- percentile_cont ignores NULL inputs, so messages written
               -- before migration 041 are skipped rather than counted as 0ms.
               percentile_cont(0.5) WITHIN GROUP (ORDER BY m.duration_ms)
                   AS latency_p50_ms
        FROM chat_messages m
        WHERE m.session_id = cs.session_id
    ) agg ON true
"#;

async fn list_sessions(
    State(state): State<AppState>,
    claims: Claims,
    Query(params): Query<ListSessionsParams>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let limit = params.limit.clamp(1, 100);
    let fetch = limit + 1;

    // Decode cursor into (timestamp, session_id) keyset anchor.
    let cursor_anchor = params.cursor.as_deref().and_then(decode_cursor);

    let surface = match params.surface.as_deref().map(str::trim) {
        // `Some("")`, not a guard: clippy::redundant_guards, and the literal
        // says the same thing in fewer moving parts. `str::trim` above means an
        // all-whitespace value arrives here as the empty string too.
        Some("") => None,
        Some(name) if !valid_surface(name) => {
            return (
                StatusCode::BAD_REQUEST,
                "surface must be a lowercase slug (a-z, 0-9, -)",
            )
                .into_response();
        }
        other => other,
    };

    let query_result: Result<Vec<ChatSessionView>, _> = match (cursor_anchor, params.agent_id) {
        (None, None) => {
            let scope = surface_predicate(surface, 3);
            let sql = format!(
                "{SESSION_LIST_SELECT}
                 WHERE cs.user_id = $1 {scope}
                 ORDER BY cs.updated_at DESC, cs.session_id DESC
                 LIMIT $2"
            );
            let mut q = sqlx::query_as::<_, ChatSessionView>(&sql)
                .bind(user_id)
                .bind(fetch);
            if let Some(name) = surface {
                q = q.bind(name);
            }
            q.fetch_all(&state.db).await
        }

        (None, Some(agent_id)) => {
            let scope = surface_predicate(surface, 4);
            let sql = format!(
                "{SESSION_LIST_SELECT}
                 WHERE cs.user_id = $1 AND cs.agent_id = $2 {scope}
                 ORDER BY cs.updated_at DESC, cs.session_id DESC
                 LIMIT $3"
            );
            let mut q = sqlx::query_as::<_, ChatSessionView>(&sql)
                .bind(user_id)
                .bind(agent_id)
                .bind(fetch);
            if let Some(name) = surface {
                q = q.bind(name);
            }
            q.fetch_all(&state.db).await
        }

        (Some((cursor_ts, cursor_sid)), None) => {
            let scope = surface_predicate(surface, 5);
            let sql = format!(
                "{SESSION_LIST_SELECT}
                 WHERE cs.user_id = $1 {scope}
                   AND (cs.updated_at < $2 OR (cs.updated_at = $2 AND cs.session_id < $3))
                 ORDER BY cs.updated_at DESC, cs.session_id DESC
                 LIMIT $4"
            );
            let mut q = sqlx::query_as::<_, ChatSessionView>(&sql)
                .bind(user_id)
                .bind(cursor_ts)
                .bind(cursor_sid)
                .bind(fetch);
            if let Some(name) = surface {
                q = q.bind(name);
            }
            q.fetch_all(&state.db).await
        }

        (Some((cursor_ts, cursor_sid)), Some(agent_id)) => {
            let scope = surface_predicate(surface, 6);
            let sql = format!(
                "{SESSION_LIST_SELECT}
                 WHERE cs.user_id = $1 AND cs.agent_id = $2 {scope}
                   AND (cs.updated_at < $3 OR (cs.updated_at = $3 AND cs.session_id < $4))
                 ORDER BY cs.updated_at DESC, cs.session_id DESC
                 LIMIT $5"
            );
            let mut q = sqlx::query_as::<_, ChatSessionView>(&sql)
                .bind(user_id)
                .bind(agent_id)
                .bind(cursor_ts)
                .bind(cursor_sid)
                .bind(fetch);
            if let Some(name) = surface {
                q = q.bind(name);
            }
            q.fetch_all(&state.db).await
        }
    };
    let mut rows = match query_result {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(%e, "list_sessions: db error");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let has_more = rows.len() > limit as usize;
    if has_more {
        rows.pop();
    }

    let next_cursor = if has_more {
        rows.last()
            .map(|r| encode_cursor(r.updated_at, &r.session_id))
    } else {
        None
    };

    // No `prev_cursor` here: unlike `list_messages` (which has a real `before`
    // cursor param and backward-paging query branch), `ListSessionsParams` has
    // no backward-paging input at all — a value here would be dead API surface
    // implying a capability that doesn't exist. The UI doesn't read this field
    // (grepped `oss/ui` — no references), so omitting it is a pure cleanup.
    // Always return the proxy URL derived from agent_id so clients get a
    // consistent, externally-reachable path regardless of what was stored
    // (old sessions may have the internal cluster URL).
    for row in &mut rows {
        if let Some(id) = row.agent_id {
            row.agent_url = Some(format!("/api/agents/{id}"));
        }
    }

    Json(CursorPage {
        data: rows,
        has_more,
        next_cursor,
        prev_cursor: None,
    })
    .into_response()
}

async fn create_session(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateSession>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // Resolve + authorize the agent server-side. `agent_url` is NEVER taken
    // from the client (stored-SSRF risk): the canonical URL is always the one
    // registered on the agent's own row, resolved here — never trusted from
    // request input. `agent_id` is validated to (a) actually exist and (b) be
    // accessible to the caller via the same `can_access_agent` check used by
    // every other agent-scoped handler, closing the gap where a client could
    // previously supply an arbitrary UUID or name with no existence/ownership
    // check at all.
    let (agent_id, agent_url): (Option<Uuid>, Option<String>) = match &body.agent_id {
        None => (None, None),
        Some(id_or_name) => {
            let row = if let Ok(uuid) = id_or_name.parse::<Uuid>() {
                sqlx::query_scalar::<_, Uuid>("SELECT id FROM agents WHERE id = $1")
                    .bind(uuid)
                    .fetch_optional(&state.db)
                    .await
            } else {
                sqlx::query_scalar::<_, Uuid>("SELECT id FROM agents WHERE name = $1")
                    .bind(id_or_name)
                    .fetch_optional(&state.db)
                    .await
            };

            match row {
                Ok(Some(id)) => {
                    if !crate::acl::can_access_agent(&state, &claims, id).await {
                        return StatusCode::FORBIDDEN.into_response();
                    }
                    // Store the proxy path so clients can extract the agent id
                    // from the URL (internal cluster URLs are not accessible from outside).
                    let proxy_url = format!("/api/agents/{id}");
                    (Some(id), Some(proxy_url))
                }
                Ok(None) => {
                    return (StatusCode::BAD_REQUEST, "agent not found").into_response();
                }
                Err(e) => {
                    tracing::error!(%e, "create_session: agent lookup failed");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
    };

    let session_id = match body.session_id.as_deref().map(str::trim) {
        Some(custom) if !custom.is_empty() => {
            // Client-supplied ID: return the caller's existing session when
            // present; reject the ID when it belongs to another user.
            let existing = sqlx::query_as::<_, ChatSession>(
                "SELECT * FROM chat_sessions WHERE session_id = $1",
            )
            .bind(custom)
            .fetch_optional(&state.db)
            .await;
            match existing {
                Ok(Some(session)) if session.user_id == user_id => {
                    return (StatusCode::OK, Json(session_response(session))).into_response();
                }
                Ok(Some(_)) => {
                    return (StatusCode::CONFLICT, "session_id already in use").into_response();
                }
                Ok(None) => custom.to_string(),
                Err(e) => {
                    tracing::error!(%e, "create_session: session lookup failed");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
        _ => format!("{PLATFORM_SESSION_PREFIX}_{}", Uuid::new_v4().simple()),
    };

    // tracing::info!(body = ?body, "Received create session request");
    // Derive the session title from the user's first prompt synchronously: block
    // on the LLM so the row is inserted with its final title in a single INSERT.
    // On an empty prompt, an unconfigured provider, or any LLM error,
    // `titling::title_from_prompt` falls back to a truncated form of the prompt
    // (or "New chat"), so creation never fails on the model.
    let title = match body.first_prompt.as_deref().map(str::trim) {
        Some(prompt) if !prompt.is_empty() => {
            crate::titling::title_from_prompt(&state, prompt).await
        }
        _ => "New chat".to_string(),
    };

    // No agent resolved above means the caller is chatting with the
    // orchestrator, which picks the agent per turn.
    let session_type = if agent_id.is_some() {
        "direct_chat"
    } else {
        "orchestrator"
    };
    let result = sqlx::query_as::<_, ChatSession>(
        r#"INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title, session_type)
           VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING *"#,
    )
    .bind(&session_id)
    .bind(user_id)
    .bind(agent_id)
    .bind(&agent_url)
    .bind(&title)
    .bind(session_type)
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(session) => (StatusCode::CREATED, Json(session_response(session))).into_response(),
        Err(e) => {
            // A dangling user_id FK means the (gateway-verified) JWT references
            // a user that no longer exists — e.g. the DB was reseeded after the
            // token was issued. That's a stale credential, not a server fault.
            if let sqlx::Error::Database(ref db_err) = e
                && db_err.constraint() == Some("chat_sessions_user_id_fkey")
            {
                return (
                    StatusCode::UNAUTHORIZED,
                    "your user account no longer exists on this control plane — log out and log in again",
                )
                    .into_response();
            }
            tracing::error!(%e, "create_session failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(serde::Serialize)]
struct SessionData {
    session_id: String,
    created_at: DateTime<Utc>,
    title: String,
    agent_id: Option<Uuid>,
    agent_url: Option<String>,
}

#[derive(serde::Serialize)]
struct SessionResponse {
    data: SessionData,
    status_code: u16,
    message: String,
}

fn session_response(s: ChatSession) -> SessionResponse {
    SessionResponse {
        data: SessionData {
            session_id: s.session_id,
            created_at: s.created_at,
            title: s.title,
            agent_id: s.agent_id,
            agent_url: s.agent_url,
        },
        status_code: 201,
        message: String::new(),
    }
}

async fn get_session(
    State(state): State<AppState>,
    claims: Claims,
    Path(session_id): Path<String>,
    Query(params): Query<ListMessagesParams>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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

    let messages = match sqlx::query_as::<_, ChatMessage>(
        r#"SELECT * FROM chat_messages
           WHERE session_id = $1
           ORDER BY timestamp ASC, id ASC
           LIMIT $2"#,
    )
    .bind(&session_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(%e, session_id, "get_session messages: db error");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    #[derive(serde::Serialize)]
    struct Response {
        data: Vec<ChatMessage>,
    }
    Json(Response { data: messages }).into_response()
}

async fn update_session(
    State(state): State<AppState>,
    claims: Claims,
    Path(session_id): Path<String>,
    Json(body): Json<UpdateSession>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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
fn default_message_limit() -> i64 {
    100
}

async fn list_messages(
    State(state): State<AppState>,
    claims: Claims,
    Path(session_id): Path<String>,
    Query(params): Query<ListMessagesParams>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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

    // Keyset on the COMPOSITE (timestamp, id), not timestamp alone — multiple
    // messages can share one `now()` tick, and a timestamp-only keyset silently
    // skips the ones straddling a page boundary (SRV-1). The cursor carries the id;
    // a raw before/after timestamp (no id) uses a sentinel id so the composite
    // comparison reduces to a pure `timestamp` bound (MAX for `>`, nil for `<`).
    let before = params
        .prev_cursor
        .as_deref()
        .and_then(decode_cursor)
        .and_then(|(ts, id)| id.parse::<Uuid>().ok().map(|u| (ts, u)))
        .or(params.before.map(|ts| (ts, Uuid::nil())));
    let after = params
        .next_cursor
        .as_deref()
        .and_then(decode_cursor)
        .and_then(|(ts, id)| id.parse::<Uuid>().ok().map(|u| (ts, u)))
        .or(params.after.map(|ts| (ts, Uuid::from_u128(u128::MAX))));

    // `role <> 'system'` on every branch: a HITL resume records its continuation ("The archive
    // agent replied: …") so the next turn's reasoning still has it (`INTERNAL_TRANSCRIPT_ROLE`,
    // `router/a2a_dispatch.rs`), but nobody said it — shown in a transcript it reads as a message
    // the human typed. Filtered in SQL rather than after the fetch so page sizes and cursors stay
    // consistent with what the client actually receives.
    //
    // Fetch DESC in all cases except `after`; reverse in Rust so client always sees ASC.
    let (msg_result, fetched_asc): (Result<Vec<ChatMessage>, _>, bool) = match (before, after) {
        (_, Some((after_ts, after_id))) => {
            let r = sqlx::query_as::<_, ChatMessage>(
                r#"SELECT * FROM chat_messages
                   WHERE session_id = $1 AND role <> 'system' AND (timestamp, id) > ($2, $3)
                   ORDER BY timestamp ASC, id ASC
                   LIMIT $4"#,
            )
            .bind(&session_id)
            .bind(after_ts)
            .bind(after_id)
            .bind(fetch)
            .fetch_all(&state.db)
            .await;
            (r, true)
        }
        (Some((before_ts, before_id)), None) => {
            let r = sqlx::query_as::<_, ChatMessage>(
                r#"SELECT * FROM chat_messages
                   WHERE session_id = $1 AND role <> 'system' AND (timestamp, id) < ($2, $3)
                   ORDER BY timestamp DESC, id DESC
                   LIMIT $4"#,
            )
            .bind(&session_id)
            .bind(before_ts)
            .bind(before_id)
            .bind(fetch)
            .fetch_all(&state.db)
            .await;
            (r, false)
        }
        (None, None) => {
            let r = sqlx::query_as::<_, ChatMessage>(
                r#"SELECT * FROM chat_messages
                   WHERE session_id = $1 AND role <> 'system'
                   ORDER BY timestamp DESC, id DESC
                   LIMIT $2"#,
            )
            .bind(&session_id)
            .bind(fetch)
            .fetch_all(&state.db)
            .await;
            (r, false)
        }
    };
    let mut rows = match msg_result {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(%e, session_id, "list_messages: db error");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let has_more = rows.len() > limit as usize;
    if has_more {
        rows.pop();
    }

    // Always present to client in ASC (chronological) order.
    if !fetched_asc {
        rows.reverse();
    }

    let out_prev_cursor = rows
        .first()
        .map(|r| encode_cursor(r.timestamp, &r.id.to_string()));
    let out_next_cursor = if has_more {
        rows.last()
            .map(|r| encode_cursor(r.timestamp, &r.id.to_string()))
    } else {
        None
    };

    // Session-load HITL discovery: every HITL request tied to this
    // session, pending or already resolved, rides along with the message page instead of
    // requiring a separate `GET /api/hitl/pending` call. Scoped by BOTH `chat_session_id` and
    // `owner_user_id` inside the query (`list_for_chat_session`) — the second is redundant with
    // the `owns` check above in the ordinary case, but costs nothing and means a future refactor
    // of that check can't silently turn this into a cross-user leak on its own. Reuses
    // `router::hitl::to_response` verbatim so this can never drift from — or accidentally leak
    // more than — the one HITL DTO the rest of the API already exposes (`resume_state` etc. stay
    // excluded because `HitlRequest` itself isn't `Serialize`).
    // A failure here degrades to an empty `hitl` array rather than failing the whole response —
    // `rows`/`has_more`/the cursors above are already a complete, correct answer to "what are
    // this session's messages", and this supplementary panel must never make that a hard
    // dependency of ordinary chat history. The frontend still has `GET /api/hitl/pending` as a
    // fallback discovery path for anything pending, unlike the messages themselves.
    let hitl_rows = match state
        .hitl_store
        .list_for_chat_session(&session_id, user_id)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(%e, session_id, "list_messages: hitl lookup failed, omitting hitl array");
            Vec::new()
        }
    };
    // Each row goes through `resolve_display_row` before `to_response` — a no-op for the
    // ordinary case, but substitutes the real row's id/kind/question when this row is a mirror
    // of a real `mcp_tool` block (see that function's doc comment): otherwise the frontend would
    // see the mirror's own generic question and could "resolve" an id that grants no real
    // permission. Resolved concurrently, not one row at a time — `resolve_display_row` does a DB
    // round-trip per linked row, and this endpoint runs on every single chat history page load.
    let hitl: Vec<serde_json::Value> = futures::future::join_all(hitl_rows.iter().map(|row| {
        let store = state.hitl_store.as_ref();
        async move {
            let display = nasiko_hitl::resolve_display_row(store, row, user_id).await;
            crate::router::hitl::to_response(&display)
        }
    }))
    .await;

    #[derive(serde::Serialize)]
    struct MessagesResponse {
        #[serde(flatten)]
        page: CursorPage<ChatMessage>,
        hitl: Vec<serde_json::Value>,
    }

    Json(MessagesResponse {
        page: CursorPage {
            data: rows,
            has_more,
            next_cursor: out_next_cursor,
            prev_cursor: out_prev_cursor,
        },
        hitl,
    })
    .into_response()
}

async fn send_message(
    State(state): State<AppState>,
    claims: Claims,
    Path(session_id): Path<String>,
    Json(body): Json<SendMessage>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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

    // Whitelisted, not just filtered on read (found in review): `list_messages` hides
    // `role = 'system'` rows from the transcript on the assumption that nothing client-supplied
    // can carry that role, and `SessionHistory::fetch` (`oss/orchestrator`) applies no such filter
    // when building the next turn's LLM prompt — an unvalidated `role` here would let a caller
    // plant an invisible system-role instruction that still reaches the model. See
    // `CHAT_MESSAGE_ROLE_USER`/`CHAT_MESSAGE_ROLE_ASSISTANT`'s own doc comment for why `"system"`
    // itself is excluded even though it's a real value in this column.
    if body.role != CHAT_MESSAGE_ROLE_USER && body.role != CHAT_MESSAGE_ROLE_ASSISTANT {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "role must be \"{CHAT_MESSAGE_ROLE_USER}\" or \"{CHAT_MESSAGE_ROLE_ASSISTANT}\""
            ),
        )
            .into_response();
    }

    // Dedupe: a client sending the same file_id twice would otherwise cause
    // `claimed != file_ids.len()` to false-positive (the UPDATE affects each
    // distinct row once, but `ANY($2)` with a duplicate doesn't double-count
    // rows_affected), producing a spurious 400 on an otherwise-valid request.
    let file_ids: Vec<Uuid> = body
        .file_ids
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let file_ids = file_ids.as_slice();
    let has_files = !file_ids.is_empty();
    // FIX: filter out JSON null so it stores as SQL NULL, not 'null'::jsonb.
    // 'null'::jsonb IS NOT NULL in Postgres, which would defeat the AND file_parts IS NULL
    // guard in delete_file and permanently stick has_file_parts = true.
    let file_parts_json = body
        .file_parts
        .filter(|v| !v.is_null())
        .map(sqlx::types::Json);
    let has_file_parts = has_files || file_parts_json.is_some();

    // FIX: wrap message insert + file claim in a transaction to eliminate the
    // TOCTOU race where two concurrent sends could both pass the unattached check.
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let usage = body.usage.as_ref();
    let mut msg = match sqlx::query_as::<_, ChatMessage>(
        r#"INSERT INTO chat_messages
               (session_id, role, content, file_parts, has_file_parts,
                input_tokens, output_tokens, model, duration_ms, cost_usd,
                usage_estimated, trace_id, cache_read_tokens, cache_creation_tokens)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
           RETURNING *"#,
    )
    .bind(&session_id)
    .bind(&body.role)
    .bind(&body.content)
    .bind(&file_parts_json)
    .bind(has_file_parts)
    .bind(usage.and_then(|u| u.input_tokens))
    .bind(usage.and_then(|u| u.output_tokens))
    .bind(usage.and_then(|u| u.model.as_deref()))
    .bind(usage.and_then(|u| u.duration_ms))
    .bind(usage.and_then(|u| u.cost_usd))
    .bind(usage.and_then(|u| u.estimated))
    .bind(usage.and_then(|u| u.trace_id.as_deref()))
    .bind(usage.and_then(|u| u.cache_read_tokens))
    .bind(usage.and_then(|u| u.cache_creation_tokens))
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

    if let Err(e) = sqlx::query("UPDATE chat_sessions SET updated_at = now() WHERE session_id = $1")
        .bind(&session_id)
        .execute(&state.db)
        .await
    {
        tracing::warn!(session_id, %e, "failed to touch session updated_at");
    }

    // Platform-driven capture: attach the workspace files the agent referenced
    // in this reply to the assistant message, downloadable via the fixed
    // `/chat/files/{id}/download` route. Works for any agent with no cooperation.
    // Best-effort AND time-bounded - a failure or a slow/not-yet-ready workspace
    // reader (the first turn after install can wait on a pod pull) must never
    // stall the user seeing their completed answer. On timeout the message is
    // returned without chips; the files are still on disk for a later turn.
    if msg.role == "assistant" {
        let capture = tokio::time::timeout(
            CAPTURE_TIMEOUT,
            capture_turn_files(&state, &session_id, msg.id, &msg.content),
        )
        .await;
        match capture {
            Ok(Some(parts)) => {
                msg.has_file_parts = true;
                msg.file_parts = Some(sqlx::types::Json(parts));
            }
            Ok(None) => {}
            Err(_) => tracing::warn!(session_id, "capture: workspace reader timed out, no chips"),
        }
    }

    (StatusCode::CREATED, Json(msg)).into_response()
}

#[derive(serde::Serialize)]
struct ExternalTurnResponse {
    inserted: bool,
    user_message: ChatMessage,
    assistant_message: ChatMessage,
}

async fn save_external_turn(
    State(state): State<AppState>,
    claims: Claims,
    Path(session_id): Path<String>,
    body: Result<Json<ExternalTurn>, JsonRejection>,
) -> impl IntoResponse {
    let body = match body {
        Ok(Json(body)) => body,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid external turn: {}", error.body_text()),
            )
                .into_response();
        }
    };
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let turn_id = body.turn_id.trim();
    if turn_id.is_empty()
        || body.user_content.trim().is_empty()
        || body.assistant_content.trim().is_empty()
    {
        return (
            StatusCode::BAD_REQUEST,
            "turn_id and content must be nonempty",
        )
            .into_response();
    }

    if let Some(usage) = &body.assistant_usage {
        let invalid = usage.input_tokens.is_some_and(|v| v < 0)
            || usage.output_tokens.is_some_and(|v| v < 0)
            || usage.duration_ms.is_some_and(|v| v < 0)
            || usage.cost_usd.is_some_and(|v| v.is_sign_negative());
        if invalid {
            return (StatusCode::BAD_REQUEST, "usage values must be nonnegative").into_response();
        }
    }

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(%e, session_id, "save_external_turn: begin transaction failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let owns = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE session_id = $1 AND user_id = $2)",
    )
    .bind(&session_id)
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await;
    match owns {
        Ok(true) => {}
        Ok(false) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, session_id, "save_external_turn: ownership lookup failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    let persisted = match persist_external_turn(&mut tx, &session_id, &body, Utc::now(), true).await
    {
        Ok(persisted) => persisted,
        Err(PersistExternalTurnError::Incomplete) => {
            let _ = tx.rollback().await;
            return (StatusCode::CONFLICT, "external turn is incomplete").into_response();
        }
        Err(PersistExternalTurnError::Conflict) => {
            let _ = tx.rollback().await;
            return (
                StatusCode::CONFLICT,
                "turn_id already exists with a different payload",
            )
                .into_response();
        }
        Err(PersistExternalTurnError::Database(e)) => {
            tracing::error!(%e, session_id, turn_id, "save_external_turn: persistence failed");
            let _ = tx.rollback().await;
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if let Err(e) = tx.commit().await {
        tracing::error!(%e, session_id, turn_id, "save_external_turn: commit failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    (
        if persisted.inserted {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(ExternalTurnResponse {
            inserted: persisted.inserted,
            user_message: persisted.user_message,
            assistant_message: persisted.assistant_message,
        }),
    )
        .into_response()
}

/// Strip anything from a captured filename that could break out of a quoted
/// `Content-Disposition` value or produce an invalid header: quotes, backslashes,
/// and control characters (a POSIX basename may contain a newline). Non-ASCII is
/// left intact — HTTP header values permit obs-text.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .filter(|&c| !c.is_control() && c != '"' && c != '\\')
        .collect()
}

/// Whether `reply` references `name` as a filename **token**, not merely as a
/// substring. `str::contains` over-captures: `a.py` would match inside
/// `a.python`, and on a shared writable container that can cross-attribute a
/// same-substring file to an unrelated turn. A match counts only when it is
/// bounded on both sides by a non-filename character (whitespace, quotes,
/// backticks, parens, path separators, sentence punctuation) — never glued to an
/// alphanumeric, `_`, `-`, or a `.` that joins two name characters (so `report.md`
/// is *not* captured inside `report.md.bak`, but a trailing sentence period in
/// "saved `report.md`." still is).
fn reply_references_file(reply: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    // A `.` continues a filename only when it glues two name characters together
    // (`a.py`, `.bak`); a `.` next to whitespace/start/end is sentence punctuation.
    let alnum = |c: char| c.is_ascii_alphanumeric();
    let name_char = |c: char| alnum(c) || c == '_' || c == '-';
    let mut from = 0;
    while let Some(rel) = reply[from..].find(name) {
        let start = from + rel;
        let end = start + name.len();
        let before = reply[..start].chars().next_back();
        let before_ok = match before {
            None => true,
            Some(c) if name_char(c) => false,
            // `foo.report.md` — the `.` before joins an alnum on its left.
            Some('.') => !reply[..start - 1].chars().next_back().is_some_and(alnum),
            Some(_) => true,
        };
        let mut after = reply[end..].chars();
        let after_ok = match after.next() {
            None => true,
            Some(c) if name_char(c) => false,
            // `report.md.bak` — the `.` after is followed by an alnum.
            Some('.') => !after.next().is_some_and(alnum),
            Some(_) => true,
        };
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Attach the workspace files the agent **named in its reply** to the assistant
/// message, as session-scoped `chat_message_files`.
///
/// Platform-driven: the agent writes to `/workspace` however it likes and knows
/// nothing about Nasiko. Attribution is by **what the reply references** - a
/// workspace file whose name appears in the agent's response is the file this
/// user asked about, whether the agent just wrote it ("saved `foo.py`") or is
/// handing back an existing one ("Download `bar.py`"). This matches "give me
/// *that* file" and, unlike write-time (mtime) attribution, is concurrency-safe:
/// each user's reply names their own file, so overlapping turns don't
/// cross-attribute.
///
/// This is a download **convenience, not a privacy boundary**: on a shared
/// container the agent can already read (and list on request) every file in
/// `/workspace`, so real per-user isolation needs a per-session container.
/// Downloads are still ACL'd to the session
/// owner. Bytes stay in the PVC (`storage_uri` =
/// `workspace://<owner>/<agent>/<relpath>`); `download_file` streams them,
/// behind a contract Phase 2 can re-back with object storage unchanged.
///
/// Returns the `file_parts` array, or `None` when the agent isn't `--writable`
/// or its reply named no existing workspace file.
async fn capture_turn_files(
    state: &AppState,
    session_id: &str,
    message_id: Uuid,
    reply: &str,
) -> Option<serde_json::Value> {
    // The session's agent, and whether it can write at all.
    let (agent_id, owner_id, writable): (Uuid, Uuid, bool) = sqlx::query_as(
        "SELECT a.id, a.owner_id, a.writable \
         FROM chat_sessions cs JOIN agents a ON a.id = cs.agent_id \
         WHERE cs.session_id = $1 AND a.deleted_at IS NULL",
    )
    .bind(session_id)
    .fetch_optional(&state.db)
    .await
    .ok()??;
    if !writable {
        return None;
    }

    // List the whole agent workspace (scope = None); keep the files this reply
    // actually references by name.
    let ws = WorkspaceRef {
        owner_id,
        container_id: ContainerId::from_uuid(agent_id),
    };
    let entries = match state.runtime.list_workspace(&ws).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(session_id, %e, "capture: list_workspace failed");
            return None;
        }
    };

    let mut parts = Vec::new();
    for entry in entries {
        let name = entry
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&entry.path)
            .to_owned();
        // Only files the agent named in this reply — the ones the user asked
        // about. A *token* match, not a bare substring: `a.py` must not be
        // captured because the reply happened to contain `a.python`, which both
        // over-captures and (on a shared writable container) risks handing one
        // user a download link to a same-substring file another user's turn
        // produced.
        if !reply_references_file(reply, &name) {
            continue;
        }
        let file_id = Uuid::new_v4();
        let mime = mime_guess::from_path(&entry.path)
            .first_or_octet_stream()
            .to_string();
        let storage_uri = format!("workspace://{owner_id}/{agent_id}/{}", entry.path);
        if let Err(e) = sqlx::query(
            "INSERT INTO chat_message_files \
                 (id, message_id, session_id, filename, mime_type, size_bytes, storage_uri) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(file_id)
        .bind(message_id)
        .bind(session_id)
        .bind(&name)
        .bind(&mime)
        .bind(entry.size as i64)
        .bind(&storage_uri)
        .execute(&state.db)
        .await
        {
            tracing::warn!(session_id, %e, "capture: file insert failed");
            continue;
        }
        parts.push(serde_json::json!({
            "id": file_id, "name": name, "size": entry.size, "mime": mime,
        }));
    }

    if parts.is_empty() {
        return None;
    }
    let file_parts = serde_json::Value::Array(parts);
    let _ = sqlx::query(
        "UPDATE chat_messages SET file_parts = $1, has_file_parts = true WHERE id = $2",
    )
    .bind(sqlx::types::Json(&file_parts))
    .bind(message_id)
    .execute(&state.db)
    .await;
    Some(file_parts)
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
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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

    // Phase 1 captures keep their bytes in the agent's PVC; `storage_uri` encodes
    // the reader-side location (`workspace://<owner>/<agent>/<relpath>`) and we
    // stream them out. Uploads (and Phase 2 captures) live in object storage and
    // redirect to a presigned URL. The route is identical either way, so the FE
    // never learns where the bytes are.
    if let Some(rest) = file.storage_uri.strip_prefix("workspace://") {
        return stream_workspace_file(&state, &file, rest).await;
    }

    match state
        .oci_storage
        .presigned_get_url(&file.storage_uri, 3600)
        .await
    {
        Ok(url) => (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, url)]).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Stream a PVC-backed capture out of the agent's `/workspace` via the runtime's
/// workspace reader. `rest` is `<owner>/<agent>/<relpath>` (the `storage_uri`
/// minus its `workspace://` scheme). Bytes are streamed, never buffered, so a
/// large file is constant-memory on the control plane.
async fn stream_workspace_file(
    state: &AppState,
    file: &ChatMessageFile,
    rest: &str,
) -> axum::response::Response {
    let mut segs = rest.splitn(3, '/');
    let (Some(owner), Some(agent), Some(rel_path)) = (segs.next(), segs.next(), segs.next()) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let (Ok(owner_id), Ok(agent_id)) = (owner.parse::<Uuid>(), agent.parse::<Uuid>()) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let ws = WorkspaceRef {
        owner_id,
        container_id: ContainerId::from_uuid(agent_id),
    };
    match state.runtime.read_workspace_file(&ws, rel_path).await {
        // No `Content-Length`: the size comes from an earlier `stat`, but the
        // bytes come from a separate streaming `cat`, and the file may have been
        // rewritten (or the reader's post-stat containment recheck may end the
        // stream early) in between. A wrong length truncates or hangs the client;
        // chunked transfer streams exactly what the reader yields.
        Ok(wf) => (
            [
                (header::CONTENT_TYPE, file.mime_type.clone()),
                (
                    header::CONTENT_DISPOSITION,
                    // Strip anything that could break out of the quoted filename or
                    // produce an invalid header: quotes, backslashes, and control
                    // characters (a workspace basename can legally contain a
                    // newline). Not just `"` — an incomplete strip risks a broken
                    // HeaderValue or, on lax clients, header confusion.
                    format!(
                        "attachment; filename=\"{}\"",
                        sanitize_filename(&file.filename)
                    ),
                ),
            ],
            Body::from_stream(wf.stream),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(file_id = %file.id, %e, "workspace file stream failed");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

async fn delete_file(
    State(state): State<AppState>,
    claims: Claims,
    Path(file_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
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

#[cfg(test)]
mod tests {
    use super::reply_references_file;

    #[test]
    fn matches_a_named_file_but_not_a_substring_of_a_longer_token() {
        // Named as a real token → captured.
        assert!(reply_references_file(
            "I saved `report.md` for you.",
            "report.md"
        ));
        assert!(reply_references_file(
            "Done. report.md is ready.",
            "report.md"
        ));
        assert!(reply_references_file("See (output.txt).", "output.txt"));
        assert!(reply_references_file("Saved to /tmp/a.py path.", "a.py"));

        // The over-capture the token match exists to stop: `a.py` glued inside a
        // longer alphanumeric run must NOT match.
        assert!(!reply_references_file("the a.python value is 3", "a.py"));
        assert!(!reply_references_file(
            "my-file.txt was written",
            "file.txt"
        ));
        assert!(!reply_references_file(
            "results.json holds it",
            "result.json"
        ));
        assert!(!reply_references_file("no files here", "data.csv"));
        assert!(!reply_references_file("anything", ""));

        // A dotted extension continuation must not cross-capture the shorter name.
        assert!(!reply_references_file("wrote report.md.bak", "report.md"));
        assert!(!reply_references_file("see my.report.md", "report.md"));
        // ...but a trailing sentence period is still a boundary.
        assert!(reply_references_file("Saved report.md.", "report.md"));
        assert!(reply_references_file(
            "Files: report.md, notes.txt",
            "report.md"
        ));
    }
}
