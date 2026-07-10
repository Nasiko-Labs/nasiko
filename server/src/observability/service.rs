//! HTTP-shape adapter for the observability endpoints.
//!
//! All trace/log/pricing logic lives in the `nasiko-observability` crate
//! behind [`ObservabilityProvider`]; this module only maps domain types to
//! the JSON response shapes the UI and CLI expect, plus the two pieces that
//! genuinely belong to the server: agent-name resolution (DB) and the
//! FinOps insights LLM call.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use nasiko_config::Config;
use nasiko_observability::{
    AgentFinOps, ObservabilityError, ObservabilityProvider, extract_token_attrs,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

// ─── Presentation helpers ─────────────────────────────────────────────────────

fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

/// Re-nest dot-separated attribute keys into a JSON tree.
/// e.g. `gen_ai.usage.input_tokens = 312` → `{"gen_ai":{"usage":{"input_tokens":312}}}`
fn unflatten_attrs(attrs: &HashMap<String, Value>) -> Value {
    let mut root = serde_json::Map::new();
    for (key, value) in attrs {
        let parts: Vec<&str> = key.split('.').collect();
        insert_nested(&mut root, &parts, value.clone());
    }
    Value::Object(root)
}

fn insert_nested(map: &mut serde_json::Map<String, Value>, parts: &[&str], value: Value) {
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
}

fn span_kind_str(kind: u8) -> &'static str {
    match kind {
        1 => "internal",
        2 => "server",
        3 => "client",
        4 => "producer",
        5 => "consumer",
        _ => "unspecified",
    }
}

fn status_code_str(code: u8) -> &'static str {
    match code {
        1 => "OK",
        2 => "ERROR",
        _ => "UNSET",
    }
}

fn parse_iso_or_default(iso: Option<&str>, default_days_ago: i64) -> DateTime<Utc> {
    iso.and_then(|s| {
        DateTime::parse_from_rfc3339(&s.replace('Z', "+00:00"))
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    })
    .unwrap_or_else(|| Utc::now() - Duration::days(default_days_ago))
}

fn encode_span_id(span_id: &str) -> String {
    base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        format!("Span:{span_id}"),
    )
}

fn encode_trace_id(trace_id: &str) -> String {
    base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        format!("Trace:{trace_id}"),
    )
}

// ─── Span tree builder ────────────────────────────────────────────────────────

fn build_span_tree(
    spans: &[nasiko_observability::Span],
) -> (Vec<SpanNode>, HashMap<String, SpanNode>) {
    let make_node = |s: &nasiko_observability::Span| {
        let (input, output, _) = extract_token_attrs(&s.attributes);
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
        }
    };

    let mut nodes: HashMap<String, SpanNode> = spans
        .iter()
        .map(|s| (s.span_id.clone(), make_node(s)))
        .collect();

    // span_lookup keys are base64-encoded span IDs to match the `id` field
    let snapshot: HashMap<String, SpanNode> = spans
        .iter()
        .map(|s| (encode_span_id(&s.span_id), make_node(s)))
        .collect();

    let mut children_map: HashMap<String, Vec<String>> = HashMap::new();
    for s in spans {
        if let Some(ref parent) = s.parent_span_id
            && nodes.contains_key(parent.as_str())
        {
            children_map
                .entry(parent.clone())
                .or_default()
                .push(s.span_id.clone());
        }
    }

    // Roots: spans whose parent_span_id is None or not in the node map
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

    #[allow(clippy::filter_map_bool_then)]
    let mut root_nodes: Vec<SpanNode> = root_ids
        .iter()
        .filter_map(|id| {
            nodes
                .contains_key(id.as_str())
                .then(|| attach_children(id, &mut nodes, &children_map))
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

// trace/{trace_id}

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
    provider: Arc<dyn ObservabilityProvider>,
    db: PgPool,
    http_client: reqwest::Client,
    config: Arc<Config>,
}

impl ObservabilityService {
    pub fn from_state(state: &crate::state::AppState) -> Self {
        Self {
            provider: state.observability.clone(),
            db: state.db.clone(),
            http_client: state.http_client.clone(),
            config: state.config.clone(),
        }
    }

    /// Returns Vec<(tempo_service_name, display_name)> for all agents in the DB.
    /// The first element is the agent UUID as text — the injector sets
    /// OTEL_SERVICE_NAME to the agent's UUID, so Tempo's `resource.service.name`
    /// is the UUID, never the human-readable name.
    /// OSS: returns all agents (NoopAuthorizer). EE adds RBAC at a higher layer.
    async fn get_agent_names(&self) -> Result<Vec<(String, String)>, ObservabilityError> {
        sqlx::query_as::<_, (String, String)>(
            "SELECT id::text, COALESCE(display_name, name) FROM agents ORDER BY name",
        )
        .fetch_all(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))
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

        for (agent_id, _) in &agents {
            match self.provider.sessions_for_agent(agent_id, start, end).await {
                Ok(sessions) => {
                    all_sessions.extend(sessions.into_iter().map(session_to_summary));
                    successful += 1;
                }
                Err(e) => {
                    tracing::warn!(agent_id, error = %e, "failed to get sessions from Tempo");
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
        let end = Utc::now();
        let start = end - Duration::days(7);
        let details = self.provider.get_session(session_id, start, end).await?;

        let trace_entries: Vec<TraceEntry> = details
            .traces
            .iter()
            .enumerate()
            .map(|(idx, t)| {
                let flat_attrs: serde_json::Map<String, Value> = t
                    .root_span
                    .attributes
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let cursor = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    format!("connection:{idx}"),
                );
                let trace_id_enc = encode_trace_id(&t.trace_id);

                TraceEntry {
                    id: trace_id_enc.clone(),
                    trace_id: t.trace_id.clone(),
                    root_span: RootSpanEntry {
                        id: encode_span_id(&t.root_span.span_id),
                        span_id: t.root_span.span_id.clone(),
                        attributes: serde_json::to_string(&flat_attrs).unwrap_or_default(),
                        cumulative_token_count_total: t.input_tokens + t.output_tokens,
                        latency_ms: round6(t.duration_ms.unwrap_or(0) as f64),
                        start_time: Some(t.root_span.started_at.to_rfc3339()),
                        span_annotations: vec![],
                        span_annotation_summaries: vec![],
                        project: ProjectRef { id: String::new() },
                        input: ContentField {
                            value: t.input_content.clone().unwrap_or_default(),
                            mime_type: "text".into(),
                            parsed_value: None,
                        },
                        output: ContentField {
                            value: t.output_content.clone().unwrap_or_default(),
                            mime_type: "text".into(),
                            parsed_value: None,
                        },
                        trace: TraceRef {
                            id: trace_id_enc,
                            cost_summary: serde_json::json!({
                                "total": { "cost": t.cost.total_usd }
                            }),
                        },
                    },
                    cursor,
                }
            })
            .collect();

        let total_tokens = details.input_tokens + details.output_tokens;
        let end_cursor = trace_entries.last().map(|e| e.cursor.clone());

        Ok(SessionDetailResponse {
            data: SessionDetailData {
                session: SessionDetail {
                    id: details.session_id.clone(),
                    session_id: details.session_id.clone(),
                    num_traces: details.traces.len(),
                    token_usage: TokenUsageSummary { total: Some(total_tokens) },
                    cost_summary: FullCostSummary {
                        total: CostWithTokens {
                            cost: details.cost.total_usd,
                            tokens: total_tokens,
                        },
                        prompt: CostWithTokens {
                            cost: details.cost.prompt_usd,
                            tokens: details.input_tokens,
                        },
                        completion: CostWithTokens {
                            cost: details.cost.completion_usd,
                            tokens: details.output_tokens,
                        },
                    },
                    latency_p50: details.latency_ms_p50,
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
        let trace = self.provider.get_trace(trace_id).await?;

        let (total_input, total_output, model_used) = trace.token_totals();
        let cost = self
            .provider
            .cost(model_used.as_deref(), total_input, total_output)
            .await;

        let trace_latency_ms = match (trace.started_at, trace.ended_at) {
            (Some(s), Some(e)) => Some((e - s).num_milliseconds().max(0) as f64),
            _ => None,
        };

        // Extract session.id from any span that carries it.
        let project_session_id = trace.spans.iter().find_map(|s| {
            s.attributes
                .get("session.id")
                .and_then(|v| v.as_str())
                .map(String::from)
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
                        total: CostOnly { cost: cost.total_usd },
                        prompt: CostOnly { cost: cost.prompt_usd },
                        completion: CostOnly { cost: cost.completion_usd },
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
        let details = self.provider.get_span(trace_id, span_id).await?;
        let span = &details.span;

        let (input_tokens, output_tokens, _) = extract_token_attrs(&span.attributes);

        // Span kind: prefer openinference.span.kind (e.g. "LLM"), fallback to OTel kind
        let span_kind = span
            .attributes
            .get("openinference.span.kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| span_kind_str(span.kind).to_lowercase());

        // Input: prefer span attribute "input.value", fallback to Loki content
        let input_value = span
            .attributes
            .get("input.value")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| details.input_content.clone())
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

        // Output: prefer span attribute "output.value", fallback to Loki content
        let output_value = span
            .attributes
            .get("output.value")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| details.output_content.clone())
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

        let status = status_code_str(span.status_code).to_string();

        Ok(SpanDetailResponse {
            data: SpanDetailData {
                span: SpanDetail {
                    id: encode_span_id(&span.span_id),
                    span_id: span.span_id.clone(),
                    trace: SpanTraceRef {
                        id: encode_trace_id(trace_id),
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
                        total: CostEntry { cost: Some(details.cost.total_usd) },
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
        start_time: Option<&str>,
    ) -> Result<AgentStatsResponse, ObservabilityError> {
        let start = parse_iso_or_default(start_time, 1);
        let stats = self.provider.agent_stats(agent_id, start, Utc::now()).await?;

        Ok(AgentStatsResponse {
            data: AgentStatsData {
                project: AgentProjectStats {
                    id: stats.agent_id,
                    trace_count: stats.trace_count,
                    cost_summary: NestedCostSummary {
                        total: CostOnly { cost: stats.cost.total_usd },
                        prompt: CostOnly { cost: stats.cost.prompt_usd },
                        completion: CostOnly { cost: stats.cost.completion_usd },
                    },
                    latency_ms_p50: stats.latency_ms_p50,
                    latency_ms_p99: stats.latency_ms_p99,
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
        let mut grand_cost = 0f64;
        let mut total_ops = 0usize;
        let mut total_ops_24h = 0usize;
        let mut active = 0usize;

        for (agent_id, agent_name) in &agents {
            let finops = self
                .provider
                .agent_finops(agent_id, start, now)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(agent_id, error = %e, "finops aggregation failed");
                    empty_agent_finops(agent_id)
                });
            let ops_24h = self
                .provider
                .count_user_traces(agent_id, last_24h, now)
                .await
                .unwrap_or(0);

            if finops.operations > 0 {
                active += 1;
            }

            let avg_cost = if finops.operations > 0 {
                round6(finops.cost.total_usd / finops.operations as f64)
            } else {
                0.0
            };

            grand_input += finops.input_tokens;
            grand_output += finops.output_tokens;
            grand_cost += finops.cost.total_usd;
            total_ops += finops.operations;
            total_ops_24h += ops_24h;

            agent_rows.push(AgentFinopsRow {
                agent_id: agent_id.clone(),
                agent_name: agent_name.clone(),
                total_cost: finops.cost.total_usd,
                operations: finops.operations,
                avg_cost_per_operation: avg_cost,
                prompt_tokens: finops.input_tokens,
                completion_tokens: finops.output_tokens,
                total_tokens: finops.input_tokens + finops.output_tokens,
                avg_latency_ms: finops.latency_ms_p50,
                version: None,
            });
        }

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

// ─── Mapping helpers ──────────────────────────────────────────────────────────

fn session_to_summary(s: nasiko_observability::Session) -> SessionSummary {
    let total_tokens = s.input_tokens + s.output_tokens;
    SessionSummary {
        id: s.session_id.clone(),
        session_id: s.session_id,
        agent_id: s.agent_id,
        num_traces: Some(s.trace_ids.len() as u32),
        start_time: s.started_at.map(|t| t.to_rfc3339()),
        end_time: s.ended_at.map(|t| t.to_rfc3339()),
        duration_ms: s.duration_ms,
        first_input: None,
        last_output: None,
        token_usage: TokenUsageSummary {
            total: (total_tokens > 0).then_some(total_tokens),
        },
        trace_latency_ms_p50: s.latency_ms_p50,
        trace_latency_ms_p99: s.latency_ms_p99,
        cost_summary: SimpleCostSummary {
            total: CostEntry {
                cost: (s.cost.total_usd > 0.0).then_some(s.cost.total_usd),
            },
        },
        session_annotations: vec![],
        session_annotation_summaries: vec![],
    }
}

fn empty_agent_finops(agent_id: &str) -> AgentFinOps {
    AgentFinOps {
        agent_id: agent_id.to_string(),
        operations: 0,
        input_tokens: 0,
        output_tokens: 0,
        model_used: None,
        latency_ms_p50: None,
        cost: Default::default(),
    }
}

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
