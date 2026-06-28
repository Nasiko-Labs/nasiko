//! Fire-and-forget usage logging to the `token_usage` table.
//!
//! Writes one row per LLM call; the DB cost trigger fills `cost_usd` from
//! `model_pricing` (we leave it NULL). Failures are logged and swallowed — usage
//! logging must never break or delay the response.

use sqlx::PgPool;
use uuid::Uuid;

use crate::ir::Usage;

/// One usage row to write.
pub struct UsageRecord {
    pub owner_id: String,
    pub agent_id: String,
    /// `token_usage.operation_type`, e.g. `"direct_llm"` (chat) or `"embedding"`.
    pub operation_type: &'static str,
    pub provider: String,
    /// Bare provider-native model id (no prefix).
    pub model: String,
    pub usage: Option<Usage>,
    pub latency_ms: i64,
    pub streaming: bool,
    pub finish_reason: Option<String>,
}

/// Spawn the usage write so it never blocks the response.
pub fn spawn_log(db: PgPool, record: UsageRecord) {
    tokio::spawn(async move {
        if let Err(e) = log_usage(db, record).await {
            tracing::warn!(error = %e, "llm_usage write failed (swallowed)");
        }
    });
}

/// Insert one `token_usage` row. `cost_usd` is left NULL so the DB trigger computes it.
pub async fn log_usage(db: PgPool, record: UsageRecord) -> Result<(), String> {
    // token_usage.user_id is NOT NULL + FK to users(id); without a valid owner we
    // cannot write a row, so skip (best-effort logging must never surface an error).
    let Ok(owner) = Uuid::parse_str(&record.owner_id) else {
        tracing::debug!(owner_id = %record.owner_id, "skipping usage row: owner_id is not a uuid");
        return Ok(());
    };
    let agent = Uuid::parse_str(&record.agent_id).ok();
    let (input, output, total) = match record.usage {
        Some(u) => (u.prompt_tokens, u.completion_tokens, u.total_tokens),
        None => (None, None, None),
    };

    sqlx::query(
        r#"INSERT INTO token_usage
               (user_id, agent_id, operation_type, provider, model,
                input_tokens, output_tokens, total_tokens,
                latency_ms, streaming, finish_reason)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
    )
    .bind(owner)
    .bind(agent)
    .bind(record.operation_type)
    .bind(&record.provider)
    .bind(&record.model)
    .bind(input.unwrap_or(0) as i32)
    .bind(output.unwrap_or(0) as i32)
    .bind(total.unwrap_or(0) as i32)
    .bind(record.latency_ms as i32)
    .bind(record.streaming)
    .bind(record.finish_reason)
    .execute(&db)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}
