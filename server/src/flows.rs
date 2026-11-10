use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::Claims;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/flows", get(list_flows))
        .route("/flows", post(create_flow))
        .route("/flows/{flow_id}", get(get_flow))
        .route("/flows/{flow_id}/complete", post(complete_flow))
        .route("/flows/{flow_id}/steps", get(list_steps))
        .route("/flows/{flow_id}/steps", post(add_step))
}

/// Why a request could not open its `flows` row.
#[derive(Debug)]
pub enum OpenFlowError {
    /// The flow already exists and belongs to a different user. The `flow_id` is a trace id, and
    /// an inbound `traceparent` is caller-supplied, so a request can name any flow it has seen.
    /// Re-opening or joining it would let the caller act inside that user's flow: whatever the
    /// platform resolves from the flow — whose memory, whose MCP permissions — would resolve to
    /// the other user.
    ForeignFlow,
    Db(sqlx::Error),
}

/// The flow a request is about to open.
pub struct NewFlow<'a> {
    pub flow_id: &'a str,
    pub user_id: Uuid,
    pub root_agent_id: Option<Uuid>,
    pub root_agent_name: &'a str,
    pub title: Option<&'a str>,
    pub metadata: &'a serde_json::Value,
    /// `flows.session_id` (0058): the conversation this flow belongs to, as a column so
    /// `session/list` can group by it. Written on insert only, like the rest of the row; `None`
    /// where the caller has no conversation to name.
    pub session_id: Option<&'a str>,
}

/// The session a flow belongs to: the named conversation, or the flow itself when the caller
/// names none (a single-run session — the same rule as the 0085 backfill).
pub fn session_or_self<'a>(session_id: Option<&'a str>, flow_id: &'a str) -> &'a str {
    session_id.unwrap_or(flow_id)
}

/// Whether `flow_id` already exists and belongs to someone other than `user_id`.
///
/// Callers run this before touching any per-flow state (the flow guard's Redis counters), so a
/// request replaying another user's trace id cannot spend that flow's depth or fan-out budget
/// before [`open_flow`] rejects it.
pub async fn flow_belongs_to_another(
    db: &sqlx::PgPool,
    flow_id: &str,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM flows WHERE flow_id = $1 AND user_id <> $2)")
        .bind(flow_id)
        .bind(user_id)
        .fetch_one(db)
        .await
}

/// Open the `flows` row for a request, or re-open the caller's own (a repeat request under one
/// traceparent must not inherit the `completed` status the previous one left, or strict
/// attribution denies the agent's LLM calls).
///
/// One statement, so ownership cannot change between the check and the write: the conflict
/// branch only updates a row whose `user_id` matches, and a row that doesn't match returns
/// nothing. Never succeed for a flow that belongs to another user — see
/// [`OpenFlowError::ForeignFlow`].
pub async fn open_flow(db: &sqlx::PgPool, flow: NewFlow<'_>) -> Result<(), OpenFlowError> {
    let opened: Option<String> = sqlx::query_scalar(
        r#"INSERT INTO flows (flow_id, user_id, root_agent_id, root_agent_name, title, status, metadata, session_id)
           VALUES ($1, $2, $3, $4, $5, 'running', $6, $7)
           ON CONFLICT (flow_id) DO UPDATE
              SET status = 'running', completed_at = NULL
              WHERE flows.user_id = EXCLUDED.user_id
           RETURNING flow_id"#,
    )
    .bind(flow.flow_id)
    .bind(flow.user_id)
    .bind(flow.root_agent_id)
    .bind(flow.root_agent_name)
    .bind(flow.title)
    .bind(flow.metadata)
    .bind(session_or_self(flow.session_id, flow.flow_id))
    .fetch_optional(db)
    .await
    .map_err(OpenFlowError::Db)?;
    opened.map(|_| ()).ok_or(OpenFlowError::ForeignFlow)
}

/// Record `agent_id` as a participant of `flow_id` — the durable fact that the
/// flow was dispatched to this agent. Awaited (not spawned) so the row is
/// guaranteed present before the agent container can call back into the
/// platform.
///
/// LOAD-BEARING for authorization: the MCP gateway and the LLM router only
/// serve a `tools/call` / LLM call when the authenticated agent is a recorded
/// participant of the flow named by the request's `traceparent`
/// (docs/MCP_GATEWAY_AGENT_AUTH.md §2.4). A missed insert here is a denial for
/// that agent — never an escalation — but it is still an agent outage, so
/// every dispatch path must call this alongside its `flows` insert.
pub async fn record_participant(db: &sqlx::PgPool, flow_id: &str, agent_id: Uuid) {
    if let Err(e) = sqlx::query(
        "INSERT INTO flow_participants (flow_id, agent_id) VALUES ($1, $2)
         ON CONFLICT (flow_id, agent_id) DO NOTHING",
    )
    .bind(flow_id)
    .bind(agent_id)
    .execute(db)
    .await
    {
        tracing::warn!(
            error = %e, %flow_id, %agent_id,
            "flow participant record failed — this agent's MCP/LLM calls in this flow will be denied"
        );
    }
}

/// Withdraw a turn the session refused (busy, superseded, or failed to start): drop the agent's
/// participant row so the flow no longer counts as one of the session's turns, and fail the flow
/// when it is still running with no other participant. Best effort: a failed write only leaves
/// the flow as it was.
pub async fn abandon_rejected(db: &sqlx::PgPool, flow_id: &str, agent_id: Uuid) {
    let dropped = sqlx::query("DELETE FROM flow_participants WHERE flow_id = $1 AND agent_id = $2")
        .bind(flow_id)
        .bind(agent_id)
        .execute(db)
        .await;
    let failed = sqlx::query(
        "UPDATE flows SET status = 'failed', error_message = 'turn rejected', completed_at = now()
          WHERE flow_id = $1 AND status = 'running'
            AND NOT EXISTS (SELECT 1 FROM flow_participants WHERE flow_id = $1)",
    )
    .bind(flow_id)
    .execute(db)
    .await;
    for result in [dropped, failed] {
        if let Err(e) = result {
            tracing::warn!(error = %e, %flow_id, "abandoning a rejected turn's flow failed");
        }
    }
}

// ─── Models ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Flow {
    pub id: Uuid,
    pub flow_id: String,
    pub user_id: Uuid,
    pub root_agent_id: Option<Uuid>,
    pub root_agent_name: Option<String>,
    pub title: Option<String>,
    pub status: String,
    pub max_depth_reached: i32,
    pub total_invocations: i32,
    pub total_tokens_used: i64,
    pub total_cost_usd: Option<rust_decimal::Decimal>,
    pub duration_ms: Option<i64>,
    pub error_message: Option<String>,
    pub metadata: sqlx::types::Json<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct FlowStep {
    pub id: Uuid,
    pub flow_id: String,
    pub step_order: i32,
    pub depth: i32,
    pub agent_id: Option<Uuid>,
    pub agent_name: String,
    pub caller_agent_name: Option<String>,
    pub input_summary: Option<String>,
    pub output_summary: Option<String>,
    pub status: String,
    pub tokens_used: i32,
    pub latency_ms: Option<i32>,
    pub error_message: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

// ─── Handlers ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    status: Option<String>,
    q: Option<String>,
}
fn default_limit() -> i64 {
    50
}

async fn list_flows(
    State(state): State<AppState>,
    claims: Claims,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let rows = sqlx::query_as::<_, Flow>(
        r#"SELECT * FROM flows
           WHERE user_id = $1
             AND ($2::text IS NULL OR status = $2)
             AND ($3::text IS NULL OR root_agent_name ILIKE '%' || $3 || '%' OR title ILIKE '%' || $3 || '%')
           ORDER BY created_at DESC
           LIMIT $4 OFFSET $5"#,
    )
    .bind(user_id)
    .bind(&q.status)
    .bind(&q.q)
    .bind(q.limit)
    .bind(q.offset)
    .fetch_all(&state.db)
    .await;

    match rows {
        Ok(data) => Json(crate::Paginated::new(data)).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, "list_flows: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn get_flow(
    State(state): State<AppState>,
    claims: Claims,
    Path(flow_id): Path<String>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let flow = sqlx::query_as::<_, Flow>(
        "SELECT * FROM flows WHERE flow_id = $1 AND ($2 OR user_id = $3)",
    )
    .bind(&flow_id)
    .bind(claims.is_superuser)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;

    match flow {
        Ok(Some(f)) => {
            let steps = sqlx::query_as::<_, FlowStep>(
                "SELECT * FROM flow_steps WHERE flow_id = $1 ORDER BY step_order ASC",
            )
            .bind(&flow_id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

            Json(serde_json::json!({
                "flow": f,
                "steps": steps
            }))
            .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, %flow_id, "get_flow: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreateFlowRequest {
    flow_id: String,
    root_agent_id: Option<Uuid>,
    root_agent_name: Option<String>,
    title: Option<String>,
    metadata: Option<serde_json::Value>,
    /// Session this flow belongs to; defaults to the flow itself (single-run session).
    session_id: Option<String>,
}

async fn create_flow(
    State(state): State<AppState>,
    claims: Claims,
    Json(body): Json<CreateFlowRequest>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let meta = body.metadata.unwrap_or(serde_json::json!({}));

    // A named session must not belong to someone else: it would attach this flow's traces
    // and cost to another user's session view.
    if let Some(sid) = body.session_id.as_deref() {
        let foreign: Result<bool, sqlx::Error> = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM chat_sessions WHERE session_id = $1 AND user_id <> $2) \
                 OR EXISTS (SELECT 1 FROM flows WHERE session_id = $1 AND user_id <> $2)",
        )
        .bind(sid)
        .bind(user_id)
        .fetch_one(&state.db)
        .await;
        match foreign {
            Ok(false) => {}
            Ok(true) => {
                return (StatusCode::FORBIDDEN, "session belongs to another user").into_response();
            }
            Err(e) => {
                tracing::error!(%e, %user_id, "create_flow: session ownership check failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
            }
        }
    }

    // The flow id doubles as a trace id. Claiming one that already carries another user's
    // usage would authorize this caller against that user's trace through the new mapping.
    let trace_taken: Result<bool, sqlx::Error> = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM trace_usage WHERE trace_id = $1 AND user_id IS DISTINCT FROM $2) \
             OR EXISTS (SELECT 1 FROM tool_calls WHERE trace_id = $1 AND user_id IS DISTINCT FROM $2)",
    )
    .bind(&body.flow_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await;
    match trace_taken {
        Ok(false) => {}
        Ok(true) => {
            return (StatusCode::FORBIDDEN, "trace belongs to another user").into_response();
        }
        Err(e) => {
            tracing::error!(%e, %user_id, "create_flow: trace ownership check failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    }

    let result = sqlx::query_as::<_, Flow>(
        r#"INSERT INTO flows (flow_id, user_id, root_agent_id, root_agent_name, title, metadata, session_id)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           RETURNING *"#,
    )
    .bind(&body.flow_id)
    .bind(user_id)
    .bind(body.root_agent_id)
    .bind(&body.root_agent_name)
    .bind(&body.title)
    .bind(sqlx::types::Json(meta))
    .bind(session_or_self(body.session_id.as_deref(), &body.flow_id))
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(flow) => (StatusCode::CREATED, Json(flow)).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, flow_id = %body.flow_id, "create_flow: db error");
            (
                StatusCode::CONFLICT,
                "flow already exists or invalid reference",
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct CompleteFlowRequest {
    status: String,
    total_tokens_used: Option<i64>,
    total_cost_usd: Option<f64>,
    error_message: Option<String>,
    max_depth_reached: Option<i32>,
    total_invocations: Option<i32>,
}

async fn complete_flow(
    State(state): State<AppState>,
    claims: Claims,
    Path(flow_id): Path<String>,
    Json(body): Json<CompleteFlowRequest>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let result = sqlx::query_as::<_, Flow>(
        r#"UPDATE flows SET
             status = $3,
             total_tokens_used = COALESCE($4, total_tokens_used),
             total_cost_usd = COALESCE($5, total_cost_usd),
             error_message = $6,
             max_depth_reached = COALESCE($7, max_depth_reached),
             total_invocations = COALESCE($8, total_invocations),
             duration_ms = EXTRACT(EPOCH FROM (now() - created_at))::bigint * 1000,
             completed_at = now()
           WHERE flow_id = $1 AND user_id = $2
           RETURNING *"#,
    )
    .bind(&flow_id)
    .bind(user_id)
    .bind(&body.status)
    .bind(body.total_tokens_used)
    .bind(
        body.total_cost_usd
            .map(rust_decimal::Decimal::try_from)
            .and_then(Result::ok),
    )
    .bind(&body.error_message)
    .bind(body.max_depth_reached)
    .bind(body.total_invocations)
    .fetch_optional(&state.db)
    .await;

    match result {
        Ok(Some(f)) => Json(f).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, %flow_id, "complete_flow: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

/// Whether the caller owns the flow, or is a superuser (who may open anyone's, as
/// `authorize_session` lets them open anyone's session).
async fn can_see_flow(state: &AppState, claims: &Claims, user_id: Uuid, flow_id: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM flows WHERE flow_id = $1 AND ($2 OR user_id = $3))",
    )
    .bind(flow_id)
    .bind(claims.is_superuser)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false)
}

async fn list_steps(
    State(state): State<AppState>,
    claims: Claims,
    Path(flow_id): Path<String>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    if !can_see_flow(&state, &claims, user_id, &flow_id).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    let steps = sqlx::query_as::<_, FlowStep>(
        "SELECT * FROM flow_steps WHERE flow_id = $1 ORDER BY step_order ASC",
    )
    .bind(&flow_id)
    .fetch_all(&state.db)
    .await;

    match steps {
        Ok(data) => Json(data).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, %flow_id, "list_steps: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct AddStepRequest {
    agent_id: Option<Uuid>,
    agent_name: String,
    caller_agent_name: Option<String>,
    depth: Option<i32>,
    input_summary: Option<String>,
}

async fn add_step(
    State(state): State<AppState>,
    claims: Claims,
    Path(flow_id): Path<String>,
    Json(body): Json<AddStepRequest>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    if !can_see_flow(&state, &claims, user_id, &flow_id).await {
        return StatusCode::NOT_FOUND.into_response();
    }

    let next_order: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(step_order), 0) + 1 FROM flow_steps WHERE flow_id = $1",
    )
    .bind(&flow_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(1);

    let result = sqlx::query_as::<_, FlowStep>(
        r#"INSERT INTO flow_steps (flow_id, step_order, depth, agent_id, agent_name, caller_agent_name, input_summary, status)
           VALUES ($1, $2, $3, $4, $5, $6, $7, 'running')
           RETURNING *"#,
    )
    .bind(&flow_id)
    .bind(next_order)
    .bind(body.depth.unwrap_or(0))
    .bind(body.agent_id)
    .bind(&body.agent_name)
    .bind(&body.caller_agent_name)
    .bind(&body.input_summary)
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(step) => (StatusCode::CREATED, Json(step)).into_response(),
        Err(e) => {
            tracing::error!(%e, %user_id, %flow_id, "add_step: db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::session_or_self;

    #[test]
    fn named_session_wins() {
        assert_eq!(session_or_self(Some("ctx-1"), "flow-1"), "ctx-1");
    }

    #[test]
    fn missing_session_falls_back_to_flow() {
        assert_eq!(session_or_self(None, "flow-1"), "flow-1");
    }
}
