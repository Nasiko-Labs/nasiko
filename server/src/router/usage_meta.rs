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
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
    pub model: Option<String>,
    /// True when usage or any contributing price was estimated.
    pub estimated: bool,
}

impl TurnUsage {
    pub fn add(&mut self, call: &PricedTurn) {
        self.input_tokens = self.input_tokens.saturating_add(call.usage.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(call.usage.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(call.usage.cache_read_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(call.usage.cache_creation_tokens);
        self.cost_usd += call.cost_usd;
        self.model.get_or_insert_with(|| call.usage.model.clone());
        self.estimated |= call.estimated;
    }
}

/// A single priced call, shared by the live summary and delayed agent attribution.
pub struct PricedTurn {
    pub usage: nasiko_react_agent::CallUsage,
    pub cost_usd: f64,
    pub estimated: bool,
    provenance: serde_json::Value,
}

impl PricedTurn {
    pub async fn price(
        engine: &nasiko_pricing::PricingEngine,
        usage: nasiko_react_agent::CallUsage,
    ) -> Self {
        let priced = engine
            .price(
                usage.provider.as_deref(),
                &usage.model,
                nasiko_pricing::RawUsage {
                    input: usage.input_tokens,
                    output: usage.output_tokens,
                    cache_read: usage.cache_read_tokens,
                    cache_creation: usage.cache_creation_tokens,
                    total: Some(usage.total_tokens),
                },
                nasiko_pricing::PromptConvention::Exclusive,
                usage.started_at,
            )
            .await;
        Self {
            cost_usd: priced.cost.total_usd,
            estimated: usage.estimated || priced.cost.estimated,
            provenance: priced.provenance(),
            usage,
        }
    }

    pub async fn persist(
        self,
        tracker: &crate::usage::UsageTracker,
        user_id: uuid::Uuid,
        flow_id: &str,
        agent_id: Option<uuid::Uuid>,
    ) -> Result<uuid::Uuid, sqlx::Error> {
        let count = |value: u64| value.min(i32::MAX as u64) as i32;
        // Set the full total directly: the general-purpose builder also serves
        // older callers whose input still includes cache and must not change here.
        let mut row = crate::usage::TokenUsageBuilder::new(user_id, "orchestrator", self.usage.provider.as_deref().unwrap_or("unknown"), &self.usage.model)
            .tokens(0, 0)
            .cache_read_tokens(count(self.usage.cache_read_tokens))
            .cache_creation_tokens(count(self.usage.cache_creation_tokens))
            .cached_tokens(count(self.usage.cache_read_tokens))
            .session_id(flow_id)
            .streaming(self.usage.streaming)
            .metadata(serde_json::json!({"key_source": "platform", "estimated": self.estimated,
                "usage_estimated": self.usage.estimated, "started_at": self.usage.started_at, "pricing": self.provenance}))
            .build();
        row.input_tokens = count(self.usage.input_tokens);
        row.output_tokens = count(self.usage.output_tokens);
        row.total_tokens = count(self.usage.total_tokens);
        row.agent_id = agent_id;
        tracker.track_priced_tokens(row, self.cost_usd).await
    }
}

/// The complete per-message summary the `usage_meta` event carries.
pub struct UsageSummary {
    /// Fresh prompt tokens — billed at the full input rate, cached tokens excluded.
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Prompt tokens served from the provider's cache, billed at the cache rate.
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
    pub model: Option<String>,
    pub estimated: bool,
    pub duration_ms: i64,
}

impl UsageSummary {
    pub fn has_tokens(&self) -> bool {
        self.total_prompt_tokens() + self.output_tokens > 0
    }

    /// Every prompt token, cached or not. What the caller actually sent.
    pub fn total_prompt_tokens(&self) -> u64 {
        self.input_tokens + self.cache_read_tokens + self.cache_creation_tokens
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
            part["cache_read_tokens"] = self.cache_read_tokens.into();
            part["cache_creation_tokens"] = self.cache_creation_tokens.into();
            // The whole prompt, cached included. Summing only the fresh part would make an
            // identical turn look smaller purely because the cache served more of it.
            part["total_tokens"] = (self.total_prompt_tokens() + self.output_tokens).into();
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
    _observability: &dyn ObservabilityProvider,
    flow_id: &str,
    turns: &TurnUsage,
    duration_ms: i64,
) -> UsageSummary {
    let agents = platform_paid_agent_usage(db, flow_id).await;

    UsageSummary {
        input_tokens: turns.input_tokens + agents.input,
        output_tokens: turns.output_tokens + agents.output,
        cache_read_tokens: turns.cache_read_tokens + agents.cache_read,
        cache_creation_tokens: turns.cache_creation_tokens + agents.cache_creation,
        cost_usd: turns.cost_usd + agents.cost,
        model: turns.model.clone(),
        estimated: turns.estimated,
        duration_ms,
    }
}

/// Sum the platform-paid rows the LLM gateway wrote for agents in this flow.
/// Orchestrator rows are excluded (`operation_type = 'direct_llm'` only) —
/// the caller already holds those exactly, in [`TurnUsage`].
#[derive(Default)]
struct AgentUsage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
    cost: f64,
}

async fn platform_paid_agent_usage(db: &PgPool, flow_id: &str) -> AgentUsage {
    let sums: Result<(i64, i64, i64, i64, f64), sqlx::Error> = sqlx::query_as(
        r#"SELECT COALESCE(SUM(input_tokens), 0)::BIGINT,
                  COALESCE(SUM(output_tokens), 0)::BIGINT,
                  COALESCE(SUM(cache_read_input_tokens), 0)::BIGINT,
                  COALESCE(SUM(cache_creation_input_tokens), 0)::BIGINT,
                  COALESCE(SUM(cost_usd), 0)::FLOAT8
           FROM token_usage
           WHERE session_id = $1
             AND operation_type = 'direct_llm'"#,
    )
    .bind(flow_id)
    .fetch_one(db)
    .await;

    match sums {
        Ok((input, output, cache_read, cache_creation, cost)) => AgentUsage {
            input: input.max(0) as u64,
            output: output.max(0) as u64,
            cache_read: cache_read.max(0) as u64,
            cache_creation: cache_creation.max(0) as u64,
            cost,
        },
        Err(e) => {
            tracing::warn!(error = %e, %flow_id, "flow usage aggregation failed; usage_meta omits agent rows");
            AgentUsage::default()
        }
    }
}

/// Persist the assistant reply with its usage columns so chips and the trace
/// link survive a history reload.
/// `is_refusal` tags the row so `SessionHistory::fetch` leaves it out of the next
/// turn's reasoning context. The human still sees it in the transcript; the model
/// does not see it as prior assistant output. Without this a single refusal makes
/// the whole session refuse: the model reads its own refusal back and treats
/// declining as the established behaviour of this conversation. Only a
/// `DelegationPolicy` ever produces one, and only it can recognise one — this
/// function is told, it does not decide.
pub async fn insert_assistant_message(
    db: &PgPool,
    session_id: &str,
    content: &str,
    summary: &UsageSummary,
    trace_id: &str,
    is_refusal: bool,
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
    let (cache_read, cache_creation) = if summary.has_tokens() {
        (
            Some(summary.cache_read_tokens as i32),
            Some(summary.cache_creation_tokens as i32),
        )
    } else {
        (None, None)
    };
    let result = sqlx::query(
        r#"INSERT INTO chat_messages
               (session_id, role, content, input_tokens, output_tokens, model,
                duration_ms, cost_usd, usage_estimated, trace_id, metadata,
                cache_read_tokens, cache_creation_tokens)
           VALUES ($1, 'assistant', $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"#,
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
    // Built from the reader's own constant rather than spelled again here: the
    // filter that consumes this tag lives in another crate, and a mismatch
    // between the two spellings fails silently.
    .bind(is_refusal.then(|| {
        let mut m = serde_json::Map::new();
        m.insert(
            nasiko_orchestrator::session_history::REFUSAL_METADATA_KEY.to_string(),
            serde_json::Value::Bool(true),
        );
        serde_json::Value::Object(m)
    }))
    .bind(cache_read)
    .bind(cache_creation)
    .execute(db)
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "failed to persist assistant chat message");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(input: u64, cache_read: u64, cache_creation: u64, output: u64) -> UsageSummary {
        UsageSummary {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_creation,
            cost_usd: 0.001,
            model: Some("gpt-4o-mini".into()),
            estimated: false,
            duration_ms: 1234,
        }
    }

    /// The regression: two identical turns must report the same size however much of the
    /// prompt the cache served. Summing only the fresh part made the second read smaller.
    #[test]
    fn an_identical_turn_reports_the_same_size_however_it_was_cached() {
        let cold = summary(3600, 3328, 0, 322);
        let warm = summary(762, 6144, 0, 322);

        assert_eq!(cold.total_prompt_tokens(), 6928);
        assert_eq!(warm.total_prompt_tokens(), 6906);

        let cold_total = cold.to_data_part("t")["total_tokens"].as_u64().unwrap();
        let warm_total = warm.to_data_part("t")["total_tokens"].as_u64().unwrap();
        let drift = cold_total.abs_diff(warm_total);
        assert!(
            drift < 50,
            "the same turn reported {cold_total} then {warm_total} — the chip still \
             collapses as the cache warms"
        );
    }

    #[test]
    fn the_data_part_carries_the_cached_split() {
        let part = summary(1039, 2816, 0, 1945).to_data_part("trace-1");
        assert_eq!(part["input_tokens"], 1039);
        assert_eq!(part["cache_read_tokens"], 2816);
        assert_eq!(part["output_tokens"], 1945);
        // 1039 + 2816 + 1945 — the whole prompt plus the reply.
        assert_eq!(part["total_tokens"], 5800);
    }

    #[test]
    fn a_turn_served_entirely_from_cache_still_counts_as_metered() {
        let s = summary(0, 4096, 0, 12);
        assert!(
            s.has_tokens(),
            "an all-cache turn reported no tokens, so it would render as duration-only"
        );
        assert_eq!(s.to_data_part("t")["total_tokens"], 4108);
    }

    #[test]
    fn a_duration_only_summary_omits_token_fields() {
        let s = summary(0, 0, 0, 0);
        let part = s.to_data_part("t");
        assert!(!s.has_tokens());
        assert!(part.get("total_tokens").is_none());
        assert!(part.get("cache_read_tokens").is_none());
        assert_eq!(part["duration_ms"], 1234);
    }
}
