use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
};
use nasiko_types::{
    CapturePolicy, CodingAgentEventBatchRequest, CodingAgentEventBatchResponse,
    CodingAgentEventResult, CodingAgentEventStatus, CodingAgentEventV1, coding_agent_session_id,
};
use serde_json::json;
use uuid::Uuid;

use crate::auth::Claims;
use crate::chat::external_turn::{PersistExternalTurnError, persist_external_turn};
use crate::chat::models::{ExternalTurn, MessageUsage};
use crate::mcp::ApiResponse;
use crate::state::AppState;

const CODING_SESSION_PLACEHOLDER: &str = "Coding session";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/telemetry/coding-agent/events/batch", post(ingest_batch))
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
}

async fn ingest_batch(
    State(state): State<AppState>,
    claims: Claims,
    Json(batch): Json<CodingAgentEventBatchRequest>,
) -> axum::response::Response {
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
    let mut results = Vec::with_capacity(batch.events.len());
    for event in batch.events {
        let event_id = event.event_id.clone();
        match process_event(&state, user_id, &event).await {
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
    ApiResponse::ok(
        serde_json::to_value(CodingAgentEventBatchResponse { results })
            .expect("batch response serializes"),
        "coding-agent telemetry processed",
    )
    .into_response()
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

async fn process_event(
    state: &AppState,
    user_id: Uuid,
    event: &CodingAgentEventV1,
) -> Result<CodingAgentEventStatus, ProcessError> {
    event.validate().map_err(ProcessError::Rejected)?;
    let expected_session_id =
        coding_agent_session_id(&event.source.agent_id, &event.session.source_id);
    if event.session.id != expected_session_id {
        return Err(ProcessError::Rejected(
            "session.id does not match the namespaced source session identity".into(),
        ));
    }

    // Pricing acquires its own database connection. Do not hold the write
    // transaction while looking up rates: a saturated pool would deadlock.
    let cost = if event.capture_policy == CapturePolicy::Content {
        price_turn(&state.pricing, event).await
    } else {
        None
    };
    let mut tx = state.db.begin().await?;
    let agent_id: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT id FROM agents
           WHERE owner_id = $1 AND name = $2
             AND coding_agent_integration_id = $3
             AND deleted_at IS NULL"#,
    )
    .bind(user_id)
    .bind(&event.source.agent_name)
    .bind(&event.source.agent_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(agent_id) = agent_id else {
        return Err(ProcessError::Rejected(format!(
            "active owned {} coding-agent integration '{}' not found",
            event.source.agent_id, event.source.agent_name
        )));
    };
    let server_session_id = scoped_session_id(agent_id, &event.session.source_id);

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

    let payload =
        serde_json::to_value(event).map_err(|error| ProcessError::Internal(error.into()))?;
    let inserted = sqlx::query(
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
    .bind(&server_session_id)
    .bind(&event.session.source_id)
    .bind(&event.turn.id)
    .bind(event.captured_at)
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
        if stored != payload {
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
    tx.commit().await?;
    Ok(CodingAgentEventStatus::Accepted)
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
