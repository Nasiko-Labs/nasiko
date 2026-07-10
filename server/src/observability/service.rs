use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use nasiko_config::Config;
use nasiko_observability::{LokiClient, ObservabilityError, TempoClient};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

// ─── Model pricing (USD per million tokens) ───────────────────────────────────

/// Fallback pricing when the model is not found in the `model_pricing` DB table.
/// The DB trigger (`calculate_token_cost`) handles cost for rows in `token_usage`;
/// this function is only used for Tempo-sourced span cost estimation.
fn model_pricing(model: &str) -> (f64, f64) {
    match model {
        "gpt-4o" | "gpt-4o-2024-08-06" | "gpt-4o-2024-11-20" => (2.50, 10.00),
        "gpt-4o-mini" | "gpt-4o-mini-2024-07-18" => (0.15, 0.60),
        "gpt-4-turbo" => (10.00, 30.00),
        "gpt-3.5-turbo" => (0.50, 1.50),
        "deepseek-v4-flash" | "deepseek-chat" => (0.14, 0.28),
        "deepseek-reasoner" => (0.55, 2.19),
        "claude-sonnet-4-20250514" | "claude-4-sonnet" => (3.00, 15.00),
        "claude-opus-4-20250514" | "claude-4-opus" => (15.00, 75.00),
        "claude-3-5-haiku-20241022" => (0.80, 4.00),
        _ => (2.50, 10.00),
    }
}

/// Returns (prompt_cost, completion_cost, total_cost) in USD.
fn compute_cost(input: u64, output: u64, model: Option<&str>) -> (f64, f64, f64) {
    let (in_p, out_p) = model_pricing(model.unwrap_or("gpt-4o"));
    let prompt = round6(input as f64 / 1_000_000.0 * in_p);
    let completion = round6(output as f64 / 1_000_000.0 * out_p);
    (prompt, completion, round6(prompt + completion))
}

/// Query the `model_pricing` DB table for the current price of a model.
/// Returns (input_price_per_1m, output_price_per_1m) or None if not found.
#[allow(dead_code)]
async fn lookup_model_pricing(db: &PgPool, model: &str) -> Option<(f64, f64)> {
    #[derive(sqlx::FromRow)]
    struct PricingRow {
        input_price_per_1m: rust_decimal::Decimal,
        output_price_per_1m: rust_decimal::Decimal,
    }

    let row = sqlx::query_as::<_, PricingRow>(
        r#"SELECT input_price_per_1m, output_price_per_1m
           FROM model_pricing
           WHERE model = $1
             AND (effective_until IS NULL OR effective_until > now())
           ORDER BY effective_from DESC
           LIMIT 1"#,
    )
    .bind(model)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()?;

    use rust_decimal::prelude::ToPrimitive;
    Some((
        row.input_price_per_1m.to_f64().unwrap_or(2.50),
        row.output_price_per_1m.to_f64().unwrap_or(10.00),
    ))
}

/// Compute cost using DB pricing when available, falling back to hardcoded pricing.
#[allow(dead_code)]
async fn compute_cost_with_db(
    db: &PgPool,
    input: u64,
    output: u64,
    model: Option<&str>,
) -> (f64, f64, f64) {
    let model_name = model.unwrap_or("gpt-4o");
    let (in_p, out_p) = match lookup_model_pricing(db, model_name).await {
        Some(prices) => prices,
        None => model_pricing(model_name),
    };
    let prompt = round6(input as f64 / 1_000_000.0 * in_p);
    let completion = round6(output as f64 / 1_000_000.0 * out_p);
    (prompt, completion, round6(prompt + completion))
}

fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

/// Convert a flat map of dot-separated keys into a nested JSON object.
/// e.g. `{"openinference.span.kind": "LLM"}` → `{"openinference": {"span": {"kind": "LLM"}}}`
fn unflatten_attrs(attrs: &HashMap<String, Value>) -> Value {
    let mut root: serde_json::Map<String, Value> = serde_json::Map::new();
    for (key, value) in attrs {
        let parts: Vec<&str> = key.split('.').collect();
        insert_nested(&mut root, &parts, value.clone());
    }
    Value::Object(root)
}

fn insert_nested(map: &mut serde_json::Map<String, Value>, parts: &[&str], value: Value) {
    if parts.is_empty() {
        return;
    }
    if parts.len() == 1 {
        map.insert(parts[0].to_string(), value);
        return;
    }
    let entry = map
        .entry(parts[0].to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Value::Object(child) = entry {
        insert_nested(child, &parts[1..], value);
    }
    // If there's a type conflict (e.g. existing value is a scalar), skip.
}

fn span_kind_str(kind: u8) -> &'static str {
    match kind {
        1 => "INTERNAL",
        2 => "SERVER",
        3 => "CLIENT",
        4 => "PRODUCER",
        5 => "CONSUMER",
        _ => "UNSPECIFIED",
    }
}

fn status_code_str(code: u8) -> &'static str {
    match code {
        1 => "OK",
        2 => "ERROR",
        _ => "UNSET",
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn parse_iso_or_default(iso: Option<&str>, default_days_ago: i64) -> DateTime<Utc> {
    iso.and_then(|s| {
        DateTime::parse_from_rfc3339(&s.replace('Z', "+00:00"))
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    })
    .unwrap_or_else(|| Utc::now() - Duration::days(default_days_ago))
}

/// Clamp start to at most 168 h before end (Tempo's max range).
fn clamp_tempo_range(start: DateTime<Utc>, end: DateTime<Utc>) -> DateTime<Utc> {
    let max_start = end - Duration::hours(168);
    if start < max_start { max_start } else { start }
}

/// TraceQL query covering all three locations where agent_id may be stored.
fn agent_query(agent_id: &str) -> String {
    format!(
        r#"{{span.agent.id="{0}"}} || {{resource.agent.id="{0}"}} || {{resource.service.name="{0}"}}"#,
        agent_id
    )
}

/// Like `agent_query` but restricted to traces that contain at least one span
/// with `session.id` set — i.e., user-facing request traces only, excluding
/// infrastructure traces (a2a-sdk remove_sink, dispatch loops, etc.).
fn agent_session_query(agent_id: &str) -> String {
    format!(
        r#"{{span.session.id != "" && resource.service.name="{0}"}}"#,
        agent_id
    )
}

/// Extract (input_tokens, output_tokens, model) from a span's attributes map.
/// Covers both current GenAI semconv names and older/deprecated variants.
fn extract_token_attrs(attrs: &HashMap<String, Value>) -> (u64, u64, Option<String>) {
    let input = attrs
        .get("gen_ai.usage.input_tokens")         // semconv v1.27+
        .or_else(|| attrs.get("gen_ai.usage.prompt_tokens"))   // pre-1.27, still common
        .or_else(|| attrs.get("llm.usage.prompt_tokens"))      // LangChain / LlamaIndex
        .or_else(|| attrs.get("input_tokens"))
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(0);

    let output = attrs
        .get("gen_ai.usage.output_tokens")        // semconv v1.27+
        .or_else(|| attrs.get("gen_ai.usage.completion_tokens")) // pre-1.27, still common
        .or_else(|| attrs.get("llm.usage.completion_tokens"))   // LangChain / LlamaIndex
        .or_else(|| attrs.get("output_tokens"))
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(0);

    let model = attrs
        .get("gen_ai.request.model")
        .or_else(|| attrs.get("llm.request.model"))
        .or_else(|| attrs.get("model"))
        .and_then(|v| v.as_str())
        .map(String::from);

    (input, output, model)
}

/// Parse raw Loki log lines into a map of span_id → {input, output} content.
fn parse_loki_logs(lines: Vec<String>) -> HashMap<String, SpanContent> {
    let mut by_span: HashMap<String, SpanContent> = HashMap::new();

    for line in lines {
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        let span_id = entry["spanId"]
            .as_str()
            .or_else(|| entry["span_id"].as_str())
            .unwrap_or("")
            .to_string();

        if span_id.is_empty() {
            continue;
        }

        let slot = by_span.entry(span_id).or_default();

        let Some(attrs) = entry["attributes"].as_array() else {
            continue;
        };

        let mut event_name: Option<&str> = None;
        let mut prompt_content: Option<String> = None;
        let mut completion_content: Option<String> = None;

        for attr in attrs {
            let key = attr["key"].as_str().unwrap_or("");
            let val = attr["value"]["stringValue"].as_str().unwrap_or("");
            match key {
                "event.name" => event_name = attr["value"]["stringValue"].as_str(),
                "gen_ai.content.prompt" | "gen_ai.prompt" => {
                    prompt_content = Some(val.to_string());
                }
                "gen_ai.content.completion" | "gen_ai.completion" => {
                    completion_content = Some(val.to_string());
                }
                _ => {}
            }
        }

        match event_name {
            Some("gen_ai.content.prompt") => {
                if let Some(c) = prompt_content {
                    slot.input = Some(c);
                }
            }
            Some("gen_ai.content.completion") => {
                if let Some(c) = completion_content {
                    slot.output = Some(c);
                }
            }
            _ => {}
        }
    }

    by_span
}

#[derive(Default)]
struct SpanContent {
    input: Option<String>,
    output: Option<String>,
}

// ─── Span tree builder ────────────────────────────────────────────────────────

fn encode_span_id(span_id: &str) -> String {
    base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        format!("Span:{span_id}"),
    )
}

fn build_span_tree(
    spans: &[nasiko_observability::Span],
) -> (Vec<SpanNode>, HashMap<String, SpanNode>) {
    // Build flat map first
    let mut nodes: HashMap<String, SpanNode> = spans
        .iter()
        .map(|s| {
            let (input, output, _) = extract_token_attrs(&s.attributes);
            (
                s.span_id.clone(),
                SpanNode {
                    id: encode_span_id(&s.span_id),
                    span_id: s.span_id.clone(),
                    name: s.name.clone(),
                    span_kind: span_kind_str(s.kind).to_string(),
                    status_code: status_code_str(s.status_code).to_string(),
                    start_time: Some(s.started_at.to_rfc3339()),
                    end_time: s.ended_at.map(|t| t.to_rfc3339()),
                    parent_id: s.parent_span_id.as_deref().map(encode_span_id),
                    latency_ms: s.duration_ms.map(|d| d as f64),
                    token_count_total: input + output,
                    span_annotation_summaries: vec![],
                    children: vec![],
                },
            )
        })
        .collect();

    // Collect children per parent
    let mut children_map: HashMap<String, Vec<String>> = HashMap::new();
    for s in spans {
        if let Some(ref parent) = s.parent_span_id
            && nodes.contains_key(parent.as_str()) {
                children_map
                    .entry(parent.clone())
                    .or_default()
                    .push(s.span_id.clone());
            }
    }

    // Identify roots: spans whose parent_span_id is None or not in the node map
    let root_ids: Vec<String> = spans
        .iter()
        .filter(|s| {
            s.parent_span_id
                .as_ref()
                .map(|p| !nodes.contains_key(p.as_str()))
                .unwrap_or(true)
        })
        .map(|s| s.span_id.clone())
        .collect();

    fn attach_children(
        id: &str,
        nodes: &mut HashMap<String, SpanNode>,
        children_map: &HashMap<String, Vec<String>>,
    ) -> SpanNode {
        let mut node = nodes.remove(id).unwrap();
        if let Some(child_ids) = children_map.get(id) {
            let mut children: Vec<SpanNode> = child_ids
                .iter()
                .map(|cid| attach_children(cid, nodes, children_map))
                .collect();
            children.sort_by(|a, b| a.start_time.cmp(&b.start_time));
            node.children = children;
        }
        node
    }

    // Detach nodes into tree (we need to move them out of the map)
    // First snapshot so we can rebuild the lookup after tree building
    // span_lookup keys are base64-encoded span IDs to match the `id` field
    let snapshot: HashMap<String, SpanNode> = spans
        .iter()
        .map(|s| {
            let (input, output, _) = extract_token_attrs(&s.attributes);
            (
                encode_span_id(&s.span_id),
                SpanNode {
                    id: encode_span_id(&s.span_id),
                    span_id: s.span_id.clone(),
                    name: s.name.clone(),
                    span_kind: span_kind_str(s.kind).to_string(),
                    status_code: status_code_str(s.status_code).to_string(),
                    start_time: Some(s.started_at.to_rfc3339()),
                    end_time: s.ended_at.map(|t| t.to_rfc3339()),
                    parent_id: s.parent_span_id.as_deref().map(encode_span_id),
                    latency_ms: s.duration_ms.map(|d| d as f64),
                    token_count_total: input + output,
                    span_annotation_summaries: vec![],
                    children: vec![],
                },
            )
        })
        .collect();

    #[allow(clippy::filter_map_bool_then)]
    let mut root_nodes: Vec<SpanNode> = root_ids
        .iter()
        .filter_map(|id| {
            nodes.contains_key(id.as_str()).then(|| {
                attach_children(id, &mut nodes, &children_map)
            })
        })
        .collect();

    root_nodes.sort_by(|a, b| a.start_time.cmp(&b.start_time));

    (root_nodes, snapshot)
}

// ─── Response types ───────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct SessionListResponse {
    pub data: SessionListData,
}

#[derive(Serialize)]
pub struct SessionListData {
    pub sessions: Vec<SessionSummary>,
    pub total_agents: usize,
    pub successful_agents: usize,
    pub pagination: Pagination,
}

#[derive(Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub session_id: String,
    pub agent_id: String,
    pub num_traces: Option<u32>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub duration_ms: Option<u64>,
    pub first_input: Option<String>,
    pub last_output: Option<String>,
    pub token_usage: TokenUsageSummary,
    pub trace_latency_ms_p50: Option<f64>,
    pub trace_latency_ms_p99: Option<f64>,
    pub cost_summary: SimpleCostSummary,
    pub session_annotations: Vec<Value>,
    pub session_annotation_summaries: Vec<Value>,
}

#[derive(Serialize)]
pub struct TokenUsageSummary {
    pub total: Option<u64>,
}

#[derive(Serialize)]
pub struct SimpleCostSummary {
    pub total: CostEntry,
}

#[derive(Serialize)]
pub struct CostEntry {
    pub cost: Option<f64>,
}

#[derive(Serialize, Clone)]
pub struct Pagination {
    pub end_cursor: Option<String>,
    pub has_next_page: bool,
}

// session/{session_id}

#[derive(Serialize)]
pub struct SessionDetailResponse {
    pub data: SessionDetailData,
}

#[derive(Serialize)]
pub struct SessionDetailData {
    pub session: SessionDetail,
}

#[derive(Serialize)]
pub struct SessionDetail {
    pub id: String,
    pub session_id: String,
    pub num_traces: usize,
    pub token_usage: TokenUsageSummary,
    pub cost_summary: FullCostSummary,
    pub latency_p50: Option<f64>,
    pub traces: Vec<TraceEntry>,
    pub pagination: Pagination,
}

#[derive(Serialize)]
pub struct FullCostSummary {
    pub total: CostWithTokens,
    pub prompt: CostWithTokens,
    pub completion: CostWithTokens,
}

#[derive(Serialize)]
pub struct CostWithTokens {
    pub cost: f64,
    pub tokens: u64,
}

#[derive(Serialize)]
pub struct TraceEntry {
    pub id: String,
    pub trace_id: String,
    pub root_span: RootSpanEntry,
    pub cursor: String,
}

#[derive(Serialize)]
pub struct RootSpanEntry {
    pub id: String,
    pub span_id: String,
    pub attributes: String,
    pub cumulative_token_count_total: u64,
    pub latency_ms: f64,
    pub start_time: Option<String>,
    pub span_annotations: Vec<Value>,
    pub span_annotation_summaries: Vec<Value>,
    pub project: ProjectRef,
    pub input: ContentField,
    pub output: ContentField,
    pub trace: TraceRef,
}

#[derive(Serialize)]
pub struct ProjectRef {
    pub id: String,
}

#[derive(Serialize)]
pub struct ContentField {
    pub value: String,
    pub mime_type: String,
    pub parsed_value: Option<Value>,
}

#[derive(Serialize)]
pub struct TraceRef {
    pub id: String,
    pub cost_summary: Value,
}

// trace/{project_id}/{trace_id}

#[derive(Serialize)]
pub struct TraceDetailResponse {
    pub data: TraceDetailData,
}

#[derive(Serialize)]
pub struct TraceDetailData {
    pub trace: TraceDetail,
}

#[derive(Serialize)]
pub struct TraceDetail {
    pub id: String,
    pub project_session_id: Option<String>,
    pub num_spans: usize,
    pub latency_ms: Option<f64>,
    pub cost_summary: NestedCostSummary,
    pub root_spans: RootSpansWrapper,
    pub spans: Vec<SpanNode>,
    pub span_lookup: HashMap<String, SpanNode>,
}

#[derive(Serialize)]
pub struct NestedCostSummary {
    pub total: CostOnly,
    pub prompt: CostOnly,
    pub completion: CostOnly,
}

#[derive(Serialize)]
pub struct CostOnly {
    pub cost: f64,
}

#[derive(Serialize)]
pub struct RootSpansWrapper {
    pub edges: Vec<RootSpanEdge>,
}

#[derive(Serialize)]
pub struct RootSpanEdge {
    pub span: RootSpanRef,
}

#[derive(Serialize)]
pub struct RootSpanRef {
    pub id: String,
    pub span_id: String,
    pub parent_id: Option<String>,
    pub status_code: String,
}

#[derive(Serialize, Clone)]
pub struct SpanNode {
    pub id: String,
    pub span_id: String,
    pub name: String,
    pub span_kind: String,
    pub status_code: String,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub parent_id: Option<String>,
    pub latency_ms: Option<f64>,
    pub token_count_total: u64,
    pub span_annotation_summaries: Vec<Value>,
    pub children: Vec<SpanNode>,
}

// span/{trace_id}/{span_id}

#[derive(Serialize)]
pub struct SpanDetailResponse {
    pub data: SpanDetailData,
}

#[derive(Serialize)]
pub struct SpanDetailData {
    pub span: SpanDetail,
}

#[derive(Serialize)]
pub struct SpanTraceRef {
    pub id: String,
    pub trace_id: String,
}

#[derive(Serialize)]
pub struct SpanProjectRef {
    pub id: String,
    pub annotation_configs: Value,
}

#[derive(Serialize)]
pub struct SpanDetail {
    pub id: String,
    pub span_id: String,
    pub trace: SpanTraceRef,
    pub name: String,
    pub span_kind: String,
    pub status_code: String,
    pub code: String,
    pub status_message: String,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub parent_id: Option<String>,
    pub latency_ms: Option<f64>,
    pub token_count_total: u64,
    pub cost_summary: SimpleCostSummary,
    pub input: ContentField,
    pub output: ContentField,
    pub attributes: Value,
    pub events: Vec<Value>,
    pub span_annotations: Vec<Value>,
    pub span_annotation_summaries: Vec<Value>,
    pub document_retrieval_metrics: Vec<Value>,
    pub document_evaluations: Vec<Value>,
    pub project: SpanProjectRef,
}

// agent/{agent_id}/stats

#[derive(Serialize)]
pub struct AgentStatsResponse {
    pub data: AgentStatsData,
}

#[derive(Serialize)]
pub struct AgentStatsData {
    pub project: AgentProjectStats,
}

#[derive(Serialize)]
pub struct AgentProjectStats {
    pub id: String,
    pub trace_count: usize,
    pub cost_summary: NestedCostSummary,
    pub latency_ms_p50: Option<f64>,
    pub latency_ms_p99: Option<f64>,
    pub span_annotation_names: Vec<String>,
    pub document_evaluation_names: Vec<String>,
}

// finops/dashboard

#[derive(Serialize)]
pub struct FinopsDashboardResponse {
    pub data: FinopsDashboardData,
    pub status_code: u16,
    pub message: String,
}

#[derive(Serialize)]
pub struct FinopsDashboardData {
    pub summary: FinopsSummary,
    pub agents: Vec<AgentFinopsRow>,
    pub token_usage: FinopsTokenUsage,
}

#[derive(Serialize)]
pub struct FinopsSummary {
    pub total_cost: f64,
    pub total_operations: usize,
    pub operations_last_24h: usize,
    pub average_cost: f64,
    pub active_agents: usize,
    pub total_agents: usize,
}

#[derive(Serialize)]
pub struct AgentFinopsRow {
    pub agent_id: String,
    pub agent_name: String,
    pub total_cost: f64,
    pub operations: usize,
    pub avg_cost_per_operation: f64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub avg_latency_ms: Option<f64>,
    pub version: Option<String>,
}

#[derive(Serialize)]
pub struct FinopsTokenUsage {
    pub total_tokens: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub avg_tokens_per_operation: u64,
}

// finops/insights

#[derive(Deserialize)]
pub struct InsightsRequest {
    pub kpi: Value,
    pub agent_costs: Vec<Value>,
}

#[derive(Serialize)]
pub struct InsightsResponse {
    pub insights: Vec<String>,
}

// ─── Service ──────────────────────────────────────────────────────────────────

pub struct ObservabilityService {
    tempo: TempoClient,
    loki: LokiClient,
    db: PgPool,
    http_client: reqwest::Client,
    config: Arc<Config>,
    redis: redis::Client,
}

impl ObservabilityService {
    pub fn from_state(state: &crate::state::AppState) -> Self {
        Self {
            tempo: TempoClient::new(state.config.tempo_url.clone()),
            loki: LokiClient::new(state.config.loki_url.clone()),
            db: state.db.clone(),
            http_client: state.http_client.clone(),
            config: state.config.clone(),
            redis: state.redis.clone(),
        }
    }

    /// Look up the session ID for a given Tempo trace_id from Redis.
    /// This covers pre-built agents (deployed via `nasiko deploy`) that don't
    /// have our sitecustomize.py patch and therefore never set session.id on
    /// their spans. The `agent_proxy` writes this mapping when it forwards
    /// A2A requests.
    async fn redis_session_id(&self, trace_id: &str) -> Option<String> {
        let mut conn = self.redis.get_multiplexed_async_connection().await.ok()?;
        let key = format!("nasiko:trace:{trace_id}:session");
        redis::cmd("GET")
            .arg(&key)
            .query_async::<Option<String>>(&mut conn)
            .await
            .ok()
            .flatten()
    }

    // ── Accessible agents ────────────────────────────────────────────────────

    /// Returns Vec<(id, name, display_name)> for all agents in the DB. `name`
    /// doubles as the Tempo `service.name`; `id` is the UUID reported to callers.
    /// OSS: returns all agents (NoopAuthorizer). EE adds RBAC at a higher layer.
    async fn get_agent_names(
        &self,
    ) -> Result<Vec<(uuid::Uuid, String, String)>, ObservabilityError> {
        sqlx::query_as::<_, (uuid::Uuid, String, String)>(
            "SELECT id, name, COALESCE(display_name, name) FROM agents ORDER BY name",
        )
        .fetch_all(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))
    }

    // ── Internal Tempo helpers ───────────────────────────────────────────────

    async fn tempo_search(
        &self,
        agent_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<(String, Option<DateTime<Utc>>, Option<u64>)>, ObservabilityError> {
        let start = clamp_tempo_range(start, end);
        self.tempo.search(&agent_query(agent_id), Some(start), Some(end), limit).await
    }

    /// Same as `tempo_search` but filters to user-facing traces only (those with
    /// `session.id` set on at least one span). Excludes a2a-sdk infrastructure
    /// traces such as `remove_sink` and dispatch loops.
    async fn tempo_search_user_traces(
        &self,
        agent_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<(String, Option<DateTime<Utc>>, Option<u64>)>, ObservabilityError> {
        let start = clamp_tempo_range(start, end);
        self.tempo
            .search(&agent_session_query(agent_id), Some(start), Some(end), limit)
            .await
    }

    async fn fetch_sessions_for_agent(
        &self,
        agent_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<SessionSummary>, ObservabilityError> {
        let results = self.tempo_search(agent_id, start, end, 100).await?;

        // Group traces by session.id span attribute (= A2A contextId).
        // Traces that have no session.id each become their own session (fallback: trace_id).
        struct SessionAccum {
            trace_ids: Vec<String>,
            earliest_start: Option<DateTime<Utc>>,
            latest_end: Option<DateTime<Utc>>,
            total_input: u64,
            total_output: u64,
            model_used: Option<String>,
            span_durations: Vec<u64>,
        }
        let mut by_session: std::collections::HashMap<String, SessionAccum> =
            std::collections::HashMap::new();

        for (trace_id, started_at, duration_ms) in results {
            let mut session_key: Option<String> = None;
            let mut trace_input = 0u64;
            let mut trace_output = 0u64;
            let mut trace_model: Option<String> = None;
            let mut trace_span_durations: Vec<u64> = Vec::new();

            if let Ok(trace) = self.tempo.get_trace(&trace_id).await {
                for span in &trace.spans {
                    // Pick up the session.id tag set by the agent executor
                    if session_key.is_none() {
                        session_key = span
                            .attributes
                            .get("session.id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                    }
                    let (inp, out, model) = extract_token_attrs(&span.attributes);
                    if inp > 0 || out > 0 {
                        trace_input += inp;
                        trace_output += out;
                        if trace_model.is_none() {
                            trace_model = model;
                        }
                    }
                    let op = span.attributes.get("gen_ai.operation.name").and_then(|v| v.as_str());
                    if matches!(op, None | Some("chat")) {
                        if let Some(d) = span.duration_ms {
                            trace_span_durations.push(d);
                        }
                    }
                }
            }

            // Fallback: for pre-built agents (deployed via `nasiko deploy`) that
            // don't have our sitecustomize.py patch, session.id is never set on
            // agent spans. The agent_proxy stores trace_id → session_id in Redis
            // when it forwards A2A requests, so we check Redis here.
            if session_key.is_none() {
                session_key = self.redis_session_id(&trace_id).await;
            }

            // Skip traces with no session association — these are a2a-sdk
            // infrastructure traces (remove_sink, dispatch loops, etc.).
            let Some(key) = session_key else { continue };
            let end_time = started_at.zip(duration_ms).map(|(s, d)| {
                s + Duration::milliseconds(d as i64)
            });

            let entry = by_session.entry(key).or_insert_with(|| SessionAccum {
                trace_ids: Vec::new(),
                earliest_start: None,
                latest_end: None,
                total_input: 0,
                total_output: 0,
                model_used: None,
                span_durations: Vec::new(),
            });

            entry.trace_ids.push(trace_id);
            if let Some(s) = started_at {
                entry.earliest_start = Some(match entry.earliest_start {
                    Some(prev) => prev.min(s),
                    None => s,
                });
            }
            if let Some(e) = end_time {
                entry.latest_end = Some(match entry.latest_end {
                    Some(prev) => prev.max(e),
                    None => e,
                });
            }
            entry.total_input += trace_input;
            entry.total_output += trace_output;
            if entry.model_used.is_none() {
                entry.model_used = trace_model;
            }
            entry.span_durations.extend(trace_span_durations);
        }

        let mut sessions = Vec::with_capacity(by_session.len());
        for (session_id, mut acc) in by_session {
            acc.span_durations.sort_unstable();
            let len = acc.span_durations.len();
            let p50 = acc.span_durations.get(len / 2).map(|&v| v as f64);
            let p99 = acc.span_durations
                .get((len * 99 / 100).saturating_sub(1))
                .map(|&v| v as f64);

            let total_tokens = acc.total_input + acc.total_output;
            let (_, _, cost) = compute_cost(acc.total_input, acc.total_output, acc.model_used.as_deref());
            let num_traces = acc.trace_ids.len() as u32;

            let duration_ms = match (acc.earliest_start, acc.latest_end) {
                (Some(s), Some(e)) => Some((e - s).num_milliseconds().max(0) as u64),
                _ => None,
            };

            sessions.push(SessionSummary {
                id: session_id.clone(),
                session_id,
                agent_id: agent_id.to_string(),
                num_traces: Some(num_traces),
                start_time: acc.earliest_start.map(|t| t.to_rfc3339()),
                end_time: acc.latest_end.map(|t| t.to_rfc3339()),
                duration_ms,
                first_input: None,
                last_output: None,
                token_usage: TokenUsageSummary {
                    total: if total_tokens > 0 { Some(total_tokens) } else { None },
                },
                trace_latency_ms_p50: p50,
                trace_latency_ms_p99: p99,
                cost_summary: SimpleCostSummary {
                    total: CostEntry { cost: if cost > 0.0 { Some(cost) } else { None } },
                },
                session_annotations: vec![],
                session_annotation_summaries: vec![],
            });
        }

        Ok(sessions)
    }

    // ── 1. session/list ──────────────────────────────────────────────────────

    pub async fn get_all_sessions(
        &self,
        _user_id: &str,
        _role: Option<&str>,
        _department_id: Option<&str>,
        _team_id: Option<&str>,
        start_time: Option<&str>,
    ) -> Result<SessionListResponse, ObservabilityError> {
        let agents = self.get_agent_names().await?;
        let total = agents.len();
        let start = parse_iso_or_default(start_time, 7);
        let end = Utc::now();

        let mut all_sessions: Vec<SessionSummary> = Vec::new();
        let mut successful = 0usize;

        for (_, agent_name, _) in &agents {
            match self.fetch_sessions_for_agent(agent_name, start, end).await {
                Ok(sessions) => {
                    all_sessions.extend(sessions);
                    successful += 1;
                }
                Err(e) => {
                    tracing::warn!(agent_name, error = %e, "failed to get sessions from Tempo");
                }
            }
        }

        all_sessions.sort_by(|a, b| b.start_time.cmp(&a.start_time));

        Ok(SessionListResponse {
            data: SessionListData {
                sessions: all_sessions,
                total_agents: total,
                successful_agents: successful,
                pagination: Pagination { end_cursor: None, has_next_page: false },
            },
        })
    }

    // ── 2. session/{session_id} ──────────────────────────────────────────────

    pub async fn get_session_details(
        &self,
        session_id: &str,
    ) -> Result<SessionDetailResponse, ObservabilityError> {
        // session_id is the A2A contextId (e.g. "ses_14cda..."), not a Tempo trace ID.
        // Find all traces that belong to this session via the span attribute set by the agent.
        let query = format!(r#"{{span.session.id="{session_id}"}}"#);
        let end = Utc::now();
        let start = clamp_tempo_range(end - Duration::days(7), end);
        let trace_results = self.tempo.search(&query, Some(start), Some(end), 100).await?;

        if trace_results.is_empty() {
            return Err(ObservabilityError::NotFound(format!("session '{session_id}'")));
        }

        let mut total_input = 0u64;
        let mut total_output = 0u64;
        let mut model_used: Option<String> = None;
        let mut latencies: Vec<f64> = Vec::new();
        let mut trace_entries: Vec<TraceEntry> = Vec::new();

        for (trace_idx, (trace_id, _, _)) in trace_results.iter().enumerate() {
            let Ok(trace) = self.tempo.get_trace(trace_id).await else { continue };

            // Identify root span: one whose parent is not present in this trace.
            let span_ids_set: std::collections::HashSet<&str> =
                trace.spans.iter().map(|s| s.span_id.as_str()).collect();
            let root_span = trace.spans.iter().find(|s| {
                s.parent_span_id
                    .as_ref()
                    .map(|p| !span_ids_set.contains(p.as_str()))
                    .unwrap_or(true)
            });
            let Some(root_span) = root_span else { continue };

            // Aggregate tokens across all spans in this trace
            let mut trace_input = 0u64;
            let mut trace_output = 0u64;
            let mut trace_model: Option<String> = None;
            for span in &trace.spans {
                let (inp, out, model) = extract_token_attrs(&span.attributes);
                if inp > 0 || out > 0 {
                    trace_input += inp;
                    trace_output += out;
                    if trace_model.is_none() {
                        trace_model = model;
                    }
                }
            }
            total_input += trace_input;
            total_output += trace_output;
            if model_used.is_none() {
                model_used = trace_model.clone();
            }

            let service_name = trace.spans.first().map(|s| s.service_name.clone());

            // Fetch Loki logs for this trace best-effort
            let logs_by_span = if let Some(ref svc) = service_name {
                self.loki
                    .get_trace_logs(svc, trace_id, trace.started_at, trace.ended_at)
                    .await
                    .map(parse_loki_logs)
                    .unwrap_or_default()
            } else {
                HashMap::new()
            };

            let duration_ms = root_span.duration_ms.unwrap_or(0) as f64;
            if duration_ms > 0.0 {
                latencies.push(duration_ms);
            }

            let logs = logs_by_span.get(&root_span.span_id);
            let flat_attrs: serde_json::Map<String, Value> = root_span
                .attributes
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let (_, _, trace_cost) = compute_cost(trace_input, trace_output, trace_model.as_deref());

            let cursor = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!("connection:{trace_idx}"),
            );
            let trace_id_enc = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!("Trace:{trace_id}"),
            );
            let span_id_enc = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!("Span:{}", &root_span.span_id),
            );

            trace_entries.push(TraceEntry {
                id: trace_id_enc.clone(),
                trace_id: trace_id.clone(),
                root_span: RootSpanEntry {
                    id: span_id_enc,
                    span_id: root_span.span_id.clone(),
                    attributes: serde_json::to_string(&flat_attrs).unwrap_or_default(),
                    cumulative_token_count_total: trace_input + trace_output,
                    latency_ms: round6(duration_ms),
                    start_time: Some(root_span.started_at.to_rfc3339()),
                    span_annotations: vec![],
                    span_annotation_summaries: vec![],
                    project: ProjectRef { id: String::new() },
                    input: ContentField {
                        value: logs.and_then(|l| l.input.clone()).unwrap_or_default(),
                        mime_type: "text".into(),
                        parsed_value: None,
                    },
                    output: ContentField {
                        value: logs.and_then(|l| l.output.clone()).unwrap_or_default(),
                        mime_type: "text".into(),
                        parsed_value: None,
                    },
                    trace: TraceRef {
                        id: trace_id_enc,
                        cost_summary: serde_json::json!({
                            "total": { "cost": trace_cost }
                        }),
                    },
                },
                cursor: cursor.clone(),
            });
        }

        let total_tokens = total_input + total_output;
        let (prompt_cost, completion_cost, total_cost) =
            compute_cost(total_input, total_output, model_used.as_deref());

        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p50 = latencies.get(latencies.len() / 2).copied();

        let end_cursor = trace_entries.last().map(|e| e.cursor.clone());

        Ok(SessionDetailResponse {
            data: SessionDetailData {
                session: SessionDetail {
                    id: session_id.to_string(),
                    session_id: session_id.to_string(),
                    num_traces: trace_results.len(),
                    token_usage: TokenUsageSummary { total: Some(total_tokens) },
                    cost_summary: FullCostSummary {
                        total: CostWithTokens { cost: total_cost, tokens: total_tokens },
                        prompt: CostWithTokens { cost: prompt_cost, tokens: total_input },
                        completion: CostWithTokens { cost: completion_cost, tokens: total_output },
                    },
                    latency_p50: p50,
                    traces: trace_entries,
                    pagination: Pagination { end_cursor, has_next_page: false },
                },
            },
        })
    }

    // ── 3. trace/{trace_id} ──────────────────────────────────────────────────

    pub async fn get_trace_details(
        &self,
        trace_id: &str,
    ) -> Result<TraceDetailResponse, ObservabilityError> {
        let trace = self.tempo.get_trace(trace_id).await?;

        let mut total_input = 0u64;
        let mut total_output = 0u64;
        let mut model_used: Option<String> = None;

        for span in &trace.spans {
            let (inp, out, model) = extract_token_attrs(&span.attributes);
            total_input += inp;
            total_output += out;
            if model_used.is_none() {
                model_used = model;
            }
        }

        let (prompt_cost, completion_cost, total_cost) =
            compute_cost(total_input, total_output, model_used.as_deref());

        let trace_latency_ms = match (trace.started_at, trace.ended_at) {
            (Some(s), Some(e)) => Some((e - s).num_milliseconds().max(0) as f64),
            _ => None,
        };

        // Extract session.id from any span that carries it.
        let project_session_id = trace
            .spans
            .iter()
            .find_map(|s| {
                s.attributes
                    .get("session.id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            });

        let num_spans = trace.spans.len();
        let (root_nodes, span_lookup) = build_span_tree(&trace.spans);

        let root_edges: Vec<RootSpanEdge> = root_nodes
            .iter()
            .map(|s| RootSpanEdge {
                span: RootSpanRef {
                    id: s.id.clone(),
                    span_id: s.span_id.clone(),
                    parent_id: None,
                    status_code: s.status_code.clone(),
                },
            })
            .collect();

        Ok(TraceDetailResponse {
            data: TraceDetailData {
                trace: TraceDetail {
                    id: trace_id.to_string(),
                    project_session_id,
                    num_spans,
                    latency_ms: trace_latency_ms,
                    cost_summary: NestedCostSummary {
                        total: CostOnly { cost: total_cost },
                        prompt: CostOnly { cost: prompt_cost },
                        completion: CostOnly { cost: completion_cost },
                    },
                    root_spans: RootSpansWrapper { edges: root_edges },
                    spans: root_nodes,
                    span_lookup,
                },
            },
        })
    }

    // ── 4. span/{trace_id}/{span_id} ─────────────────────────────────────────

    pub async fn get_span_details(
        &self,
        trace_id: &str,
        span_id: &str,
    ) -> Result<SpanDetailResponse, ObservabilityError> {
        let trace = self.tempo.get_trace(trace_id).await?;

        let span = trace
            .spans
            .iter()
            .find(|s| s.span_id == span_id)
            .ok_or_else(|| {
                ObservabilityError::NotFound(format!("span '{span_id}' in trace '{trace_id}'"))
            })?;

        // Best-effort Loki fetch for input/output content.
        // service_name comes from resource.service.name in Tempo; fallback to
        // code.namespace span attribute if the resource attr was not set.
        let logs = {
            let svc = if span.service_name.is_empty() {
                span.attributes
                    .get("code.namespace")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                span.service_name.clone()
            };

            if svc.is_empty() {
                tracing::warn!(trace_id, span_id, "loki: service_name empty — skipping");
                None
            } else {
                // Pad the time window so slight clock skew doesn't exclude logs.
                let start = trace.started_at.map(|t| t - chrono::Duration::minutes(1));
                let end = trace.ended_at.map(|t| t + chrono::Duration::minutes(1));

                tracing::warn!(svc, trace_id, span_id, "loki: querying");
                match self.loki.get_trace_logs(&svc, trace_id, start, end).await {
                    Ok(lines) => {
                        tracing::warn!(
                            svc,
                            trace_id,
                            span_id,
                            count = lines.len(),
                            "loki: got lines"
                        );
                        let mut map = parse_loki_logs(lines);
                        let result = map.remove(span_id);
                        tracing::warn!(
                            span_id,
                            found = result.is_some(),
                            map_keys = ?map.keys().collect::<Vec<_>>(),
                            "loki: span lookup result"
                        );
                        result
                    }
                    Err(e) => {
                        tracing::warn!(svc, trace_id, error = %e, "loki: fetch failed");
                        None
                    }
                }
            }
        };

        let (input_tokens, output_tokens, model) = extract_token_attrs(&span.attributes);
        let (_, _, total_cost) = compute_cost(input_tokens, output_tokens, model.as_deref());

        // Span kind: prefer openinference.span.kind (e.g. "LLM"), fallback to OTel kind string
        let span_kind = span
            .attributes
            .get("openinference.span.kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| span_kind_str(span.kind).to_lowercase());

        // Input: prefer span attribute "input.value", fallback to Loki log
        let input_value = span
            .attributes
            .get("input.value")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| logs.as_ref().and_then(|l| l.input.clone()))
            .unwrap_or_default();
        let input_parsed: Option<Value> = serde_json::from_str(&input_value).ok();
        let input_mime = if input_parsed.is_some() {
            "json".to_string()
        } else {
            span.attributes
                .get("input.mime_type")
                .and_then(|v| v.as_str())
                .unwrap_or("text")
                .to_string()
        };

        // Output: prefer span attribute "output.value", fallback to Loki log
        let output_value = span
            .attributes
            .get("output.value")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| logs.as_ref().and_then(|l| l.output.clone()))
            .unwrap_or_default();
        let output_parsed: Option<Value> = serde_json::from_str(&output_value).ok();
        let output_mime = if output_parsed.is_some() {
            "json".to_string()
        } else {
            span.attributes
                .get("output.mime_type")
                .and_then(|v| v.as_str())
                .unwrap_or("text")
                .to_string()
        };

        let span_id_enc = encode_span_id(&span.span_id);
        let trace_id_enc = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("Trace:{trace_id}"),
        );
        let status = status_code_str(span.status_code).to_string();

        Ok(SpanDetailResponse {
            data: SpanDetailData {
                span: SpanDetail {
                    id: span_id_enc,
                    span_id: span.span_id.clone(),
                    trace: SpanTraceRef {
                        id: trace_id_enc,
                        trace_id: trace_id.to_string(),
                    },
                    name: span.name.clone(),
                    span_kind,
                    code: status.clone(),
                    status_code: status,
                    status_message: span.status_message.clone(),
                    start_time: Some(span.started_at.to_rfc3339()),
                    end_time: span.ended_at.map(|t| t.to_rfc3339()),
                    parent_id: span.parent_span_id.clone(),
                    latency_ms: span.duration_ms.map(|d| d as f64),
                    token_count_total: input_tokens + output_tokens,
                    cost_summary: SimpleCostSummary {
                        total: CostEntry { cost: Some(total_cost) },
                    },
                    input: ContentField {
                        value: input_value,
                        mime_type: input_mime,
                        parsed_value: input_parsed,
                    },
                    output: ContentField {
                        value: output_value,
                        mime_type: output_mime,
                        parsed_value: output_parsed,
                    },
                    attributes: unflatten_attrs(&span.attributes),
                    events: vec![],
                    span_annotations: vec![],
                    span_annotation_summaries: vec![],
                    document_retrieval_metrics: vec![],
                    document_evaluations: vec![],
                    project: SpanProjectRef {
                        id: String::new(),
                        annotation_configs: serde_json::json!({ "edges": [], "configs": [] }),
                    },
                },
            },
        })
    }

    // ── 5. agent/{agent_id}/stats ─────────────────────────────────────────────

    pub async fn get_agent_stats(
        &self,
        agent_id: &str,
        start_time: &str,
    ) -> Result<AgentStatsResponse, ObservabilityError> {
        let start = parse_iso_or_default(Some(start_time), 1);
        let end = Utc::now();

        let results = self.tempo_search_user_traces(agent_id, start, end, 1000).await?;
        let trace_count = results.len();

        let mut durations: Vec<u64> = results
            .iter()
            .filter_map(|(_, _, d)| *d)
            .collect();
        durations.sort_unstable();

        let p50 = durations.get(durations.len() / 2).map(|&v| v as f64);
        let p99 = durations
            .get((durations.len() * 99 / 100).saturating_sub(1))
            .map(|&v| v as f64);

        let mut total_input = 0u64;
        let mut total_output = 0u64;
        let mut model_used: Option<String> = None;

        for (trace_id, _, _) in results.iter().take(100) {
            match self.tempo.get_trace(trace_id).await {
                Ok(trace) => {
                    for span in &trace.spans {
                        let (inp, out, model) = extract_token_attrs(&span.attributes);
                        total_input += inp;
                        total_output += out;
                        if model_used.is_none() {
                            model_used = model;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(trace_id, error = %e, "token fetch failed");
                }
            }
        }

        let (prompt_cost, completion_cost, total_cost) =
            compute_cost(total_input, total_output, model_used.as_deref());

        Ok(AgentStatsResponse {
            data: AgentStatsData {
                project: AgentProjectStats {
                    id: agent_id.to_string(),
                    trace_count,
                    cost_summary: NestedCostSummary {
                        total: CostOnly { cost: total_cost },
                        prompt: CostOnly { cost: prompt_cost },
                        completion: CostOnly { cost: completion_cost },
                    },
                    latency_ms_p50: p50,
                    latency_ms_p99: p99,
                    span_annotation_names: vec![],
                    document_evaluation_names: vec![],
                },
            },
        })
    }

    // ── 6. finops/dashboard ───────────────────────────────────────────────────

    pub async fn get_finops_dashboard(
        &self,
        _user_id: &str,
        _role: Option<&str>,
        _department_id: Option<&str>,
        _team_id: Option<&str>,
        start_time: Option<&str>,
    ) -> Result<FinopsDashboardResponse, ObservabilityError> {
        let agents = self.get_agent_names().await?;
        let total_agents = agents.len();

        if agents.is_empty() {
            return Ok(empty_finops_response());
        }

        let now = Utc::now();
        let start = parse_iso_or_default(start_time, 30);
        let last_24h = now - Duration::hours(24);

        let mut agent_rows: Vec<AgentFinopsRow> = Vec::new();
        let mut grand_input = 0u64;
        let mut grand_output = 0u64;
        let mut total_ops = 0usize;
        let mut total_ops_24h = 0usize;
        let mut active = 0usize;

        for (agent_uuid, agent_name, display_name) in &agents {
            let traces = self
                .tempo_search_user_traces(agent_name, start, now, 1000)
                .await
                .unwrap_or_default();
            let traces_24h = self
                .tempo_search_user_traces(agent_name, last_24h, now, 1000)
                .await
                .unwrap_or_default();

            let ops = traces.len();
            let ops_24h = traces_24h.len();

            if ops > 0 {
                active += 1;
            }

            let mut durations: Vec<u64> = traces.iter().filter_map(|(_, _, d)| *d).collect();
            durations.sort_unstable();
            let p50 = durations.get(durations.len() / 2).map(|&v| v as f64);

            let mut total_input = 0u64;
            let mut total_output = 0u64;
            let mut model_used: Option<String> = None;

            for (trace_id, _, _) in traces.iter().take(100) {
                match self.tempo.get_trace(trace_id).await {
                    Ok(trace) => {
                        for span in &trace.spans {
                            let (inp, out, model) = extract_token_attrs(&span.attributes);
                            total_input += inp;
                            total_output += out;
                            if model_used.is_none() {
                                model_used = model;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(trace_id, error = %e, "token fetch failed");
                    }
                }
            }

            let (_, _, total_cost) =
                compute_cost(total_input, total_output, model_used.as_deref());
            let avg_cost = if ops > 0 {
                round6(total_cost / ops as f64)
            } else {
                0.0
            };

            grand_input += total_input;
            grand_output += total_output;
            total_ops += ops;
            total_ops_24h += ops_24h;

            agent_rows.push(AgentFinopsRow {
                agent_id: agent_uuid.to_string(),
                agent_name: display_name.clone(),
                total_cost,
                operations: ops,
                avg_cost_per_operation: avg_cost,
                prompt_tokens: total_input,
                completion_tokens: total_output,
                total_tokens: total_input + total_output,
                avg_latency_ms: p50,
                version: None,
            });
        }

        let (_, _, grand_cost) = compute_cost(grand_input, grand_output, None);
        let avg_cost = if total_ops > 0 {
            round6(grand_cost / total_ops as f64)
        } else {
            0.0
        };
        let grand_total_tokens = grand_input + grand_output;
        let avg_tpo = if total_ops > 0 {
            grand_total_tokens / total_ops as u64
        } else {
            0
        };

        Ok(FinopsDashboardResponse {
            data: FinopsDashboardData {
                summary: FinopsSummary {
                    total_cost: round6(grand_cost),
                    total_operations: total_ops,
                    operations_last_24h: total_ops_24h,
                    average_cost: avg_cost,
                    active_agents: active,
                    total_agents,
                },
                agents: agent_rows,
                token_usage: FinopsTokenUsage {
                    total_tokens: grand_total_tokens,
                    prompt_tokens: grand_input,
                    completion_tokens: grand_output,
                    avg_tokens_per_operation: avg_tpo,
                },
            },
            status_code: 200,
            message: "FinOps dashboard data retrieved successfully".into(),
        })
    }

    // ── 7. finops/insights ────────────────────────────────────────────────────

    pub async fn get_finops_insights(
        &self,
        payload: &InsightsRequest,
    ) -> Result<InsightsResponse, ObservabilityError> {
        let base_url = self
            .config
            .openai_base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1");
        let api_key = self.config.openai_api_key.as_deref().unwrap_or_default();

        let prompt = format!(
            r#"You are a FinOps analyst reviewing AI agent usage metrics for the last 30 days.
Analyze the data and return exactly 3 bullet points — no headers, no markdown, no numbering.
Each bullet must:
- Start with the "•" character
- Be under 30 words
- Be specific with dollar amounts or percentages from the data

Cover: (1) highest cost driver, (2) efficiency observation, (3) one actionable cost-reduction recommendation.

Data: {}"#,
            serde_json::json!({ "kpi": payload.kpi, "agent_costs": payload.agent_costs })
        );

        let body = serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": 200,
            "temperature": 0.3,
        });

        let resp = self
            .http_client
            .post(format!("{base_url}/chat/completions"))
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ObservabilityError::Internal(format!(
                "LLM HTTP {status}: {text}"
            )));
        }

        let json: Value = resp
            .json()
            .await
            .map_err(|e| ObservabilityError::Deserialization(e.to_string()))?;

        let text = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("");
        let insights: Vec<String> = text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .take(3)
            .collect();

        Ok(InsightsResponse { insights })
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn empty_finops_response() -> FinopsDashboardResponse {
    FinopsDashboardResponse {
        data: FinopsDashboardData {
            summary: FinopsSummary {
                total_cost: 0.0,
                total_operations: 0,
                operations_last_24h: 0,
                average_cost: 0.0,
                active_agents: 0,
                total_agents: 0,
            },
            agents: vec![],
            token_usage: FinopsTokenUsage {
                total_tokens: 0,
                prompt_tokens: 0,
                completion_tokens: 0,
                avg_tokens_per_operation: 0,
            },
        },
        status_code: 200,
        message: "FinOps dashboard data retrieved successfully".into(),
    }
}
