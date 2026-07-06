use sqlx::PgPool;
use uuid::Uuid;

use super::models::{CreateRouterRequestLog, CreateTokenUsage, TokenUsage};

/// Service for tracking token usage and orchestrator requests
#[derive(Clone)]
pub struct UsageTracker {
    db: PgPool,
}

impl UsageTracker {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Track token usage for an LLM call
    /// Cost is auto-calculated by database trigger using model_pricing table
    pub async fn track_tokens(&self, usage: CreateTokenUsage) -> Result<Uuid, sqlx::Error> {
        let id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO token_usage (
                user_id, agent_id, operation_type, request_id, session_id,
                provider, model,
                input_tokens, output_tokens, total_tokens,
                cache_creation_input_tokens, cache_read_input_tokens,
                cached_tokens, audio_tokens, reasoning_tokens,
                accepted_prediction_tokens, rejected_prediction_tokens,
                completion_tokens_details, prompt_tokens_details,
                latency_ms, ttft_ms, streaming, finish_reason, metadata
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24)
            RETURNING id
            "#,
        )
        .bind(usage.user_id)
        .bind(usage.agent_id)
        .bind(usage.operation_type)
        .bind(usage.request_id)
        .bind(usage.session_id)
        .bind(usage.provider)
        .bind(usage.model)
        .bind(usage.input_tokens)
        .bind(usage.output_tokens)
        .bind(usage.total_tokens)
        .bind(usage.cache_creation_input_tokens)
        .bind(usage.cache_read_input_tokens)
        .bind(usage.cached_tokens)
        .bind(usage.audio_tokens)
        .bind(usage.reasoning_tokens)
        .bind(usage.accepted_prediction_tokens)
        .bind(usage.rejected_prediction_tokens)
        .bind(usage.completion_tokens_details.map(sqlx::types::Json))
        .bind(usage.prompt_tokens_details.map(sqlx::types::Json))
        .bind(usage.latency_ms)
        .bind(usage.ttft_ms)
        .bind(usage.streaming)
        .bind(usage.finish_reason)
        .bind(sqlx::types::Json(usage.metadata))
        .fetch_one(&self.db)
        .await?;

        Ok(id)
    }

    /// Track a orchestrator request
    pub async fn track_router_request(&self, log: CreateRouterRequestLog) -> Result<Uuid, sqlx::Error> {
        let id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO router_request_log (
                request_id, user_id, session_id, query,
                agents_considered, selected_agent_id, selected_agent_name,
                selection_reasoning, fallback_used, selection_token_usage_id,
                total_latency_ms, registry_fetch_ms, vector_store_ms,
                selection_llm_ms, agent_call_ms,
                success, error_message, finish_reason,
                streaming, file_count, metadata
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21)
            RETURNING id
            "#,
        )
        .bind(log.request_id)
        .bind(log.user_id)
        .bind(log.session_id)
        .bind(log.query)
        .bind(log.agents_considered)
        .bind(log.selected_agent_id)
        .bind(log.selected_agent_name)
        .bind(log.selection_reasoning)
        .bind(log.fallback_used)
        .bind(log.selection_token_usage_id)
        .bind(log.total_latency_ms)
        .bind(log.registry_fetch_ms)
        .bind(log.vector_store_ms)
        .bind(log.selection_llm_ms)
        .bind(log.agent_call_ms)
        .bind(log.success)
        .bind(log.error_message)
        .bind(log.finish_reason)
        .bind(log.streaming)
        .bind(log.file_count)
        .bind(sqlx::types::Json(log.metadata))
        .fetch_one(&self.db)
        .await?;

        Ok(id)
    }

    /// Get token usage for a specific request
    pub async fn get_token_usage(&self, id: Uuid) -> Result<TokenUsage, sqlx::Error> {
        sqlx::query_as::<_, TokenUsage>(
            "SELECT * FROM token_usage WHERE id = $1"
        )
        .bind(id)
        .fetch_one(&self.db)
        .await
    }

    /// Get token usage summary for a user in a date range
    pub async fn get_user_usage_summary(
        &self,
        user_id: Uuid,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<UsageSummary, sqlx::Error> {
        let row = sqlx::query_as::<_, UsageSummaryRow>(
            r#"
            SELECT
                COUNT(*)::bigint as request_count,
                COALESCE(SUM(input_tokens), 0)::bigint as total_input,
                COALESCE(SUM(output_tokens), 0)::bigint as total_output,
                COALESCE(SUM(total_tokens), 0)::bigint as total_tokens,
                COALESCE(SUM(cost_usd), 0) as total_cost,
                AVG(latency_ms)::double precision as avg_latency
            FROM token_usage
            WHERE user_id = $1
              AND created_at >= $2
              AND created_at <= $3
            "#,
        )
        .bind(user_id)
        .bind(from)
        .bind(to)
        .fetch_one(&self.db)
        .await?;

        Ok(UsageSummary {
            request_count: row.request_count.unwrap_or(0),
            total_input_tokens: row.total_input.unwrap_or(0),
            total_output_tokens: row.total_output.unwrap_or(0),
            total_tokens: row.total_tokens.unwrap_or(0),
            total_cost: row.total_cost.unwrap_or_default(),
            avg_latency_ms: row.avg_latency,
        })
    }
}

#[derive(sqlx::FromRow)]
struct UsageSummaryRow {
    request_count: Option<i64>,
    total_input: Option<i64>,
    total_output: Option<i64>,
    total_tokens: Option<i64>,
    total_cost: Option<rust_decimal::Decimal>,
    avg_latency: Option<f64>,
}

#[derive(Debug, serde::Serialize)]
pub struct UsageSummary {
    pub request_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub total_cost: rust_decimal::Decimal,
    pub avg_latency_ms: Option<f64>,
}
