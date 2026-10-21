//! Durable recovery for delivered receipts older than the Tempo search cursor.
use nasiko_observability::{ObservabilityProvider, SessionIdResolver};
use nasiko_types::CodingAgentEventV1;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

/// How many delivered receipts one pass may fetch from Tempo.
const BATCH: i64 = 8;

/// Backoff after a failed attempt. A trace Tempo has not finished indexing is the
/// common case, so this is a short retry rather than an escalating one.
const RETRY_AFTER: &str = "30 seconds";

/// Materialize a bounded batch, acknowledging only verified, successfully written usage.
pub async fn materialize_once(
    db: &PgPool,
    provider: &dyn ObservabilityProvider,
    sessions: &dyn SessionIdResolver,
) -> anyhow::Result<usize> {
    let pending: Vec<(Uuid, String, String, Value)> = sqlx::query_as(
        "SELECT user_id, event_id, session_id, payload
           FROM coding_agent_telemetry_events
          WHERE otlp_trace_delivered_at IS NOT NULL
            AND materialized_at IS NULL
            AND materialize_next_attempt_at <= now()
          ORDER BY materialize_next_attempt_at
          LIMIT $1",
    )
    .bind(BATCH)
    .fetch_all(db)
    .await?;

    let mut completed = 0;
    for (user, id, session, payload) in pending {
        let result = materialize_receipt(db, provider, sessions, session, payload).await;
        let error = result.as_ref().err().map(ToString::to_string);
        // A failure leaves `materialized_at` NULL, so the work stays claimable.
        sqlx::query(
            "UPDATE coding_agent_telemetry_events
                SET materialized_at = CASE WHEN $3::TEXT IS NULL THEN now() END,
                    materialize_next_attempt_at = now() + $4::INTERVAL,
                    materialize_last_error = $3
              WHERE user_id = $1 AND event_id = $2 AND materialized_at IS NULL",
        )
        .bind(user)
        .bind(id)
        .bind(error)
        .bind(RETRY_AFTER)
        .execute(db)
        .await?;
        completed += usize::from(result.is_ok());
    }
    Ok(completed)
}

async fn materialize_receipt(
    db: &PgPool,
    provider: &dyn ObservabilityProvider,
    sessions: &dyn SessionIdResolver,
    session: String,
    payload: Value,
) -> anyhow::Result<()> {
    let mut event: CodingAgentEventV1 = serde_json::from_value(payload)?;
    event.session.id = session;
    let id = crate::coding_agent_otlp::trace_id_for_event(&event);
    let trace = provider.get_trace(&id).await?;
    let usage = trace.usage_totals().0;
    let sum = |get: fn(&nasiko_types::CodingAgentLlmCall) -> u64| {
        event.turn.llm_calls.iter().map(get).sum::<u64>()
    };
    anyhow::ensure!(
        [
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_tokens,
            usage.cache_creation_tokens
        ] == [
            sum(|c| c.input_tokens),
            sum(|c| c.output_tokens),
            sum(|c| c.cache_read_tokens),
            sum(|c| c.cache_creation_tokens)
        ],
        "receipt trace usage is incomplete"
    );
    let rows = provider.extract_trace_usage(&id).await?;
    anyhow::ensure!(
        !rows.is_empty()
            || event.turn.llm_calls.iter().all(|c| c.input_tokens == 0
                && c.output_tokens == 0
                && c.cache_read_tokens == 0
                && c.cache_creation_tokens == 0),
        "token-bearing receipt has no materialization rows"
    );
    for row in rows {
        super::trace_materializer::upsert_row(db, sessions, &row)
            .await
            .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}
