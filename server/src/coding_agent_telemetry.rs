use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::post,
};
use chrono::{DateTime, Utc};
use nasiko_types::{
    CODING_AGENT_BATCH_MAX_EVENTS, CodingAgentEventBatchRequest, CodingAgentEventBatchResponse,
    CodingAgentEventResult, CodingAgentEventStatus, CodingAgentEventV1, CodingAgentToolCallStatus,
    CodingAgentTurn, coding_agent_session_id,
};
use serde_json::json;
use uuid::Uuid;

use crate::auth::Claims;
use crate::coding_agent_subagents::{link_parent, same_turn_but_parent};
use crate::mcp::ApiResponse;
use crate::state::AppState;

pub(crate) const CODING_SESSION_PLACEHOLDER: &str = "Coding session";

/// Largest request body the batch route reads. A full batch of content turns
/// fits; a client that hits it splits the batch.
const MAX_BATCH_BYTES: usize = 8 * 1024 * 1024;

/// A turn that ended longer than this before the server received it is history
/// (an import, or a laptop catching up), not a live session: background work
/// takes it after every live turn. Stored per receipt as `otlp_historical`.
pub(crate) const HISTORICAL_AFTER: &str = "1 hour";

/// The body limit applies to the route; the concurrency cap is a route layer,
/// so it runs before the body is read and an overloaded server turns a batch
/// away without buffering it first.
pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/telemetry/coding-agent/events/batch", post(ingest_batch))
        .route_layer(middleware::from_fn_with_state(
            state,
            crate::coding_agent_ingest_guard::limit_concurrency,
        ))
        .layer(DefaultBodyLimit::max(MAX_BATCH_BYTES))
}

/// Acknowledges a batch only once every accepted turn in it is committed: each
/// turn commits its own transaction before the next starts, and any failure to
/// persist one answers `500` before a response is sent, so a client that sees
/// `200` has nothing to resend and a client that sees anything else resends
/// the whole batch — turns already committed come back `duplicate`.
async fn ingest_batch(
    State(state): State<AppState>,
    claims: Claims,
    batch: Result<Json<CodingAgentEventBatchRequest>, JsonRejection>,
) -> axum::response::Response {
    let guard = &state.coding_agent_ingest;
    let batch = match batch {
        Ok(Json(batch)) => batch,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return guard.too_large(&format!(
                "request body exceeds the {} MiB batch limit",
                MAX_BATCH_BYTES / (1024 * 1024)
            ));
        }
        Err(rejection) => return rejection.into_response(),
    };
    if batch.events.len() > CODING_AGENT_BATCH_MAX_EVENTS {
        return guard.too_large(&format!(
            "batch has {} events; at most {CODING_AGENT_BATCH_MAX_EVENTS} are accepted per request",
            batch.events.len()
        ));
    }
    if let Err(error) = batch.validate() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"data": null, "status_code": 400, "message": error})),
        )
            .into_response();
    }
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    if let Err(response) = guard.admit_events(&claims.sub, batch.events.len()) {
        return response;
    }
    let tempo_cutoff = tempo_cutoff(state.config.coding_agent_ingest.tempo_max_age_secs);
    let mut results = Vec::with_capacity(batch.events.len());
    for event in batch.events {
        let event_id = event.event_id.clone();
        match process_event(&state, user_id, &event, tempo_cutoff).await {
            Ok(status) => results.push(CodingAgentEventResult {
                event_id,
                status,
                error: None,
            }),
            Err(ProcessError::Rejected(error)) => results.push(CodingAgentEventResult {
                event_id,
                status: CodingAgentEventStatus::Rejected,
                error: Some(error),
            }),
            Err(ProcessError::Internal(error)) => {
                record_outcomes(guard, &results);
                tracing::error!(%error, %user_id, %event_id, "coding-agent event ingestion failed");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "data": null,
                        "status_code": 500,
                        "message": "failed to persist coding-agent telemetry"
                    })),
                )
                    .into_response();
            }
        }
    }
    record_outcomes(guard, &results);
    ApiResponse::ok(
        serde_json::to_value(CodingAgentEventBatchResponse { results })
            .expect("batch response serializes"),
        "coding-agent telemetry processed",
    )
    .into_response()
}

/// Turns that started before this are older than Tempo will ingest. `None`
/// when the configured age reaches past the representable range: nothing is.
fn tempo_cutoff(max_age_secs: u64) -> Option<DateTime<Utc>> {
    let max_age = chrono::Duration::try_seconds(i64::try_from(max_age_secs).ok()?)?;
    Utc::now().checked_sub_signed(max_age)
}

fn record_outcomes(
    guard: &crate::coding_agent_ingest_guard::IngestGuard,
    results: &[CodingAgentEventResult],
) {
    let count = |status: CodingAgentEventStatus| {
        results.iter().filter(|r| r.status == status).count() as u64
    };
    guard.metrics.record_events(
        count(CodingAgentEventStatus::Accepted),
        count(CodingAgentEventStatus::Duplicate),
        count(CodingAgentEventStatus::Rejected),
    );
}

enum ProcessError {
    Rejected(String),
    Internal(anyhow::Error),
}

impl From<sqlx::Error> for ProcessError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// Persist one telemetry event for a *container* ACP coding agent (NAS-841).
///
/// Unlike the CLI batch path ([`process_event`]), a container agent is already
/// identified by its `agent_id` (so there is no `coding_agent_integration_id`
/// lookup), and the turn's chat session + messages were already written by the
/// agent proxy — so this only records the telemetry row the OTLP outbox exports,
/// attached to the proxy's existing `session_id` (the A2A contextId). No chat
/// session or message is written here, avoiding duplicates. Idempotent on
/// `(user_id, event_id)`; failures are the caller's to log, never fatal to a turn.
///
/// Spend is not written here: the container's LLM calls go through the router,
/// which upserts `trace_usage` with `source='acp'` per call.
pub async fn persist_coding_turn_telemetry(
    db: &sqlx::PgPool,
    user_id: Uuid,
    agent_id: Uuid,
    session_id: &str,
    event: &CodingAgentEventV1,
) -> anyhow::Result<()> {
    let payload = serde_json::to_value(event)?;
    sqlx::query(
        r#"INSERT INTO coding_agent_telemetry_events
             (user_id, event_id, payload, agent_id, agent_name, source_agent_id,
              session_id, source_session_id, turn_id, captured_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
           ON CONFLICT (user_id, event_id) DO NOTHING"#,
    )
    .bind(user_id)
    .bind(&event.event_id)
    .bind(&payload)
    .bind(agent_id)
    .bind(&event.source.agent_name)
    .bind(&event.source.agent_id)
    .bind(session_id)
    .bind(&event.session.source_id)
    .bind(&event.turn.id)
    .bind(event.captured_at)
    .execute(db)
    .await?;
    Ok(())
}

async fn process_event(
    state: &AppState,
    user_id: Uuid,
    event: &CodingAgentEventV1,
    tempo_cutoff: Option<DateTime<Utc>>,
) -> Result<CodingAgentEventStatus, ProcessError> {
    // A coding agent's conversation is never stored, whatever an older CLI still sends: only
    // what prices the turn is kept — no prompt, response, session title or tool bodies.
    let event = &event.clone().without_content();
    event.validate().map_err(ProcessError::Rejected)?;
    let expected_session_id =
        coding_agent_session_id(&event.source.agent_id, &event.session.source_id);
    if event.session.id != expected_session_id {
        return Err(ProcessError::Rejected(
            "session.id does not match the namespaced source session identity".into(),
        ));
    }

    // Pricing and the router relabel acquire their own database connections. Do
    // not hold the write transaction while running them: a saturated pool would
    // deadlock. The agent is looked up here for the relabel, and again inside the
    // transaction, which is what authorizes the write.
    let agent_id = owned_agent_id(&state.db, user_id, event).await?;
    let server_session_id = scoped_session_id(agent_id, &event.session.source_id);
    let usage = {
        let mut exported = event.clone();
        exported.session.id = server_session_id.clone();
        crate::coding_agent_otlp::relabel_from_router(&state.db, agent_id, &mut exported).await;
        crate::coding_agent_usage::turn_usage_row(&state.pricing, &exported).await
    };
    let mut tx = state.db.begin().await?;
    if owned_agent_id(&mut *tx, user_id, event).await? != agent_id {
        return Err(ProcessError::Internal(anyhow::anyhow!(
            "coding-agent integration was replaced while the turn was being ingested"
        )));
    }

    sqlx::query(
        r#"INSERT INTO chat_sessions
              (session_id, user_id, agent_id, title, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6)
           ON CONFLICT (session_id) DO NOTHING"#,
    )
    .bind(&server_session_id)
    .bind(user_id)
    .bind(agent_id)
    .bind(CODING_SESSION_PLACEHOLDER)
    .bind(event.turn.started_at)
    .bind(event.turn.ended_at)
    .execute(&mut *tx)
    .await?;
    let session_matches: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
             SELECT 1 FROM chat_sessions
             WHERE session_id = $1 AND user_id = $2 AND agent_id = $3 AND deleted_at IS NULL
           )"#,
    )
    .bind(&server_session_id)
    .bind(user_id)
    .bind(agent_id)
    .fetch_one(&mut *tx)
    .await?;
    if !session_matches {
        return Err(ProcessError::Rejected(
            "session already belongs to a different user or agent".into(),
        ));
    }
    // Before the receipt, so a replay that only adds the link (below) records it.
    // The parent's id is scoped exactly as the child's: same agent, same rule.
    if let Some(parent) = &event.session.parent {
        let parent_session_id = scoped_session_id(agent_id, &parent.source_id);
        link_parent(&mut tx, &server_session_id, &parent_session_id, parent).await?;
    }

    let payload =
        serde_json::to_value(event).map_err(|error| ProcessError::Internal(error.into()))?;
    let summary = TurnSummary::of(&event.turn);
    // The receipt is born materialized: its `trace_usage` row is written below,
    // in this transaction, so the Tempo read-back never processes it. A turn
    // older than Tempo's ingestion window is born `skipped`: exporting it would
    // be dropped by Tempo, and nothing needs it there.
    let inserted = sqlx::query(
        r#"INSERT INTO coding_agent_telemetry_events
             (user_id, event_id, payload, agent_id, agent_name, source_agent_id,
              session_id, source_session_id, turn_id, captured_at,
              turn_started_at, turn_ended_at, turn_tokens, turn_tool_errors,
              otlp_state, otlp_historical, materialized_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                   CASE WHEN $11 < $15 THEN 'skipped' ELSE 'pending' END,
                   $10 < now() - $16::interval,
                   now())
           ON CONFLICT (user_id, event_id) DO NOTHING"#,
    )
    .bind(user_id)
    .bind(&event.event_id)
    .bind(&payload)
    .bind(agent_id)
    .bind(&event.source.agent_name)
    .bind(&event.source.agent_id)
    .bind(&server_session_id)
    .bind(&event.session.source_id)
    .bind(&event.turn.id)
    .bind(event.captured_at)
    .bind(event.turn.started_at)
    .bind(event.turn.ended_at)
    .bind(summary.tokens)
    .bind(summary.tool_errors)
    .bind(tempo_cutoff)
    .bind(HISTORICAL_AFTER)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !inserted {
        let stored: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM coding_agent_telemetry_events WHERE user_id = $1 AND event_id = $2",
        )
        .bind(user_id)
        .bind(&event.event_id)
        .fetch_one(&mut *tx)
        .await?;
        if !is_replay(&stored, &payload) {
            return Err(ProcessError::Rejected(
                "event_id already exists with a different payload".into(),
            ));
        }
        tx.commit().await?;
        return Ok(CodingAgentEventStatus::Duplicate);
    }

    sqlx::query(
        "UPDATE chat_sessions SET created_at = LEAST(created_at, $2) WHERE session_id = $1",
    )
    .bind(&server_session_id)
    .bind(event.turn.started_at)
    .execute(&mut *tx)
    .await?;
    if let Some(usage) = &usage {
        crate::observability::trace_materializer::write_usage_row(
            &mut tx,
            usage,
            crate::observability::trace_materializer::UsageSource::Coding,
            Some(&server_session_id),
            Some(agent_id),
            Some(user_id),
        )
        .await?;
    }
    crate::coding_agent_otlp::write_tool_calls(
        &mut tx,
        event,
        &server_session_id,
        agent_id,
        user_id,
    )
    .await?;
    tx.commit().await?;
    Ok(CodingAgentEventStatus::Accepted)
}

/// Whether `incoming` is a replay of the receipt already `stored` for its event id: the same
/// turn, perhaps now carrying its subagent parent, or a turn an older CLI stored with its
/// conversation, which is now received — and kept — without it.
fn is_replay(stored: &serde_json::Value, incoming: &serde_json::Value) -> bool {
    let without_content = serde_json::from_value::<CodingAgentEventV1>(stored.clone())
        .ok()
        .and_then(|event| serde_json::to_value(event.without_content()).ok());
    std::iter::once(stored)
        .chain(without_content.as_ref())
        .any(|stored| stored == incoming || same_turn_but_parent(stored, incoming))
}

/// The caller's live registration of the turn's coding agent, or the rejection
/// for a turn reported against one they do not have.
async fn owned_agent_id<'e>(
    db: impl sqlx::PgExecutor<'e>,
    user_id: Uuid,
    event: &CodingAgentEventV1,
) -> Result<Uuid, ProcessError> {
    let agent_id: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT id FROM agents
           WHERE owner_id = $1 AND name = $2
             AND coding_agent_integration_id = $3
             AND deleted_at IS NULL"#,
    )
    .bind(user_id)
    .bind(&event.source.agent_name)
    .bind(&event.source.agent_id)
    .fetch_optional(db)
    .await?;
    agent_id.ok_or_else(|| {
        ProcessError::Rejected(format!(
            "active owned {} coding-agent integration '{}' not found",
            event.source.agent_id, event.source.agent_name
        ))
    })
}

/// The typed columns `session/list` totals a coding session from, so it never has to read the
/// payload (prompt, response and tool outputs). Same rules as `coding_agent_turn_summary_backfill`.
#[derive(Debug, PartialEq, Eq)]
struct TurnSummary {
    /// Every token class over every LLM call, saturating at `i64::MAX`.
    tokens: i64,
    /// Tool calls that ended in an error. Denied and cancelled calls are the user's decision.
    tool_errors: i32,
}

impl TurnSummary {
    fn of(turn: &CodingAgentTurn) -> Self {
        let tokens = turn
            .llm_calls
            .iter()
            .flat_map(|call| {
                [
                    call.input_tokens,
                    call.output_tokens,
                    call.cache_read_tokens,
                    call.cache_creation_tokens,
                ]
            })
            .fold(0_u64, u64::saturating_add);
        let tool_errors = turn
            .tool_calls
            .iter()
            .filter(|call| {
                matches!(
                    call.status,
                    CodingAgentToolCallStatus::Failed | CodingAgentToolCallStatus::TimedOut
                )
            })
            .count();
        Self {
            tokens: i64::try_from(tokens).unwrap_or(i64::MAX),
            tool_errors: i32::try_from(tool_errors).unwrap_or(i32::MAX),
        }
    }
}

fn scoped_session_id(agent_id: Uuid, source_session_id: &str) -> String {
    const NAMESPACE: Uuid = Uuid::from_u128(0x9268448b_45e4_466d_a7d7_587e0fd47272);
    Uuid::new_v5(
        &NAMESPACE,
        format!("{agent_id}\0{source_session_id}").as_bytes(),
    )
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_types::{
        CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentLlmCall, CodingAgentSession,
        CodingAgentSource, CodingAgentTimestampQuality, CodingAgentToolAssociation,
        CodingAgentToolCall, CodingAgentTurn,
    };

    fn event() -> CodingAgentEventV1 {
        let at = chrono::Utc::now();
        CodingAgentEventV1 {
            version: CODING_AGENT_EVENT_VERSION,
            event_id: "event".into(),
            captured_at: at,
            source: CodingAgentSource {
                agent_id: "claude".into(),
                agent_name: "coding-agent".into(),
            },
            session: CodingAgentSession {
                id: "session".into(),
                source_id: "source-session".into(),
                title: None,
                parent: None,
            },
            turn: CodingAgentTurn {
                id: "turn".into(),
                prompt: Some("question".into()),
                response: Some("answer".into()),
                started_at: at,
                ended_at: at,
                llm_calls: ["gpt-4o", "gpt-4o-mini"]
                    .into_iter()
                    .map(|model| CodingAgentLlmCall {
                        id: model.into(),
                        model: model.into(),
                        provider: "openai".into(),
                        input_tokens: 1000,
                        output_tokens: 500,
                        cache_read_tokens: 2000,
                        cache_creation_tokens: 100,
                        accounting: None,
                        started_at: at,
                        ended_at: at,
                    })
                    .collect(),
                tool_calls: vec![],
            },
            capture_policy: CapturePolicy::Content,
        }
    }

    #[test]
    fn a_turn_stored_with_its_conversation_is_replayed_without_it() {
        let stored = serde_json::to_value(event()).unwrap();
        let replay = serde_json::to_value(event().without_content()).unwrap();
        assert!(is_replay(&stored, &replay));
        assert!(is_replay(&replay, &replay));

        let mut other = event().without_content();
        other.turn.llm_calls[0].input_tokens += 1;
        let other = serde_json::to_value(other).unwrap();
        assert!(!is_replay(&stored, &other));
    }

    fn tool_call(status: CodingAgentToolCallStatus) -> CodingAgentToolCall {
        CodingAgentToolCall {
            id: format!("{status:?}"),
            name: "bash".into(),
            kind: "shell".into(),
            model_call_id: None,
            status,
            arguments: None,
            output: None,
            raw: None,
            error: None,
            started_at: None,
            ended_at: None,
            duration_ms: None,
            association: CodingAgentToolAssociation::Turn,
            timestamp_quality: CodingAgentTimestampQuality::Unknown,
        }
    }

    #[test]
    fn turn_summary_sums_every_token_class_and_counts_only_erroring_tools() {
        use CodingAgentToolCallStatus::*;
        let mut event = event();
        event.turn.tool_calls = [Succeeded, Failed, TimedOut, Denied, Cancelled, Running]
            .into_iter()
            .map(tool_call)
            .collect();
        // Two calls of 1000 + 500 + 2000 + 100.
        assert_eq!(
            TurnSummary::of(&event.turn),
            TurnSummary {
                tokens: 7200,
                tool_errors: 2
            }
        );
    }

    #[test]
    fn turn_summary_saturates_oversized_token_counts() {
        let mut event = event();
        for call in &mut event.turn.llm_calls {
            call.input_tokens = u64::MAX;
        }
        assert_eq!(TurnSummary::of(&event.turn).tokens, i64::MAX);
    }
}
