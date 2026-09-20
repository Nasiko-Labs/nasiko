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
           VALUES ($1, $2, $3, 'Coding session', $4, $5)
           ON CONFLICT (session_id) DO NOTHING"#,
    )
    .bind(&server_session_id)
    .bind(user_id)
    .bind(agent_id)
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

    if event.capture_policy == CapturePolicy::Content {
        let turn = external_turn(event, &server_session_id);
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

fn external_turn(event: &CodingAgentEventV1, session_id: &str) -> ExternalTurn {
    let mut scoped_event = event.clone();
    scoped_event.session.id = session_id.to_string();
    let calls = &event.turn.llm_calls;
    let input_tokens = calls.iter().fold(0_u64, |total, call| {
        total
            .saturating_add(call.input_tokens)
            .saturating_add(call.cache_read_tokens)
            .saturating_add(call.cache_creation_tokens)
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
            input_tokens: Some(input_tokens.min(i32::MAX as u64) as i32),
            output_tokens: Some(output_tokens.min(i32::MAX as u64) as i32),
            model,
            duration_ms: Some(duration_ms.min(i32::MAX as i64) as i32),
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
