use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use nasiko_pricing::{LedgerEntry, NoUsageLedger, UsageLedger};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::models::{CreateRouterRequestLog, CreateTokenUsage};

/// What a tool call is recorded as on the usage ledger.
const TOOL_CALL_OPERATION: &str = "mcp_tool_call";

/// Service for tracking spend (`trace_usage`), tool calls (`tool_calls`) and orchestrator requests.
///
/// Every spend and tool-call write also hands the call to its [`UsageLedger`], on the same
/// transaction. The LLM router and the MAF worker take the same ledger from here.
#[derive(Clone)]
pub struct UsageTracker {
    db: PgPool,
    ledger: Arc<SwappableUsageLedger>,
}

/// The tracker's ledger, installed through this cell rather than by replacing the tracker, for
/// the same reason `prompt_context` is: the HITL resume dispatcher holds an `AppState` clone taken
/// before the EE composition root runs, and a clone of the tracker shares this one cell.
struct SwappableUsageLedger(RwLock<Arc<dyn UsageLedger>>);

impl SwappableUsageLedger {
    fn current(&self) -> Arc<dyn UsageLedger> {
        self.0.read().expect("usage ledger lock poisoned").clone()
    }
}

#[async_trait]
impl UsageLedger for SwappableUsageLedger {
    async fn record(
        &self,
        conn: &mut PgConnection,
        entry: &LedgerEntry<'_>,
    ) -> Result<(), sqlx::Error> {
        self.current().record(conn, entry).await
    }
}

/// Primary key of the `trace_usage` row a call was added to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceUsageKey {
    pub trace_id: String,
    pub agent_name: String,
}

/// One MCP tool call. Carries no tokens, so it lands in `tool_calls`, not `trace_usage`.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    /// Shared with the exported `execute_tool` span (`gen_ai.tool.call.id`) so the materializer
    /// and this row key the same call.
    pub call_id: String,
    pub user_id: Uuid,
    pub agent_id: Uuid,
    pub tool_name: String,
    pub success: bool,
    pub latency_ms: i32,
    pub metadata: serde_json::Value,
    /// Flow verified against `flows`/`flow_participants`, never a raw traceparent.
    pub verified_flow_id: Option<String>,
}

impl UsageTracker {
    pub fn new(db: PgPool) -> Self {
        Self {
            db,
            ledger: Arc::new(SwappableUsageLedger(RwLock::new(Arc::new(NoUsageLedger)))),
        }
    }

    /// Hands every later call to `ledger`, for every clone of this tracker, past and future.
    pub fn install_ledger(&self, ledger: Arc<dyn UsageLedger>) {
        *self.ledger.0.write().expect("usage ledger lock poisoned") = ledger;
    }

    /// The tracker's ledger cell, for writers outside it: it follows [`Self::install_ledger`].
    pub fn ledger(&self) -> Arc<dyn UsageLedger> {
        self.ledger.clone()
    }

    /// Record one MCP tool call in `tool_calls`, and hand it to the ledger on the same
    /// transaction. Trace id is `mcp:<call id>`: gateway calls carry no traceparent here, and a
    /// fresh id keeps each call its own row.
    pub async fn track_tool_call(&self, call: ToolCallRecord) -> Result<(), sqlx::Error> {
        let call_id: String = call.call_id.clone();
        let trace_id: String = match call.verified_flow_id.as_deref() {
            Some(flow) => flow.to_string(),
            None => format!("mcp:{call_id}"),
        };
        let status: &str = if call.success { "ok" } else { "error" };
        let mut tx = self.db.begin().await?;
        sqlx::query(
            r#"INSERT INTO tool_calls (
                   trace_id, call_id, user_id, agent_id, agent_name, tool_name, tool_kind,
                   status, arguments, duration_ms, started_at, session_id
               ) VALUES (
                   $1, $2, $3, $4,
                   COALESCE((SELECT name FROM agents WHERE id = $4), $4::text),
                   $5, 'mcp', $6, $7, $8, now() - make_interval(secs => $8::double precision / 1000),
                     (SELECT session_id FROM flows WHERE flow_id = $9)
               )
               ON CONFLICT (trace_id, call_id) DO NOTHING"#,
        )
        .bind(&trace_id)
        .bind(&call_id)
        .bind(call.user_id)
        .bind(call.agent_id)
        .bind(&call.tool_name)
        .bind(status)
        .bind(sqlx::types::Json(&call.metadata))
        .bind(i64::from(call.latency_ms))
        .bind(call.verified_flow_id.as_deref())
        .execute(&mut *tx)
        .await?;
        let entry = LedgerEntry {
            user_id: call.user_id,
            agent_id: Some(call.agent_id),
            operation_type: TOOL_CALL_OPERATION,
            provider: "mcp",
            model: &call.tool_name,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            cost_usd: None,
            finish_reason: Some(status),
            metadata: &call.metadata,
        };
        self.ledger.record(&mut tx, &entry).await?;
        tx.commit().await
    }

    /// Add a completion priced by the caller's shared engine to its `trace_usage` row, and hand
    /// it to the ledger on the same transaction.
    ///
    /// Keyed `(session_id, agent name)` like the LLM router's live writer, `direct:<uuid>` when
    /// there is no flow. Tokens and cost accumulate; metadata merges with `||`.
    pub async fn track_priced_tokens(
        &self,
        usage: CreateTokenUsage,
        cost_usd: f64,
    ) -> Result<TraceUsageKey, sqlx::Error> {
        let trace_id: String = match usage.session_id.as_deref() {
            Some(flow) if !flow.is_empty() => flow.to_string(),
            _ => format!("direct:{}", Uuid::new_v4()),
        };
        let agent_name: String = match usage.agent_id {
            Some(id) => sqlx::query_scalar("SELECT name FROM agents WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.db)
                .await?
                .unwrap_or_else(|| id.to_string()),
            None => usage.operation_type.clone(),
        };
        let mut metadata: serde_json::Value = usage.metadata;
        if let Some(map) = metadata.as_object_mut() {
            map.insert("operation_type".into(), usage.operation_type.clone().into());
        }
        let mut tx = self.db.begin().await?;
        sqlx::query(
            r#"INSERT INTO trace_usage (
                   trace_id, agent_name, session_id, agent_id, user_id, model, provider,
                   input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                   cost_usd, latency_ms, started_at, priced_by, metadata, materialized_at,
                   router_input_tokens, router_output_tokens, router_cache_read_tokens,
                   router_cache_creation_tokens, router_cost_usd
               ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,now(),'router',$14,now(),
                         $8,$9,$10,$11,$12)
               ON CONFLICT (trace_id, agent_name) DO UPDATE SET
                   -- Added to the visible columns and the router share alike (0080):
                   -- a Tempo-priced row keeps what Tempo counted.
                   input_tokens          = trace_usage.input_tokens + EXCLUDED.input_tokens,
                   output_tokens         = trace_usage.output_tokens + EXCLUDED.output_tokens,
                   cache_read_tokens     = trace_usage.cache_read_tokens + EXCLUDED.cache_read_tokens,
                   cache_creation_tokens = trace_usage.cache_creation_tokens + EXCLUDED.cache_creation_tokens,
                   cost_usd              = COALESCE(trace_usage.cost_usd, 0) + EXCLUDED.cost_usd,
                   router_input_tokens          = trace_usage.router_input_tokens + EXCLUDED.router_input_tokens,
                   router_output_tokens         = trace_usage.router_output_tokens + EXCLUDED.router_output_tokens,
                   router_cache_read_tokens     = trace_usage.router_cache_read_tokens + EXCLUDED.router_cache_read_tokens,
                   router_cache_creation_tokens = trace_usage.router_cache_creation_tokens + EXCLUDED.router_cache_creation_tokens,
                   router_cost_usd              = trace_usage.router_cost_usd + EXCLUDED.router_cost_usd,
                   model                 = EXCLUDED.model,
                   provider              = EXCLUDED.provider,
                   agent_id              = COALESCE(trace_usage.agent_id, EXCLUDED.agent_id),
                   user_id               = COALESCE(trace_usage.user_id, EXCLUDED.user_id),
                   latency_ms            = COALESCE(trace_usage.latency_ms, 0) + COALESCE(EXCLUDED.latency_ms, 0),
                   metadata              = trace_usage.metadata || EXCLUDED.metadata,
                   priced_by             = 'router',
                   materialized_at       = now()"#,
        )
        .bind(&trace_id)
        .bind(&agent_name)
        .bind(usage.session_id.as_deref())
        .bind(usage.agent_id)
        .bind(usage.user_id)
        .bind(&usage.model)
        .bind(&usage.provider)
        .bind(i64::from(usage.input_tokens))
        .bind(i64::from(usage.output_tokens))
        .bind(i64::from(usage.cache_read_input_tokens.unwrap_or(0)))
        .bind(i64::from(usage.cache_creation_input_tokens.unwrap_or(0)))
        .bind(cost_usd)
        .bind(usage.latency_ms.map(i64::from))
        .bind(sqlx::types::Json(&metadata))
        .execute(&mut *tx)
        .await?;
        let entry = LedgerEntry {
            user_id: usage.user_id,
            agent_id: usage.agent_id,
            operation_type: &usage.operation_type,
            provider: &usage.provider,
            model: &usage.model,
            input_tokens: i64::from(usage.input_tokens),
            output_tokens: i64::from(usage.output_tokens),
            total_tokens: i64::from(usage.total_tokens),
            cache_read_tokens: i64::from(usage.cache_read_input_tokens.unwrap_or(0)),
            cache_creation_tokens: i64::from(usage.cache_creation_input_tokens.unwrap_or(0)),
            cost_usd: Some(cost_usd),
            finish_reason: usage.finish_reason.as_deref(),
            metadata: &metadata,
        };
        self.ledger.record(&mut tx, &entry).await?;
        tx.commit().await?;
        Ok(TraceUsageKey {
            trace_id,
            agent_name,
        })
    }

    /// Track a orchestrator request
    pub async fn track_router_request(
        &self,
        log: CreateRouterRequestLog,
    ) -> Result<Uuid, sqlx::Error> {
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
}
