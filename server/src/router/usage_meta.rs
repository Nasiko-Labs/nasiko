//! Terminal per-message usage summary for A2A streams.
//!
//! Policy: only **platform-paid** LLM spend is metered — the orchestrator's own
//! turns and agent calls routed through the LLM gateway with the platform key.
//! Bring-your-own-key agent spend is the agent developer's concern; those
//! messages get a duration-only summary. The summary is emitted as the
//! `usage_meta` data part right before the stream's terminal status event, and
//! persisted on the assistant's `chat_messages` row so it survives reload.

use nasiko_observability::ObservabilityProvider;
use sqlx::PgPool;

/// Orchestrator-turn usage accumulated while the stream runs.
#[derive(Default)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model: Option<String>,
    /// True when any contributing turn reported a character-estimate
    /// rather than exact provider counts (streamed turns under rig 0.11).
    pub estimated: bool,
}

impl TurnUsage {
    pub fn add(&mut self, input_tokens: u64, output_tokens: u64, model: &str, estimated: bool) {
        self.input_tokens += input_tokens;
        self.output_tokens += output_tokens;
        self.model.get_or_insert_with(|| model.to_string());
        self.estimated |= estimated;
    }
}

/// The complete per-message summary the `usage_meta` event carries.
pub struct UsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub model: Option<String>,
    pub estimated: bool,
    pub duration_ms: i64,
}

impl UsageSummary {
    pub fn has_tokens(&self) -> bool {
        self.input_tokens + self.output_tokens > 0
    }

    /// The `usage_meta` data-part payload. Token/cost fields are omitted for
    /// duration-only summaries (nothing platform-paid was metered) so clients
    /// show latency without implying the call was free.
    pub fn to_data_part(&self, trace_id: &str) -> serde_json::Value {
        let mut part = serde_json::json!({
            "type": "usage_meta",
            "duration_ms": self.duration_ms,
            "trace_id": trace_id,
        });
        if self.has_tokens() {
            part["input_tokens"] = self.input_tokens.into();
            part["output_tokens"] = self.output_tokens.into();
            part["total_tokens"] = (self.input_tokens + self.output_tokens).into();
            part["cost_usd"] = self.cost_usd.into();
            part["estimated"] = self.estimated.into();
            if let Some(model) = &self.model {
                part["model"] = model.as_str().into();
            }
        }
        part
    }
}

/// Build the summary for one finished stream: the orchestrator's accumulated
/// turns (priced through the observability pricing source) plus the
/// platform-paid `token_usage` rows agents wrote inside this flow.
pub async fn summarize_flow_usage(
    db: &PgPool,
    observability: &dyn ObservabilityProvider,
    flow_id: &str,
    turns: &TurnUsage,
    duration_ms: i64,
) -> UsageSummary {
    let (agent_in, agent_out, agent_cost) = platform_paid_agent_usage(db, flow_id).await;

    let turn_cost = if turns.input_tokens + turns.output_tokens > 0 {
        observability
            .cost(
                turns.model.as_deref(),
                turns.input_tokens,
                turns.output_tokens,
            )
            .await
            .total_usd
    } else {
        0.0
    };

    UsageSummary {
        input_tokens: turns.input_tokens + agent_in,
        output_tokens: turns.output_tokens + agent_out,
        cost_usd: turn_cost + agent_cost,
        model: turns.model.clone(),
        estimated: turns.estimated,
        duration_ms,
    }
}

/// Sum the platform-paid rows the LLM gateway wrote for agents in this flow.
/// Orchestrator rows are excluded (`operation_type = 'direct_llm'` only) —
/// the caller already holds those exactly, in [`TurnUsage`].
async fn platform_paid_agent_usage(db: &PgPool, flow_id: &str) -> (u64, u64, f64) {
    let sums: Result<(i64, i64, f64), sqlx::Error> = sqlx::query_as(
        r#"SELECT COALESCE(SUM(input_tokens), 0)::BIGINT,
                  COALESCE(SUM(output_tokens), 0)::BIGINT,
                  COALESCE(SUM(cost_usd), 0)::FLOAT8
           FROM token_usage
           WHERE session_id = $1
             AND operation_type = 'direct_llm'
             AND metadata->>'key_source' = 'platform'"#,
    )
    .bind(flow_id)
    .fetch_one(db)
    .await;

    match sums {
        Ok((input, output, cost)) => (input.max(0) as u64, output.max(0) as u64, cost),
        Err(e) => {
            tracing::warn!(error = %e, %flow_id, "flow usage aggregation failed; usage_meta omits agent rows");
            (0, 0, 0.0)
        }
    }
}

/// Persist the assistant reply with its usage columns so chips and the trace
/// link survive a history reload.
pub async fn insert_assistant_message(
    db: &PgPool,
    session_id: &str,
    content: &str,
    summary: &UsageSummary,
    trace_id: &str,
) {
    let (input, output, cost, estimated) = if summary.has_tokens() {
        (
            Some(summary.input_tokens as i32),
            Some(summary.output_tokens as i32),
            Some(summary.cost_usd),
            Some(summary.estimated),
        )
    } else {
        (None, None, None, None)
    };
    let result = sqlx::query(
        r#"INSERT INTO chat_messages
               (session_id, role, content, input_tokens, output_tokens, model,
                duration_ms, cost_usd, usage_estimated, trace_id)
           VALUES ($1, 'assistant', $2, $3, $4, $5, $6, $7, $8, $9)"#,
    )
    .bind(session_id)
    .bind(content)
    .bind(input)
    .bind(output)
    .bind(summary.model.as_deref().filter(|_| input.is_some()))
    .bind(summary.duration_ms as i32)
    .bind(cost)
    .bind(estimated)
    .bind(trace_id)
    .execute(db)
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "failed to persist assistant chat message");
    }
}
