//! A per-call record of every priced call, for a distribution that needs one.
//!
//! Spend lands in `trace_usage`, one row per `(trace, agent)` that every call of the trace is
//! added to. That is the right shape for dashboards and budgets, but it cannot say what one call
//! cost or whose key paid for it. A distribution that accounts for calls one at a time receives each
//! call here instead, on the same transaction as its `trace_usage` write, so the two never
//! disagree. The open-source ledger is [`NoUsageLedger`], which keeps nothing.

use async_trait::async_trait;
use sqlx::PgConnection;
use uuid::Uuid;

/// One priced call, as its writer saw it.
#[derive(Debug, Clone, Copy)]
pub struct LedgerEntry<'a> {
    /// Who the call is attributed to.
    pub user_id: Uuid,
    pub agent_id: Option<Uuid>,
    /// What kind of call this was, e.g. `"direct_llm"`, `"orchestrator"`,
    /// `"maf_orchestration"` or `"mcp_tool_call"`.
    pub operation_type: &'a str,
    pub provider: &'a str,
    /// The model, or for a tool call the tool's name.
    pub model: &'a str,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    /// What the platform's cost engine priced the call at; `None` for a call it did not price.
    pub cost_usd: Option<f64>,
    pub finish_reason: Option<&'a str>,
    /// The call's own metadata, including `key_source` (whose key paid).
    pub metadata: &'a serde_json::Value,
}

#[async_trait]
pub trait UsageLedger: Send + Sync {
    /// Records one call on `conn`, inside the writer's transaction. An error rolls the
    /// writer's `trace_usage` write back with it.
    async fn record(
        &self,
        conn: &mut PgConnection,
        entry: &LedgerEntry<'_>,
    ) -> Result<(), sqlx::Error>;
}

/// The open-source ledger: keeps nothing.
pub struct NoUsageLedger;

#[async_trait]
impl UsageLedger for NoUsageLedger {
    async fn record(
        &self,
        _conn: &mut PgConnection,
        _entry: &LedgerEntry<'_>,
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }
}
