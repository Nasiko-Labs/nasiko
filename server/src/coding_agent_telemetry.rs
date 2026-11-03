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
    CODING_AGENT_BATCH_MAX_EVENTS, CapturePolicy, CodingAgentEventBatchRequest,
    CodingAgentEventBatchResponse, CodingAgentEventResult, CodingAgentEventStatus,
    CodingAgentEventV1, CodingAgentToolCallStatus, CodingAgentTurn, coding_agent_session_id,
};
use serde_json::json;
use uuid::Uuid;

use crate::auth::Claims;
use crate::chat::external_turn::{PersistExternalTurnError, persist_external_turn};
use crate::chat::models::{ExternalTurn, MessageUsage};
use crate::coding_agent_subagents::{link_parent, same_turn_but_parent};
use crate::mcp::ApiResponse;
use crate::state::AppState;

const CODING_SESSION_PLACEHOLDER: &str = "Coding session";

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
    let cost = if event.capture_policy == CapturePolicy::Content {
        price_turn(&state.pricing, event).await
    } else {
        None
    };
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
    .bind(
        event
            .session
            .title
            .as_deref()
            .unwrap_or(CODING_SESSION_PLACEHOLDER),
    )
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
        if stored != payload && !same_turn_but_parent(&stored, &payload) {
            return Err(ProcessError::Rejected(
                "event_id already exists with a different payload".into(),
            ));
        }
        tx.commit().await?;
        return Ok(CodingAgentEventStatus::Duplicate);
    }

    // Replays must never rename a session; only newly accepted receipts can upgrade it.
    if let Some(title) = &event.session.title {
        sqlx::query(
            r#"UPDATE chat_sessions SET title = $4
               WHERE session_id = $1 AND user_id = $2 AND agent_id = $3
                 AND deleted_at IS NULL AND title = $5"#,
        )
        .bind(&server_session_id)
        .bind(user_id)
        .bind(agent_id)
        .bind(title)
        .bind(CODING_SESSION_PLACEHOLDER)
        .execute(&mut *tx)
        .await?;
    }

    if event.capture_policy == CapturePolicy::Content {
        let mut turn = external_turn(event, &server_session_id);
        if let (Some(usage), Some((cost, estimated))) = (turn.assistant_usage.as_mut(), cost) {
            usage.cost_usd = Some(cost);
            usage.estimated = Some(estimated);
        }
        persist_external_turn(
            &mut tx,
            &server_session_id,
            &turn,
            event.turn.started_at,
            false,
        )
        .await
        .map_err(|error| match error {
            PersistExternalTurnError::Incomplete | PersistExternalTurnError::Conflict => {
                ProcessError::Rejected(error.to_string())
            }
            PersistExternalTurnError::Database(error) => ProcessError::Internal(error.into()),
        })?;
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
            Some(&server_session_id),
            Some(agent_id),
            Some(user_id),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(CodingAgentEventStatus::Accepted)
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

/// Cost the turn through the platform's pricing engine.
///
/// Coding-agent turns carried no cost at all, so a session that spent hundreds
/// of dollars showed a blank in the chat view while the observability page,
/// pricing the same spans, showed the real figure. The counts here are the
/// agent's own and exact, so this is a lookup rather than an estimate.
///
/// Priced per call rather than per turn: a turn can switch models mid-way (a
/// sub-agent on a cheaper tier), and pricing the summed tokens against one
/// model's rate would charge the whole turn at whichever model happened to be
/// reported. Failure is non-fatal — an unpriced turn is worth keeping.
async fn price_turn(
    pricing: &nasiko_pricing::PricingEngine,
    event: &CodingAgentEventV1,
) -> Option<(rust_decimal::Decimal, bool)> {
    if event.turn.llm_calls.is_empty() {
        return None;
    }
    let mut total = nasiko_pricing::CostBreakdown::default();
    for call in &event.turn.llm_calls {
        let context = call
            .accounting
            .as_ref()
            .map(|a| nasiko_pricing::PricingContext {
                cache_creation_5m: a.cache_creation_5m_tokens,
                cache_creation_1h: a.cache_creation_1h_tokens,
                speed: a.speed.as_deref(),
                service_tier: a.service_tier.as_deref(),
                inference_geo: a.inference_geo.as_deref(),
                conflicting_observations: a.conflicting_observations,
            })
            .unwrap_or_default();
        let priced = pricing
            .price_with_context(
                Some(&call.provider),
                &call.model,
                nasiko_pricing::RawUsage {
                    input: call.input_tokens,
                    output: call.output_tokens,
                    cache_read: call.cache_read_tokens,
                    cache_creation: call.cache_creation_tokens,
                    total: None,
                },
                // Coding-agent transcripts report the prompt disjoint from the
                // cache counts, which is Anthropic's convention and what the
                // adapters normalize the others to.
                nasiko_pricing::PromptConvention::Exclusive,
                call.started_at,
                context,
            )
            .await;
        total.add(priced.cost);
    }
    // Retain legitimate zero costs and round to the engine's micro-dollar precision.
    let cost = rust_decimal::Decimal::from_f64_retain(total.total_usd)?;
    Some((cost.round_dp(6), total.estimated))
}

/// `chat_messages` usage columns are `INTEGER`; a long coding-agent session can
/// exceed that in cache reads alone (one real transcript reported 187M), so the
/// conversion saturates rather than wrapping to a negative count.
fn saturating_i32(value: u64) -> i32 {
    value.min(i32::MAX as u64) as i32
}

fn external_turn(event: &CodingAgentEventV1, session_id: &str) -> ExternalTurn {
    let mut scoped_event = event.clone();
    scoped_event.session.id = session_id.to_string();
    let calls = &event.turn.llm_calls;
    // The four classes are kept apart. Folding the cached counts into `input`
    // — which is what this did — loses the only information that explains why a
    // turn was cheap, and leaves the chat view reporting a whole prompt as if
    // every token of it were fresh. The agents report the split exactly (an
    // Anthropic transcript carries `cache_read_input_tokens` per call), so
    // there is nothing to infer here.
    let input_tokens = calls
        .iter()
        .fold(0_u64, |total, call| total.saturating_add(call.input_tokens));
    let cache_read_tokens = calls.iter().fold(0_u64, |total, call| {
        total.saturating_add(call.cache_read_tokens)
    });
    let cache_creation_tokens = calls.iter().fold(0_u64, |total, call| {
        total.saturating_add(call.cache_creation_tokens)
    });
    let output_tokens = calls.iter().fold(0_u64, |total, call| {
        total.saturating_add(call.output_tokens)
    });
    let duration_ms = calls.iter().fold(0_i64, |total, call| {
        total.saturating_add((call.ended_at - call.started_at).num_milliseconds().max(0))
    });
    let model = calls.first().map(|first| {
        if calls.iter().all(|call| call.model == first.model) {
            first.model.clone()
        } else {
            calls.last().expect("calls is nonempty").model.clone()
        }
    });
    ExternalTurn {
        turn_id: event.turn.id.clone(),
        user_content: event.turn.prompt.clone().expect("content event validated"),
        assistant_content: event
            .turn
            .response
            .clone()
            .expect("content event validated"),
        assistant_usage: Some(MessageUsage {
            input_tokens: Some(saturating_i32(input_tokens)),
            output_tokens: Some(saturating_i32(output_tokens)),
            cache_read_tokens: Some(saturating_i32(cache_read_tokens)),
            cache_creation_tokens: Some(saturating_i32(cache_creation_tokens)),
            model,
            duration_ms: Some(duration_ms.min(i32::MAX as i64) as i32),
            // Filled in by the caller, which holds the pricing engine. Left
            // `None` here rather than zero: a coding-agent turn showing no cost
            // at all is what this used to do, and zero would be worse — it
            // reads as "this turn was free" instead of "not priced yet".
            cost_usd: None,
            estimated: None,
            trace_id: Some(crate::coding_agent_otlp::trace_id_for_event(&scoped_event)),
        }),
        assistant_metadata: Some(serde_json::Map::from_iter([(
            "coding_agent".to_string(),
            json!({
                "capture_policy": "content",
                "tool_calls": event.turn.tool_calls,
            }),
        )])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_types::{
        CODING_AGENT_EVENT_VERSION, CodingAgentLlmCall, CodingAgentSession, CodingAgentSource,
        CodingAgentTimestampQuality, CodingAgentToolAssociation, CodingAgentToolCall,
        CodingAgentTurn,
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

    #[tokio::test]
    async fn mixed_model_turns_keep_four_classes_and_sum_each_calls_cost() {
        let event = event();
        let pricing = nasiko_pricing::PricingEngine::offline();
        let turn = external_turn(&event, "scoped-session");
        let (cost, estimated) = price_turn(&pricing, &event).await.unwrap();
        let usage = turn.assistant_usage.unwrap();
        assert_eq!(usage.input_tokens, Some(2000));
        assert_eq!(usage.output_tokens, Some(1000));
        assert_eq!(usage.cache_read_tokens, Some(4000));
        assert_eq!(usage.cache_creation_tokens, Some(200));
        let mut expected = nasiko_pricing::CostBreakdown::default();
        for call in &event.turn.llm_calls {
            expected.add(
                pricing
                    .price(
                        Some(&call.provider),
                        &call.model,
                        nasiko_pricing::RawUsage {
                            input: call.input_tokens,
                            output: call.output_tokens,
                            cache_read: call.cache_read_tokens,
                            cache_creation: call.cache_creation_tokens,
                            total: None,
                        },
                        nasiko_pricing::PromptConvention::Exclusive,
                        call.started_at,
                    )
                    .await
                    .cost,
            );
        }
        assert_eq!(
            cost,
            rust_decimal::Decimal::from_f64_retain(expected.total_usd)
                .unwrap()
                .round_dp(6)
        );
        assert_eq!(estimated, expected.estimated);
    }

    #[tokio::test]
    async fn absent_calls_are_unpriced_but_reported_zero_usage_is_zero_cost() {
        let pricing = nasiko_pricing::PricingEngine::offline();
        let mut event = event();
        for call in &mut event.turn.llm_calls {
            call.input_tokens = 0;
            call.output_tokens = 0;
            call.cache_read_tokens = 0;
            call.cache_creation_tokens = 0;
        }
        assert_eq!(
            price_turn(&pricing, &event).await.unwrap().0,
            rust_decimal::Decimal::ZERO
        );
        event.turn.llm_calls.clear();
        assert_eq!(price_turn(&pricing, &event).await, None);
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

    #[test]
    fn oversized_transcript_counts_saturate_instead_of_wrapping() {
        let mut event = event();
        for call in &mut event.turn.llm_calls {
            call.input_tokens = u64::MAX;
            call.cache_read_tokens = u64::MAX;
        }
        let usage = external_turn(&event, "scoped-session")
            .assistant_usage
            .unwrap();
        assert_eq!(usage.input_tokens, Some(i32::MAX));
        assert_eq!(usage.cache_read_tokens, Some(i32::MAX));
    }
}
