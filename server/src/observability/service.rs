//! HTTP-shape adapter for the observability endpoints.
//!
//! All trace/log/pricing logic lives in the `nasiko-observability` crate
//! behind [`ObservabilityProvider`]; this module only maps domain types to
//! the JSON response shapes the UI and CLI expect, plus the two pieces that
//! genuinely belong to the server: agent-name resolution (DB) and the
//! FinOps insights LLM call.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use nasiko_config::Config;
use nasiko_observability::{
    AgentFinOps, ObservabilityError, ObservabilityProvider, extract_token_attrs,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::agents::hours_meter;

// ─── Presentation helpers ─────────────────────────────────────────────────────

/// Format a timestamp as RFC 3339 with millisecond precision.
///
/// Tempo stores span timestamps at nanosecond precision; Dart's DateTime.parse
/// only handles up to microseconds. Capping at millis is safe for all consumers.
fn fmt_ts(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
}

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

fn parse_iso(iso: Option<&str>) -> Option<DateTime<Utc>> {
    iso.and_then(|s| {
        DateTime::parse_from_rfc3339(&s.replace('Z', "+00:00"))
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    })
}

/// Strict variant of [`parse_iso`] for query params: an absent value yields
/// `Ok(None)` (caller applies its default), but a present-but-unparseable value
/// is a `BadRequest` rather than a silent fallback. `field` names the offending
/// param in the error so the caller can fix it.
fn parse_iso_param(
    field: &str,
    iso: Option<&str>,
) -> Result<Option<DateTime<Utc>>, ObservabilityError> {
    match iso {
        None | Some("") => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(&s.replace('Z', "+00:00"))
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|_| {
                ObservabilityError::BadRequest(format!(
                    "invalid {field} '{s}': expected RFC 3339 with a timezone, \
                     e.g. 2026-07-23T00:00:00Z"
                ))
            }),
    }
}

fn parse_iso_or_default(iso: Option<&str>, default_days_ago: i64) -> DateTime<Utc> {
    parse_iso(iso).unwrap_or_else(|| Utc::now() - Duration::days(default_days_ago))
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
        let (input, output, model) = extract_token_attrs(&s.attributes);
        SpanNode {
            id: encode_span_id(&s.span_id),
            span_id: s.span_id.clone(),
            name: s.name.clone(),
            span_kind: span_kind_str(s.kind).to_string(),
            status_code: status_code_str(s.status_code).to_string(),
            start_time: Some(fmt_ts(s.started_at)),
            end_time: s.ended_at.map(fmt_ts),
            parent_id: s.parent_span_id.as_deref().map(encode_span_id),
            latency_ms: s.duration_ms.map(|d| d as f64),
            token_count_total: input + output,
            input_tokens: input,
            output_tokens: output,
            model,
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

#[derive(Serialize, ToSchema)]
pub struct SessionListResponse {
    pub data: SessionListData,
}

#[derive(Serialize, ToSchema)]
pub struct SessionListData {
    pub sessions: Vec<SessionSummary>,
    pub total_agents: usize,
    pub successful_agents: usize,
    pub pagination: Pagination,
}

#[derive(Serialize, ToSchema)]
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
    #[schema(value_type = Vec<Object>)]
    pub session_annotations: Vec<Value>,
    #[schema(value_type = Vec<Object>)]
    pub session_annotation_summaries: Vec<Value>,
}

#[derive(Serialize, ToSchema)]
pub struct TokenUsageSummary {
    pub total: Option<u64>,
}

#[derive(Serialize, ToSchema)]
pub struct SimpleCostSummary {
    pub total: CostEntry,
}

#[derive(Serialize, ToSchema)]
pub struct CostEntry {
    pub cost: Option<f64>,
}

#[derive(Serialize, Clone, ToSchema)]
pub struct Pagination {
    pub end_cursor: Option<String>,
    pub has_next_page: bool,
}

// session/{session_id}

#[derive(Serialize, ToSchema)]
pub struct SessionDetailResponse {
    pub data: SessionDetailData,
}

#[derive(Serialize, ToSchema)]
pub struct SessionDetailData {
    pub session: SessionDetail,
}

#[derive(Serialize, ToSchema)]
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

#[derive(Serialize, ToSchema)]
pub struct FullCostSummary {
    pub total: CostWithTokens,
    pub prompt: CostWithTokens,
    pub completion: CostWithTokens,
}

#[derive(Serialize, ToSchema)]
pub struct CostWithTokens {
    pub cost: f64,
    pub tokens: u64,
}

#[derive(Serialize, ToSchema)]
pub struct TraceEntry {
    pub id: String,
    pub trace_id: String,
    pub root_span: RootSpanEntry,
    pub cursor: String,
}

#[derive(Serialize, ToSchema)]
pub struct RootSpanEntry {
    pub id: String,
    pub span_id: String,
    pub attributes: String,
    pub cumulative_token_count_total: u64,
    pub latency_ms: f64,
    pub start_time: Option<String>,
    #[schema(value_type = Vec<Object>)]
    pub span_annotations: Vec<Value>,
    #[schema(value_type = Vec<Object>)]
    pub span_annotation_summaries: Vec<Value>,
    pub project: ProjectRef,
    pub input: ContentField,
    pub output: ContentField,
    pub trace: TraceRef,
}

#[derive(Serialize, ToSchema)]
pub struct ProjectRef {
    pub id: String,
}

#[derive(Serialize, ToSchema)]
pub struct ContentField {
    pub value: String,
    pub mime_type: String,
    #[schema(value_type = Option<Object>)]
    pub parsed_value: Option<Value>,
}

#[derive(Serialize, ToSchema)]
pub struct TraceRef {
    pub id: String,
    #[schema(value_type = Object)]
    pub cost_summary: Value,
}

// trace/{trace_id}

#[derive(Serialize, ToSchema)]
pub struct TraceDetailResponse {
    pub data: TraceDetailData,
}

#[derive(Serialize, ToSchema)]
pub struct TraceDetailData {
    pub trace: TraceDetail,
}

#[derive(Serialize, ToSchema)]
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

#[derive(Serialize, ToSchema)]
pub struct NestedCostSummary {
    pub total: CostOnly,
    pub prompt: CostOnly,
    pub completion: CostOnly,
}

#[derive(Serialize, ToSchema)]
pub struct CostOnly {
    pub cost: f64,
}

#[derive(Serialize, ToSchema)]
pub struct RootSpansWrapper {
    pub edges: Vec<RootSpanEdge>,
}

#[derive(Serialize, ToSchema)]
pub struct RootSpanEdge {
    pub span: RootSpanRef,
}

#[derive(Serialize, ToSchema)]
pub struct RootSpanRef {
    pub id: String,
    pub span_id: String,
    pub parent_id: Option<String>,
    pub status_code: String,
}

#[derive(Serialize, Clone, ToSchema)]
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
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model: Option<String>,
    #[schema(value_type = Vec<Object>)]
    pub span_annotation_summaries: Vec<Value>,
    // `no_recursion`: self-referential — without it utoipa's schema builder
    // recurses infinitely and overflows the stack at startup.
    #[schema(no_recursion)]
    pub children: Vec<SpanNode>,
}

// span/{trace_id}/{span_id}

#[derive(Serialize, ToSchema)]
pub struct SpanDetailResponse {
    pub data: SpanDetailData,
}

#[derive(Serialize, ToSchema)]
pub struct SpanDetailData {
    pub span: SpanDetail,
}

#[derive(Serialize, ToSchema)]
pub struct SpanTraceRef {
    pub id: String,
    pub trace_id: String,
}

#[derive(Serialize, ToSchema)]
pub struct SpanProjectRef {
    pub id: String,
    #[schema(value_type = Object)]
    pub annotation_configs: Value,
}

#[derive(Serialize, ToSchema)]
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
    #[schema(value_type = Object)]
    pub attributes: Value,
    #[schema(value_type = Vec<Object>)]
    pub events: Vec<Value>,
    #[schema(value_type = Vec<Object>)]
    pub span_annotations: Vec<Value>,
    #[schema(value_type = Vec<Object>)]
    pub span_annotation_summaries: Vec<Value>,
    #[schema(value_type = Vec<Object>)]
    pub document_retrieval_metrics: Vec<Value>,
    #[schema(value_type = Vec<Object>)]
    pub document_evaluations: Vec<Value>,
    pub project: SpanProjectRef,
}

// agent/{agent_id}/stats

#[derive(Serialize, ToSchema)]
pub struct AgentStatsResponse {
    pub data: AgentStatsData,
}

#[derive(Serialize, ToSchema)]
pub struct AgentStatsData {
    pub project: AgentProjectStats,
}

#[derive(Serialize, ToSchema)]
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

#[derive(Serialize, ToSchema)]
pub struct FinopsDashboardResponse {
    pub data: FinopsDashboardData,
    pub status_code: u16,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
pub struct FinopsDashboardData {
    pub summary: FinopsSummary,
    pub agents: Vec<AgentFinopsRow>,
    pub token_usage: FinopsTokenUsage,
}

#[derive(Serialize, ToSchema)]
pub struct FinopsSummary {
    pub total_cost: f64,
    pub total_operations: usize,
    pub operations_last_24h: usize,
    pub average_cost: f64,
    pub active_agents: usize,
    pub total_agents: usize,
    /// Replica-hours consumed in the dashboard window — includes agents that
    /// have since been deleted (their sessions survive deletion).
    pub total_container_hours: f64,
}

#[derive(Serialize, ToSchema)]
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
    /// Replica-hours this agent consumed in the dashboard window.
    pub container_hours: f64,
}

#[derive(Serialize, ToSchema)]
pub struct FinopsTokenUsage {
    pub total_tokens: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub avg_tokens_per_operation: u64,
}

// finops/insights

#[derive(Deserialize, ToSchema)]
pub struct InsightsRequest {
    #[schema(value_type = Object)]
    pub kpi: Value,
    #[schema(value_type = Vec<Object>)]
    pub agent_costs: Vec<Value>,
}

#[derive(Serialize, ToSchema)]
pub struct InsightsResponse {
    pub insights: Vec<String>,
}

// finops/agent-hours

#[derive(Serialize, ToSchema)]
pub struct AgentHoursResponse {
    pub data: AgentHoursData,
    pub status_code: u16,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
pub struct AgentHoursData {
    /// Replica-hours across all listed agents within the window.
    pub total_hours: f64,
    pub window: AgentHoursWindow,
    /// Sessions-derived rows — includes agents that have since been deleted.
    pub agents: Vec<AgentHoursRow>,
    /// Time series, present only when the `bucket` query param is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buckets: Option<Vec<AgentHoursBucket>>,
}

#[derive(Serialize, ToSchema)]
pub struct AgentHoursWindow {
    pub start: String,
    pub end: String,
}

#[derive(Serialize, ToSchema)]
pub struct AgentHoursRow {
    pub agent_id: String,
    pub agent_name: String,
    pub hours: f64,
    /// Replicas live right now among the sessions that overlapped this window.
    pub live_replicas: i64,
    pub deleted: bool,
}

#[derive(Serialize, ToSchema)]
pub struct AgentHoursBucket {
    pub start: String,
    pub total_hours: f64,
    /// Per-agent breakdown for this bucket (only agents with hours in it).
    pub agents: Vec<AgentHoursBucketAgent>,
}

#[derive(Serialize, ToSchema)]
pub struct AgentHoursBucketAgent {
    pub agent_id: String,
    pub agent_name: String,
    pub hours: f64,
    pub deleted: bool,
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

    /// Returns Vec<(id, name, display_name)> for all agents in the DB. `name`
    /// doubles as the Tempo `service.name` (the injector sets OTEL_SERVICE_NAME
    /// to the agent name); `id` is the UUID reported to callers.
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

    // ── 1. session/list ──────────────────────────────────────────────────────

    pub async fn get_all_sessions(
        &self,
        user_id: &str,
        _role: Option<&str>,
        _department_id: Option<&str>,
        _team_id: Option<&str>,
        start_time: Option<&str>,
        is_superuser: bool,
    ) -> Result<SessionListResponse, ObservabilityError> {
        let start = parse_iso_or_default(start_time, 7);
        let end = Utc::now();

        // 1. Query chat_sessions as the authoritative source — every session
        //    shows up here regardless of whether the agent is OTel-instrumented.
        //    Non-superusers only see their own sessions.
        let db_sessions: Vec<(String, Option<uuid::Uuid>, DateTime<Utc>)> = if is_superuser {
            sqlx::query_as(
                "SELECT session_id, agent_id, created_at \
                 FROM chat_sessions \
                 WHERE deleted_at IS NULL AND created_at >= $1 \
                 ORDER BY created_at DESC LIMIT 500",
            )
            .bind(start)
            .fetch_all(&self.db)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?
        } else {
            let caller_uuid: uuid::Uuid = user_id
                .parse()
                .map_err(|_| ObservabilityError::Internal("invalid user id in claims".into()))?;
            sqlx::query_as(
                "SELECT session_id, agent_id, created_at \
                 FROM chat_sessions \
                 WHERE user_id = $1 AND deleted_at IS NULL AND created_at >= $2 \
                 ORDER BY created_at DESC LIMIT 500",
            )
            .bind(caller_uuid)
            .bind(start)
            .fetch_all(&self.db)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?
        };

        // 2. Build agent_id → name lookup for the agent_id column.
        let agents = self.get_agent_names().await.unwrap_or_default();
        let total = agents.len();
        let agent_name_by_id: std::collections::HashMap<uuid::Uuid, String> = agents
            .into_iter()
            .map(|(id, name, _display)| (id, name))
            .collect();

        // 3. Enrich each DB session from Tempo (by session_id). For agents
        //    without OTel, Tempo returns NotFound and we fall back to a minimal
        //    summary built from the DB row — the session still appears in the UI.
        let mut all_sessions: Vec<SessionSummary> = Vec::new();
        let mut successful = 0usize;

        for (session_id, agent_id_opt, created_at) in db_sessions {
            let agent_name = agent_id_opt
                .and_then(|id| agent_name_by_id.get(&id))
                .cloned()
                .unwrap_or_default();

            match self.provider.get_session(&session_id, start, end).await {
                Ok(details) => {
                    let total_tokens = details.input_tokens + details.output_tokens;
                    let started_at = details.traces.iter().map(|t| t.root_span.started_at).min();
                    let ended_at = details
                        .traces
                        .iter()
                        .filter_map(|t| t.root_span.ended_at)
                        .max();
                    let duration_ms = started_at
                        .zip(ended_at)
                        .map(|(s, e)| (e - s).num_milliseconds().max(0) as u64);
                    all_sessions.push(SessionSummary {
                        id: session_id.clone(),
                        session_id,
                        agent_id: agent_name,
                        num_traces: Some(details.traces.len() as u32),
                        start_time: started_at.map(fmt_ts),
                        // Flutter's DateTime.parse requires a non-empty string —
                        // fall back to start_time when no end time is known.
                        end_time: ended_at.or(started_at).map(fmt_ts),
                        duration_ms,
                        first_input: details.traces.first().and_then(|t| t.input_content.clone()),
                        last_output: details.traces.last().and_then(|t| t.output_content.clone()),
                        token_usage: TokenUsageSummary {
                            total: (total_tokens > 0).then_some(total_tokens),
                        },
                        trace_latency_ms_p50: details.latency_ms_p50,
                        trace_latency_ms_p99: None,
                        cost_summary: SimpleCostSummary {
                            total: CostEntry {
                                cost: (details.cost.total_usd > 0.0)
                                    .then_some(details.cost.total_usd),
                            },
                        },
                        session_annotations: vec![],
                        session_annotation_summaries: vec![],
                    });
                    successful += 1;
                }
                Err(e) => {
                    // Not found in Tempo (agent not OTel-instrumented) or a
                    // transient error — surface the session from DB metadata so
                    // it still appears in the execution history.
                    if !matches!(e, ObservabilityError::NotFound(_)) {
                        tracing::warn!(session_id, error = %e, "tempo lookup failed for session");
                    }
                    all_sessions.push(SessionSummary {
                        id: session_id.clone(),
                        session_id,
                        agent_id: agent_name,
                        num_traces: None,
                        start_time: Some(fmt_ts(created_at)),
                        end_time: Some(fmt_ts(created_at)),
                        duration_ms: None,
                        first_input: None,
                        last_output: None,
                        token_usage: TokenUsageSummary { total: None },
                        trace_latency_ms_p50: None,
                        trace_latency_ms_p99: None,
                        cost_summary: SimpleCostSummary {
                            total: CostEntry { cost: None },
                        },
                        session_annotations: vec![],
                        session_annotation_summaries: vec![],
                    });
                }
            }
        }

        Ok(SessionListResponse {
            data: SessionListData {
                sessions: all_sessions,
                total_agents: total,
                successful_agents: successful,
                pagination: Pagination {
                    end_cursor: None,
                    has_next_page: false,
                },
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
                        start_time: Some(fmt_ts(t.root_span.started_at)),
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
                    token_usage: TokenUsageSummary {
                        total: Some(total_tokens),
                    },
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
                    pagination: Pagination {
                        end_cursor,
                        has_next_page: false,
                    },
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
                    parent_id: s.parent_id.clone(),
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
                        total: CostOnly {
                            cost: cost.total_usd,
                        },
                        prompt: CostOnly {
                            cost: cost.prompt_usd,
                        },
                        completion: CostOnly {
                            cost: cost.completion_usd,
                        },
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
                    start_time: Some(fmt_ts(span.started_at)),
                    end_time: span.ended_at.map(fmt_ts),
                    parent_id: span.parent_span_id.clone(),
                    latency_ms: span.duration_ms.map(|d| d as f64),
                    token_count_total: input_tokens + output_tokens,
                    cost_summary: SimpleCostSummary {
                        total: CostEntry {
                            cost: Some(details.cost.total_usd),
                        },
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
        let stats = self
            .provider
            .agent_stats(agent_id, start, Utc::now())
            .await?;

        Ok(AgentStatsResponse {
            data: AgentStatsData {
                project: AgentProjectStats {
                    id: stats.agent_id,
                    trace_count: stats.trace_count,
                    cost_summary: NestedCostSummary {
                        total: CostOnly {
                            cost: stats.cost.total_usd,
                        },
                        prompt: CostOnly {
                            cost: stats.cost.prompt_usd,
                        },
                        completion: CostOnly {
                            cost: stats.cost.completion_usd,
                        },
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

        let now = Utc::now();
        let start = parse_iso_or_default(start_time, 30);
        let last_24h = now - Duration::hours(24);

        // Container-hours for the same window, one batched query. Includes
        // agents that have since been deleted, so the summary total stays
        // honest even when the per-agent rows below can't show them.
        // Fail-soft, matching the per-agent finops calls.
        let hours_rows = hours_meter::windowed_agent_hours(&self.db, start, now, None)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "container hours aggregation failed");
                vec![]
            });
        let total_container_hours = round6(hours_rows.iter().map(|r| r.hours).sum());
        let hours_by_agent: HashMap<uuid::Uuid, f64> =
            hours_rows.iter().map(|r| (r.agent_id, r.hours)).collect();

        if agents.is_empty() {
            return Ok(empty_finops_response(total_container_hours));
        }

        let mut agent_rows: Vec<AgentFinopsRow> = Vec::new();
        let mut grand_input = 0u64;
        let mut grand_output = 0u64;
        let mut grand_cost = 0f64;
        let mut total_ops = 0usize;
        let mut total_ops_24h = 0usize;
        let mut active = 0usize;

        for (agent_uuid, agent_name, display_name) in &agents {
            let finops = self
                .provider
                .agent_finops(agent_name, start, now)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(agent_name, error = %e, "finops aggregation failed");
                    empty_agent_finops(agent_name)
                });
            let ops_24h = self
                .provider
                .count_user_traces(agent_name, last_24h, now)
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
                agent_id: agent_uuid.to_string(),
                agent_name: display_name.clone(),
                total_cost: finops.cost.total_usd,
                operations: finops.operations,
                avg_cost_per_operation: avg_cost,
                prompt_tokens: finops.input_tokens,
                completion_tokens: finops.output_tokens,
                total_tokens: finops.input_tokens + finops.output_tokens,
                avg_latency_ms: finops.latency_ms_p50,
                version: None,
                container_hours: round6(hours_by_agent.get(agent_uuid).copied().unwrap_or(0.0)),
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
                    total_container_hours,
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

    // ── 8. finops/agent-hours ─────────────────────────────────────────────────

    /// Windowed replica-hours from `agent_instance_sessions` — the metering
    /// source of truth the external billing system reads. Deleted agents are
    /// included (rows have no FK to `agents`); `bucket` adds an hourly/daily
    /// series whose sum equals `total_hours` (additivity).
    pub async fn get_agent_hours(
        &self,
        start_time: Option<&str>,
        end_time: Option<&str>,
        agent_id: Option<&str>,
        bucket: Option<&str>,
    ) -> Result<AgentHoursResponse, ObservabilityError> {
        /// Hard cap on series length so a caller can't request an unbounded
        /// (e.g. epoch-to-now hourly) response.
        const MAX_SERIES_BUCKETS: i64 = 1000;

        let bucket = bucket.and_then(hours_meter::HoursBucket::parse);

        // Reject a present-but-unparseable time param with 400 rather than
        // silently falling back to a default window — on a billing endpoint a
        // mistyped timestamp must never quietly return the wrong range. An
        // absent param (None) still uses the documented default below.
        let end = parse_iso_param("end_time", end_time)?.unwrap_or_else(Utc::now);
        // No start_time means all-time for the plain report (this endpoint is
        // the billing source of truth — silently dropping history would be
        // wrong), but 30 days for a series (an epoch-to-now series is
        // unbounded and gets capped below anyway).
        let mut start =
            parse_iso_param("start_time", start_time)?.unwrap_or_else(|| match bucket {
                Some(_) => end - Duration::days(30),
                None => DateTime::<Utc>::UNIX_EPOCH,
            });
        if let Some(b) = bucket {
            let max_span = Duration::seconds(b.seconds() * MAX_SERIES_BUCKETS);
            if end - start > max_span {
                tracing::warn!(
                    requested_start = %start,
                    clamped_start = %(end - max_span),
                    "agent-hours series window clamped to {MAX_SERIES_BUCKETS} buckets"
                );
                start = end - max_span;
            }
        }

        // A malformed agent_id is rejected with 400 — silently returning an
        // empty (zero-hours) result for a mistyped UUID would read as "this
        // agent used nothing", another way to skew a bill.
        let agent_filter = match agent_id {
            Some(raw) => Some(uuid::Uuid::parse_str(raw).map_err(|_| {
                ObservabilityError::BadRequest(format!("invalid agent_id '{raw}': expected a UUID"))
            })?),
            None => None,
        };

        if start >= end {
            return Ok(empty_agent_hours_response(start, end, bucket.is_some()));
        }

        let rows = hours_meter::windowed_agent_hours(&self.db, start, end, agent_filter)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        let total_hours = round6(rows.iter().map(|r| r.hours).sum());
        let agents = rows
            .into_iter()
            .map(|r| AgentHoursRow {
                agent_id: r.agent_id.to_string(),
                agent_name: r.agent_name,
                hours: round6(r.hours),
                live_replicas: r.live_replicas,
                deleted: r.deleted,
            })
            .collect();

        let buckets = match bucket {
            Some(b) => {
                // Canonical bucket timeline (includes idle buckets at 0.0) +
                // per-agent breakdown (only non-empty (bucket, agent) cells),
                // stitched together keyed by bucket_start (both come from the
                // same generate_series, so the timestamps match exactly).
                let totals =
                    hours_meter::windowed_hours_series(&self.db, start, end, b, agent_filter)
                        .await
                        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;
                let per_agent = hours_meter::windowed_hours_series_by_agent(
                    &self.db,
                    start,
                    end,
                    b,
                    agent_filter,
                )
                .await
                .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

                let mut by_bucket: HashMap<DateTime<Utc>, Vec<AgentHoursBucketAgent>> =
                    HashMap::new();
                for r in per_agent {
                    by_bucket
                        .entry(r.bucket_start)
                        .or_default()
                        .push(AgentHoursBucketAgent {
                            agent_id: r.agent_id.to_string(),
                            agent_name: r.agent_name,
                            hours: round6(r.hours),
                            deleted: r.deleted,
                        });
                }

                Some(
                    totals
                        .into_iter()
                        .map(|row| AgentHoursBucket {
                            start: fmt_ts(row.bucket_start),
                            total_hours: round6(row.hours),
                            agents: by_bucket.remove(&row.bucket_start).unwrap_or_default(),
                        })
                        .collect(),
                )
            }
            None => None,
        };

        Ok(AgentHoursResponse {
            data: AgentHoursData {
                total_hours,
                window: AgentHoursWindow {
                    start: fmt_ts(start),
                    end: fmt_ts(end),
                },
                agents,
                buckets,
            },
            status_code: 200,
            message: "Agent hours retrieved successfully".into(),
        })
    }
}

fn empty_agent_hours_response(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    with_buckets: bool,
) -> AgentHoursResponse {
    AgentHoursResponse {
        data: AgentHoursData {
            total_hours: 0.0,
            window: AgentHoursWindow {
                start: fmt_ts(start),
                end: fmt_ts(end),
            },
            agents: vec![],
            buckets: with_buckets.then(Vec::new),
        },
        status_code: 200,
        message: "Agent hours retrieved successfully".into(),
    }
}

// ─── Mapping helpers ──────────────────────────────────────────────────────────

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

/// `total_container_hours` is threaded in rather than zeroed: a deployment
/// whose agents were all hard-deleted still has billable session history.
fn empty_finops_response(total_container_hours: f64) -> FinopsDashboardResponse {
    FinopsDashboardResponse {
        data: FinopsDashboardData {
            summary: FinopsSummary {
                total_cost: 0.0,
                total_operations: 0,
                operations_last_24h: 0,
                average_cost: 0.0,
                active_agents: 0,
                total_agents: 0,
                total_container_hours,
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
