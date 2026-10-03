//! HTTP-shape adapter for the observability endpoints.
//!
//! All trace/log/pricing logic lives in the `nasiko-observability` crate
//! behind [`ObservabilityProvider`]; this module only maps domain types to
//! the JSON response shapes the UI and CLI expect, plus the two pieces that
//! genuinely belong to the server: agent-name resolution (DB) and the
//! FinOps insights LLM call.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, SecondsFormat, TimeZone, Utc};
use futures::stream::{self, StreamExt};
use nasiko_config::Config;
use nasiko_observability::{
    CostBreakdown, ObservabilityError, ObservabilityProvider, TimeBucket, extract_token_attrs,
    extract_usage_attrs,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::agents::hours_meter;

// ─── Session listing tuning ───────────────────────────────────────────────────

/// Sessions per page when the caller doesn't ask for a size. Every row costs a
/// trace-store lookup, so this is a work bound, not just a display preference.
const DEFAULT_SESSION_PAGE: i64 = 25;

/// How many trace-store lookups may be in flight while enriching one page.
/// High enough that a full page resolves in a couple of round-trips, low enough
/// not to stampede Tempo when several users load the page at once.
const SESSION_ENRICH_CONCURRENCY: usize = 8;

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

pub(super) fn parse_iso_or_default(iso: Option<&str>, default_days_ago: i64) -> DateTime<Utc> {
    parse_iso(iso).unwrap_or_else(|| Utc::now() - Duration::days(default_days_ago))
}

fn session_trace_window(
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    (
        created_at - Duration::minutes(5),
        updated_at + Duration::minutes(5),
    )
}

/// Hours covered by a "24h" | "7d" | "30d" quick-range value, or `None` for
/// anything else (unknown values are the caller's responsibility to reject).
fn range_hours(range: &str) -> Option<i64> {
    match range {
        "24h" => Some(24),
        "7d" => Some(24 * 7),
        "30d" => Some(24 * 30),
        _ => None,
    }
}

fn bucket_label(bucket: TimeBucket) -> &'static str {
    match bucket {
        TimeBucket::Hour => "hour",
        TimeBucket::Day => "day",
    }
}

/// Resolves `(start, end, bucket)` for the time-series/heatmap endpoints.
/// `range` (24h/7d/30d), when present, wins over `start_time`/`end_time` —
/// same precedence as the quick-select UI. `end_time` still overrides "now"
/// when both are given (matches the month-picker semantics elsewhere in this
/// module). Hour granularity for 24h, day granularity otherwise.
fn resolve_window(
    start_time: Option<&str>,
    end_time: Option<&str>,
    range: Option<&str>,
) -> Result<(DateTime<Utc>, DateTime<Utc>, TimeBucket), ObservabilityError> {
    let end = end_time
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    if let Some(r) = range {
        let hours = range_hours(r)
            .ok_or_else(|| ObservabilityError::BadRequest(format!("invalid range '{r}'")))?;
        let bucket = if r == "24h" {
            TimeBucket::Hour
        } else {
            TimeBucket::Day
        };
        return Ok((end - Duration::hours(hours), end, bucket));
    }

    let start = parse_iso_or_default(start_time, 30);
    Ok((start, end, TimeBucket::Day))
}

fn sort_agent_rows(rows: &mut [AgentFinopsRow], sort_by: Option<&str>, desc: bool) {
    match sort_by.unwrap_or("cost") {
        "tokens" => rows.sort_by_key(|a| a.total_tokens),
        "operations" => rows.sort_by_key(|a| a.operations),
        "avg_latency" => rows.sort_by(|a, b| {
            a.avg_latency_ms
                .unwrap_or(0.0)
                .total_cmp(&b.avg_latency_ms.unwrap_or(0.0))
        }),
        "container_hours" => rows.sort_by(|a, b| a.container_hours.total_cmp(&b.container_hours)),
        "name" => rows.sort_by(|a, b| a.agent_name.cmp(&b.agent_name)),
        _ => rows.sort_by(|a, b| a.total_cost.total_cmp(&b.total_cost)),
    }
    if desc {
        rows.reverse();
    }
}

fn sort_workflow_rows(rows: &mut [WorkflowFinopsRow], sort_by: Option<&str>, desc: bool) {
    match sort_by.unwrap_or("cost") {
        "tokens" => rows.sort_by_key(|a| a.total_tokens),
        "operations" => rows.sort_by_key(|a| a.executions),
        "avg_latency" => rows.sort_by(|a, b| {
            a.avg_latency_ms
                .unwrap_or(0.0)
                .total_cmp(&b.avg_latency_ms.unwrap_or(0.0))
        }),
        "name" => rows.sort_by(|a, b| a.workflow_name.cmp(&b.workflow_name)),
        _ => rows.sort_by(|a, b| a.total_cost.total_cmp(&b.total_cost)),
    }
    if desc {
        rows.reverse();
    }
}

fn paginate<T>(rows: Vec<T>, limit: Option<i64>, offset: Option<i64>) -> Vec<T> {
    let offset = offset.unwrap_or(0).max(0) as usize;
    let rows: Vec<T> = rows.into_iter().skip(offset).collect();
    match limit {
        Some(l) if l >= 0 => rows.into_iter().take(l as usize).collect(),
        _ => rows,
    }
}

/// First present string attribute out of `keys`, in order. A fallback chain
/// rather than a semconv-version check — the same shape the token extractors
/// use, because agents in one fleet rarely run one instrumentation version.
fn first_str_attr(attrs: &HashMap<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| attrs.get(*k))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
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

fn span_display_name(span: &nasiko_observability::Span) -> String {
    let operation = span
        .attributes
        .get("gen_ai.operation.name")
        .and_then(Value::as_str);
    if operation != Some("execute_tool") {
        return span.name.clone();
    }
    let tool = span
        .attributes
        .get("tool.name")
        .and_then(Value::as_str)
        .unwrap_or("tool");
    match tool_argument_summary(&span.attributes) {
        Some(summary) => format!("{tool}: {summary}"),
        None => tool.to_string(),
    }
}

fn tool_argument_summary(attributes: &HashMap<String, Value>) -> Option<String> {
    let raw = attributes.get("tool.arguments")?.as_str()?;
    let parsed: Value = serde_json::from_str(raw).ok()?;
    let value = if let Some(object) = parsed.as_object() {
        [
            "command",
            "cmd",
            "path",
            "file_path",
            "query",
            "url",
            "pattern",
            "description",
        ]
        .iter()
        .find_map(|key| object.get(*key))?
    } else {
        &parsed
    };
    let summary = value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string());
    let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.is_empty() {
        return None;
    }
    Some(if summary.chars().count() > 72 {
        summary.chars().take(69).collect::<String>() + "..."
    } else {
        summary
    })
}

// ─── Span tree builder ────────────────────────────────────────────────────────

fn build_span_tree(
    spans: &[nasiko_observability::Span],
) -> (Vec<SpanNode>, HashMap<String, SpanNode>) {
    let mut seen = HashSet::new();
    let mut trace_usage = (0u64, 0u64, 0u64, 0u64, None);
    for span in spans {
        if !seen.insert(&span.span_id) {
            continue;
        }
        let u = extract_usage_attrs(&span.attributes);
        let (input, output, model) = (u.input, u.output, u.model.clone());
        let (cache_read, cache_creation) = (u.cache_read, u.cache_creation);
        trace_usage.0 += input;
        trace_usage.1 += output;
        trace_usage.2 += cache_read;
        trace_usage.3 += cache_creation;
        if trace_usage.4.is_none()
            && (input > 0 || output > 0 || cache_read > 0 || cache_creation > 0)
        {
            trace_usage.4 = model;
        }
    }

    let make_node = |s: &nasiko_observability::Span| {
        let (input, output, model, cache_read, cache_creation) = if s.name == "coding_agent.turn" {
            (
                trace_usage.0,
                trace_usage.1,
                trace_usage.4.clone(),
                trace_usage.2,
                trace_usage.3,
            )
        } else {
            let u = extract_usage_attrs(&s.attributes);
            (u.input, u.output, u.model, u.cache_read, u.cache_creation)
        };
        SpanNode {
            id: encode_span_id(&s.span_id),
            span_id: s.span_id.clone(),
            name: span_display_name(s),
            span_kind: span_kind_str(s.kind).to_string(),
            status_code: status_code_str(s.status_code).to_string(),
            start_time: Some(fmt_ts(s.started_at)),
            end_time: s.ended_at.map(fmt_ts),
            parent_id: s.parent_span_id.as_deref().map(encode_span_id),
            latency_ms: s.duration_ms.map(|d| d as f64),
            token_count_total: input + output + cache_read + cache_creation,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_creation,
            model,
            operation: first_str_attr(
                &s.attributes,
                &["gen_ai.operation.name", "rpc.method", "code.function"],
            ),
            provider: first_str_attr(&s.attributes, &["gen_ai.system", "gen_ai.provider.name"]),
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
    /// LLM-derived session name from `chat_sessions`. `None` for sessions that
    /// never went through chat (CLI / direct A2A), where the id is the heading.
    pub title: Option<String>,
    pub agent_name: Option<String>,
    pub num_traces: usize,
    pub token_usage: TokenUsageSummary,
    pub cost_summary: FullCostSummary,
    pub latency_p50: Option<f64>,
    /// The session page renders this KPI (`s.latency_p99`); it was silently
    /// `0.0 s` for every session while the field didn't exist in the response.
    pub latency_p99: Option<f64>,
    /// Mean trace duration, which is what the KPI strip labels "Avg latency".
    pub latency_avg: Option<f64>,
    /// Prompt tokens served from / written to provider cache, over the session.
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub metrics_complete: bool,
    pub traces: Vec<TraceEntry>,
    pub pagination: Pagination,
}

#[derive(Serialize, ToSchema)]
pub struct FullCostSummary {
    pub total: CostWithTokens,
    pub prompt: CostWithTokens,
    pub completion: CostWithTokens,
    pub cache_read: CostWithTokens,
    pub cache_creation: CostWithTokens,
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
    /// Per-turn token split. `cumulative_token_count_total` is `input+output`;
    /// the cache counts are tracked separately and are not folded into it.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
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
    pub cache_read: CostOnly,
    pub cache_creation: CostOnly,
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
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub model: Option<String>,
    /// The greyed second label on a span row (`chat`, `git.clone`, …).
    /// `None` when no attribute names one — the UI must not synthesise it
    /// from the span name.
    pub operation: Option<String>,
    /// GenAI provider (`openai`, `anthropic`, …), for the row's glyph.
    pub provider: Option<String>,
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
    /// GenAI provider and model, promoted out of the nested `attributes` blob
    /// so the detail pane reads one field instead of walking a semconv tree.
    pub provider: Option<String>,
    pub model: Option<String>,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    /// Carries the prompt/completion split the Usage tab renders; the service
    /// already computed it and used to sum it away before serializing.
    pub cost_summary: FullCostSummary,
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
    pub status_code: u16,
    pub message: String,
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
    pub kpis: FinopsKpis,
    pub attributions: FinopsAttributions,
    /// Pre-computed spend split for the selected window: top-5 agents by spend
    /// plus an "Others" catchall, ready to feed a pie/donut chart without any
    /// client-side aggregation.
    pub spend_by_agent: SpendByAgentBreakdown,
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
    /// Calls in the window that consumed tokens but carry no cost (no price row
    /// for the model — e.g. a custom provider with no price book). `SUM(cost_usd)`
    /// silently skips these, so `total_cost` under-reports; this surfaces the gap
    /// as a known number rather than a smaller one.
    pub unpriced_calls: usize,
    /// Sum of rows explicitly priced using inferred rates or usage evidence.
    pub estimated_cost: f64,
    /// Older materializations without recorded pricing confidence.
    pub unknown_confidence_calls: usize,
}

#[derive(Serialize, Clone, ToSchema)]
pub struct AgentFinopsRow {
    pub agent_id: String,
    pub agent_name: String,
    pub total_cost: f64,
    pub operations: usize,

    /// True when `operations` was capped by the token-aggregation trace
    /// limit — the token/cost fields below undercount the real total.
    pub is_capped: bool,
    pub avg_cost_per_operation: f64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// Prompt tokens served from provider cache (OpenAI cached / Anthropic cache read).
    pub cache_read_tokens: u64,
    /// Prompt tokens written to provider cache (Anthropic cache creation).
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
    /// p50 trace-level latency for this agent in the window.
    pub avg_latency_ms: Option<f64>,
    pub avg_latency_p95_ms: Option<f64>,
    pub avg_latency_p99_ms: Option<f64>,
    pub tool_call_count: u64,
    pub version: Option<String>,
    /// Replica-hours this agent consumed in the dashboard window.
    pub container_hours: f64,
}

#[derive(Serialize, ToSchema)]
pub struct FinopsTokenUsage {
    pub total_tokens: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// Prompt tokens served from provider cache (OpenAI cached / Anthropic cache read).
    pub cache_read_tokens: u64,
    /// Prompt tokens written to provider cache (Anthropic cache creation).
    pub cache_creation_tokens: u64,
    pub avg_tokens_per_operation: u64,
}

// ─── KPI %-change ──────────────────────────────────────────────────────────────

#[derive(Serialize, ToSchema)]
pub struct KpiValue {
    pub current: f64,
    pub previous: f64,
    /// `(current - previous) / previous * 100`, rounded to 2dp. `None` when
    /// `previous == 0` — an undefined percentage, not a fabricated 0 or ∞.
    pub change_pct: Option<f64>,
}

impl KpiValue {
    fn new(current: f64, previous: f64) -> Self {
        let change_pct = if previous == 0.0 {
            None
        } else {
            Some(((current - previous) / previous * 100.0 * 100.0).round() / 100.0)
        };
        Self {
            current,
            previous,
            change_pct,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct FinopsKpis {
    pub total_spend: KpiValue,
    pub total_tokens: KpiValue,
    pub cost_per_operation: KpiValue,
    /// Fleet-wide p50 latency (label kept for UI backward compat).
    pub avg_latency_ms: KpiValue,
    pub total_agents: KpiValue,
    pub active_agents: KpiValue,
    pub total_operations: KpiValue,
    pub total_tool_calls: KpiValue,
    pub latency_p95_ms: KpiValue,
    pub latency_p99_ms: KpiValue,
}

// ─── Spend over time ────────────────────────────────────────────────────────────

#[derive(Serialize, ToSchema)]
pub struct SpendTimeseriesPoint {
    /// Bucket start, RFC3339.
    pub bucket_start: String,
    pub spend_usd: f64,
    pub operations: usize,
    /// Total tool calls across all agents in this bucket.
    pub tool_calls: u64,
    /// Highest-spend agent in this bucket, for the hover breakdown.
    pub top_agent_name: Option<String>,
    pub top_agent_spend_usd: Option<f64>,
    /// Fleet-wide latency percentiles across all traces in this bucket.
    /// `None` when no traces carry a latency measurement.
    pub p50_latency_ms: Option<f64>,
    pub p95_latency_ms: Option<f64>,
    pub p99_latency_ms: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct FinopsSpendTimeseriesResponse {
    pub data: FinopsSpendTimeseries,
    pub status_code: u16,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
pub struct FinopsSpendTimeseries {
    /// "hour" | "day"
    pub bucket: String,
    pub points: Vec<SpendTimeseriesPoint>,
}

// ─── Spend concentration ────────────────────────────────────────────────────────

#[derive(Serialize, ToSchema)]
pub struct SpendCalendarDay {
    pub date: String,
    pub spend_usd: f64,
    pub operations: usize,
    /// 0.0-1.0, `spend_usd` relative to the month's max — lets the frontend
    /// shade heatmap intensity without a second pass.
    pub intensity: f64,
}

#[derive(Serialize, ToSchema)]
pub struct FinopsSpendCalendarResponse {
    pub data: FinopsSpendCalendar,
    pub status_code: u16,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
pub struct FinopsSpendCalendar {
    pub days: Vec<SpendCalendarDay>,
    /// Dates that fall inside the caller's active 24h/7d/30d range, so the
    /// frontend can pre-highlight matching blocks without recomputing dates.
    pub highlighted_dates: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct SpendHourPoint {
    /// 0-23, UTC.
    pub hour: u8,
    pub spend_usd: f64,
    /// Per-agent breakdown for this hour: top-N agents by daily spend,
    /// with the rest rolled into `others_spend_usd`.
    pub top_agents: Vec<AgentSpendSlice>,
    pub others_spend_usd: f64,
}

#[derive(Serialize, Clone, ToSchema)]
pub struct AgentSpendSlice {
    pub agent_name: String,
    pub spend_usd: f64,
}

#[derive(Serialize, ToSchema)]
pub struct SpendPieSlice {
    pub agent_name: String,
    pub spend_usd: f64,
    /// Percentage of total window spend, rounded to 2dp.
    pub pct: f64,
}

#[derive(Serialize, ToSchema)]
pub struct SpendByAgentBreakdown {
    /// Top-5 agents by spend, followed by an "Others" entry when there are
    /// more than 5 agents. Empty when there is no spend in the window.
    pub slices: Vec<SpendPieSlice>,
    pub total_spend_usd: f64,
}

#[derive(Serialize, ToSchema)]
pub struct FinopsDayDrilldownResponse {
    pub data: FinopsDayDrilldown,
    pub status_code: u16,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
pub struct FinopsDayDrilldown {
    pub date: String,
    /// 24 entries, 12am-12am UTC.
    pub hours: Vec<SpendHourPoint>,
    pub avg_hourly_spend_usd: f64,
    /// Top-N agents by total spend that day; the remainder is rolled into `others_spend_usd`.
    pub top_agents: Vec<AgentSpendSlice>,
    pub others_spend_usd: f64,
}

// ─── Attributions (Agent / Workflow) ────────────────────────────────────────────

#[derive(Serialize, ToSchema)]
pub struct WorkflowFinopsRow {
    pub maf_id: String,
    pub workflow_name: String,
    pub total_cost: f64,
    pub executions: usize,
    pub avg_cost_per_execution: f64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// Prompt tokens served from provider cache (OpenAI cached / Anthropic cache read).
    pub cache_read_tokens: u64,
    /// Prompt tokens written to provider cache (Anthropic cache creation).
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
    pub avg_latency_ms: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct FinopsAttributionsResponse {
    pub data: FinopsAttributions,
    pub status_code: u16,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "view")]
pub enum FinopsAttributions {
    #[serde(rename = "agent")]
    Agent { rows: Vec<AgentFinopsRow> },
    #[serde(rename = "workflow")]
    Workflow { rows: Vec<WorkflowFinopsRow> },
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
pub struct InsightsResponseEnvelope {
    pub data: InsightsData,
    pub status_code: u16,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
pub struct InsightsData {
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

#[derive(Debug, PartialEq, Eq)]
pub enum EnsureSessionOutcome {
    Created,
    Existing,
    Conflict,
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

    /// Returns Vec<(id, name, display_name, version)> for all agents in the DB. `name`
    /// doubles as the Tempo `service.name` (the injector sets OTEL_SERVICE_NAME
    /// to the agent name); `id` is the UUID reported to callers.
    /// OSS: returns all agents (NoopAuthorizer). EE adds RBAC at a higher layer.
    async fn get_agent_names(
        &self,
        owner_id: Option<uuid::Uuid>,
    ) -> Result<Vec<(uuid::Uuid, String, String, String)>, ObservabilityError> {
        // Live agents only (deleted_at IS NULL): a soft-deleted agent's name is
        // free to be re-registered by a re-upload, and leaving the dead rows in
        // would make the finops dashboard list one row per past incarnation —
        // all resolving to the same name — and double-count their spend in the
        // fleet totals (the name-keyed `agent_finops`/`count_user_traces` queries
        // return identical results for every duplicate). Historical spend of
        // deleted agents still counts toward the fleet figures via the
        // `token_usage`/`session_traces` queries, which are name/agent_id-keyed,
        // not row-keyed.
        sqlx::query_as::<_, (uuid::Uuid, String, String, String)>(
            "SELECT id, name, COALESCE(display_name, name), version FROM agents \
             WHERE deleted_at IS NULL \
               AND ($1::UUID IS NULL OR owner_id = $1) \
             ORDER BY name",
        )
        .bind(owner_id)
        .fetch_all(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))
    }

    /// Ensure a `chat_sessions` row exists for an external coding agent session.
    ///
    /// An existing session is idempotent only when both its user and agent match.
    pub async fn ensure_session(
        &self,
        session_id: &str,
        agent_name: &str,
        user_id_str: &str,
    ) -> Result<EnsureSessionOutcome, ObservabilityError> {
        let user_id: uuid::Uuid = user_id_str
            .parse()
            .map_err(|_| ObservabilityError::BadRequest("invalid user id".into()))?;

        // Agent names are unique per owner, not globally. An integration may
        // only attach sessions to the caller's own active agent.
        let agent_id: uuid::Uuid = sqlx::query_scalar(
            "SELECT id FROM agents WHERE owner_id = $1 AND name = $2 AND deleted_at IS NULL",
        )
        .bind(user_id)
        .bind(agent_name)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?
        .ok_or_else(|| ObservabilityError::NotFound(format!("agent '{}' not found", agent_name)))?;

        let created: Option<bool> = sqlx::query_scalar(
            r#"INSERT INTO chat_sessions (session_id, user_id, agent_id, title)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (session_id) DO UPDATE
                 SET session_id = EXCLUDED.session_id
                 WHERE chat_sessions.user_id = EXCLUDED.user_id
                   AND chat_sessions.agent_id = EXCLUDED.agent_id
               RETURNING xmax = 0"#,
        )
        .bind(session_id)
        .bind(user_id)
        .bind(agent_id)
        .bind("Coding session")
        .fetch_optional(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        Ok(match created {
            Some(true) => EnsureSessionOutcome::Created,
            Some(false) => EnsureSessionOutcome::Existing,
            None => EnsureSessionOutcome::Conflict,
        })
    }

    async fn authorize_session(
        &self,
        session_id: &str,
        user_id: &str,
        is_superuser: bool,
        resource: &str,
    ) -> Result<(DateTime<Utc>, DateTime<Utc>), ObservabilityError> {
        let caller: uuid::Uuid = user_id
            .parse()
            .map_err(|_| ObservabilityError::Internal("invalid user id in claims".into()))?;
        sqlx::query_as(
            r#"SELECT created_at, updated_at FROM chat_sessions
               WHERE session_id = $1 AND deleted_at IS NULL
                 AND ($2 OR user_id = $3)"#,
        )
        .bind(session_id)
        .bind(is_superuser)
        .bind(caller)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?
        .ok_or_else(|| ObservabilityError::NotFound(format!("{resource} not found")))
    }

    pub async fn authorize_session_access(
        &self,
        session_id: &str,
        user_id: &str,
        is_superuser: bool,
    ) -> Result<(), ObservabilityError> {
        self.authorize_session(session_id, user_id, is_superuser, "session")
            .await
            .map(|_| ())
    }

    async fn trace_session_id(
        &self,
        trace_id: &str,
        trace: &nasiko_observability::TraceDetails,
    ) -> Result<String, ObservabilityError> {
        if let Some(session_id) = trace.spans.iter().find_map(|span| {
            span.attributes
                .get("session.id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }) {
            return Ok(session_id);
        }

        sqlx::query_scalar("SELECT session_id FROM session_traces WHERE trace_id = $1")
            .bind(trace_id)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?
            .ok_or_else(|| ObservabilityError::NotFound("trace not found".into()))
    }

    // ── 1. session/list ──────────────────────────────────────────────────────

    /// Row for a session the trace store knows about: token counts, latency and
    /// cost come from its traces.
    fn session_summary_from_traces(
        session_id: String,
        agent_name: String,
        details: &nasiko_observability::SessionDetails,
    ) -> SessionSummary {
        let total_tokens = details.input_tokens
            + details.output_tokens
            + details.cache_read_tokens
            + details.cache_creation_tokens;
        let complete = details.metrics_complete;
        let started_at = details.traces.iter().map(|t| t.root_span.started_at).min();
        let ended_at = details
            .traces
            .iter()
            .filter_map(|t| t.root_span.ended_at)
            .max();
        let duration_ms = started_at
            .zip(ended_at)
            .map(|(s, e)| (e - s).num_milliseconds().max(0) as u64);

        SessionSummary {
            id: session_id.clone(),
            session_id,
            agent_id: agent_name,
            num_traces: Some(details.trace_count as u32),
            start_time: started_at.map(fmt_ts),
            // Clients parsing this as a timestamp need a non-empty string — fall back
            // to start_time when no end time is known.
            end_time: ended_at.or(started_at).map(fmt_ts),
            duration_ms,
            first_input: details.traces.first().and_then(|t| t.input_content.clone()),
            last_output: details.traces.last().and_then(|t| t.output_content.clone()),
            token_usage: TokenUsageSummary {
                total: (complete && total_tokens > 0).then_some(total_tokens),
            },
            trace_latency_ms_p50: complete.then_some(details.latency_ms_p50).flatten(),
            trace_latency_ms_p99: complete.then_some(details.latency_ms_p99).flatten(),
            cost_summary: SimpleCostSummary {
                total: CostEntry {
                    cost: (complete && details.cost.total_usd > 0.0)
                        .then_some(details.cost.total_usd),
                },
            },
            session_annotations: vec![],
            session_annotation_summaries: vec![],
        }
    }

    /// Row for a session with no traces — the agent isn't OTel-instrumented, or
    /// the trace store is unavailable. Everything the DB knows, nothing invented.
    fn session_summary_from_db(
        session_id: String,
        agent_name: String,
        created_at: DateTime<Utc>,
    ) -> SessionSummary {
        SessionSummary {
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
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn get_all_sessions(
        &self,
        user_id: &str,
        _role: Option<&str>,
        _department_id: Option<&str>,
        _team_id: Option<&str>,
        start_time: Option<&str>,
        is_superuser: bool,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<SessionListResponse, ObservabilityError> {
        let start = parse_iso_or_default(start_time, 7);
        let end = Utc::now();

        // Each row costs a trace-store round-trip, so the page size is the real
        // cost driver here. This used to load a flat 500 rows and enrich them
        // one at a time — up to 500 serial Tempo requests per page view, which
        // is why Execution history took so long to appear.
        let limit = limit.unwrap_or(DEFAULT_SESSION_PAGE).clamp(1, 100);
        let offset = offset.unwrap_or(0).max(0);
        // One extra row answers "is there a next page?" without a COUNT query.
        let fetch = limit + 1;

        // 1. Query chat_sessions as the authoritative source — every session
        //    shows up here regardless of whether the agent is OTel-instrumented.
        //    Non-superusers only see their own sessions.
        let mut db_sessions: Vec<(String, Option<uuid::Uuid>, DateTime<Utc>)> = if is_superuser {
            sqlx::query_as(
                "SELECT session_id, agent_id, created_at \
                 FROM chat_sessions \
                 WHERE deleted_at IS NULL AND created_at >= $1 \
                 ORDER BY created_at DESC, session_id DESC \
                 LIMIT $2 OFFSET $3",
            )
            .bind(start)
            .bind(fetch)
            .bind(offset)
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
                 ORDER BY created_at DESC, session_id DESC \
                 LIMIT $3 OFFSET $4",
            )
            .bind(caller_uuid)
            .bind(start)
            .bind(fetch)
            .bind(offset)
            .fetch_all(&self.db)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?
        };

        let has_next_page = db_sessions.len() > limit as usize;
        if has_next_page {
            db_sessions.pop();
        }

        // 2. Build agent_id → name lookup for the agent_id column.
        let agents = self.get_agent_names(None).await.unwrap_or_default();
        let total = agents.len();
        let agent_name_by_id: std::collections::HashMap<uuid::Uuid, String> = agents
            .into_iter()
            .map(|(id, name, _display, _version)| (id, name))
            .collect();

        // 3. Enrich each session from the trace store, concurrently. For agents
        //    without OTel the provider returns NotFound and we fall back to a
        //    minimal summary built from the DB row — the session still appears
        //    in the execution history.
        //
        //    Bounded fan-out rather than a serial loop: latency is now roughly
        //    one round-trip per batch instead of one per session, while the cap
        //    keeps a large page from stampeding the trace store.
        let enriched: Vec<SessionSummary> = stream::iter(db_sessions.into_iter().map(
            |(session_id, agent_id_opt, created_at)| {
                let agent_name = agent_id_opt
                    .and_then(|id| agent_name_by_id.get(&id))
                    .cloned()
                    .unwrap_or_default();
                async move {
                    match self.provider.get_session(&session_id, start, end).await {
                        Ok(details) => Self::session_summary_from_traces(
                            session_id,
                            agent_name,
                            &details,
                        ),
                        Err(e) => {
                            if !matches!(e, ObservabilityError::NotFound(_)) {
                                tracing::warn!(%session_id, error = %e, "trace lookup failed for session");
                            }
                            Self::session_summary_from_db(session_id, agent_name, created_at)
                        }
                    }
                }
            },
        ))
        .buffered(SESSION_ENRICH_CONCURRENCY)
        .collect()
        .await;

        // `num_traces` is Some only on the trace-store path, so it doubles as
        // "this session was found in the trace store".
        let successful = enriched.iter().filter(|s| s.num_traces.is_some()).count();
        let all_sessions = enriched;

        Ok(SessionListResponse {
            data: SessionListData {
                sessions: all_sessions,
                total_agents: total,
                successful_agents: successful,
                pagination: Pagination {
                    // Offset paging: the next page starts where this one ended.
                    end_cursor: has_next_page.then(|| (offset + limit).to_string()),
                    has_next_page,
                },
            },
        })
    }

    // ── 2. session/{session_id} ──────────────────────────────────────────────

    /// `(title, start, end)` for a session drill-down.
    ///
    /// The trace store is queried over a window, and this used to hardcode
    /// `now() - 7d`: opening any session older than a week returned an empty
    /// trace list. The session's own rows say when it actually ran —
    /// `chat_sessions` for chat-originated sessions, `session_traces` (written
    /// by agent_proxy for every forwarded query) for the rest — so take the
    /// window from whichever exists, padded for clock skew between the control
    /// plane and the agents' exporters.
    ///
    /// A session in neither table is unknown to us; the old 7-day window is
    /// then as good a guess as any, and keeps the Tempo scan bounded.
    async fn session_window(
        &self,
        session_id: &str,
    ) -> (Option<String>, DateTime<Utc>, DateTime<Utc>) {
        #[derive(sqlx::FromRow)]
        struct WindowRow {
            title: Option<String>,
            lo: Option<DateTime<Utc>>,
            hi: Option<DateTime<Utc>>,
        }

        // The aggregate subquery always yields one row, so the LEFT JOIN gives
        // a row whether or not the session ever went through chat.
        let row: Option<WindowRow> = sqlx::query_as(
            r#"SELECT c.title,
                      LEAST(c.created_at, t.lo) AS lo,
                      GREATEST(c.updated_at, t.hi) AS hi
                 FROM (SELECT MIN(created_at) AS lo, MAX(created_at) AS hi
                         FROM session_traces WHERE session_id = $1) t
                 LEFT JOIN chat_sessions c ON c.session_id = $1"#,
        )
        .bind(session_id)
        .fetch_optional(&self.db)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(%session_id, error = %e, "session window lookup failed");
            None
        });

        let now = Utc::now();
        match row {
            Some(WindowRow {
                title,
                lo: Some(lo),
                hi: Some(hi),
            }) => {
                let (start, end) = session_trace_window(lo, hi);
                (title, start, end)
            }
            Some(WindowRow { title, .. }) => (title, now - Duration::days(7), now),
            None => (None, now - Duration::days(7), now),
        }
    }

    pub async fn get_session_details(
        &self,
        session_id: &str,
        user_id: &str,
        is_superuser: bool,
    ) -> Result<SessionDetailResponse, ObservabilityError> {
        self.authorize_session(session_id, user_id, is_superuser, "session")
            .await?;
        let agent_name: Option<String> = sqlx::query_scalar(
            r#"SELECT CASE
                   WHEN a.coding_agent_integration_id IS NOT NULL
                        AND u.username IS NOT NULL
                        AND a.name NOT LIKE u.username || '-%'
                     THEN u.username || '-' || a.name
                   ELSE a.name
                 END
               FROM chat_sessions cs
               LEFT JOIN agents a ON a.id = cs.agent_id
               LEFT JOIN users u ON u.id = cs.user_id
               WHERE cs.session_id = $1"#,
        )
        .bind(session_id)
        .fetch_optional(&self.db)
        .await
        .map_err(|error| ObservabilityError::Internal(error.to_string()))?
        .flatten();
        // Anchor Tempo's seven-day maximum query range to this session rather
        // than to today. Otherwise an old session that still exists in
        // Postgres can never find its historical traces.
        let (title, start, end) = self.session_window(session_id).await;
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
                        cumulative_token_count_total: t.input_tokens
                            + t.output_tokens
                            + t.cache_read_tokens
                            + t.cache_creation_tokens,
                        input_tokens: t.input_tokens,
                        output_tokens: t.output_tokens,
                        cache_read_tokens: t.cache_read_tokens,
                        cache_creation_tokens: t.cache_creation_tokens,
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

        let total_tokens = details.input_tokens
            + details.output_tokens
            + details.cache_read_tokens
            + details.cache_creation_tokens;
        let end_cursor = trace_entries.last().map(|e| e.cursor.clone());

        Ok(SessionDetailResponse {
            data: SessionDetailData {
                session: SessionDetail {
                    id: details.session_id.clone(),
                    session_id: details.session_id.clone(),
                    title,
                    agent_name,
                    num_traces: details.trace_count,
                    token_usage: TokenUsageSummary {
                        total: details.metrics_complete.then_some(total_tokens),
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
                        cache_read: CostWithTokens {
                            cost: details.cost.cache_read_usd,
                            tokens: details.cache_read_tokens,
                        },
                        cache_creation: CostWithTokens {
                            cost: details.cost.cache_creation_usd,
                            tokens: details.cache_creation_tokens,
                        },
                    },
                    latency_p50: details.latency_ms_p50,
                    latency_p99: details.latency_ms_p99,
                    latency_avg: details.latency_ms_avg,
                    cache_read_tokens: details.cache_read_tokens,
                    cache_creation_tokens: details.cache_creation_tokens,
                    metrics_complete: details.metrics_complete,
                    traces: trace_entries,
                    pagination: Pagination {
                        end_cursor,
                        has_next_page: details.has_more_traces,
                    },
                },
            },
        })
    }

    // ── 3. trace/{trace_id} ──────────────────────────────────────────────────

    pub async fn get_trace_details(
        &self,
        trace_id: &str,
        user_id: &str,
        is_superuser: bool,
    ) -> Result<TraceDetailResponse, ObservabilityError> {
        let mut trace = self
            .provider
            .get_trace(trace_id)
            .await
            .map_err(|e| match e {
                ObservabilityError::NotFound(_) => {
                    ObservabilityError::NotFound("trace not found".into())
                }
                other => other,
            })?;
        let mut seen_spans = HashSet::new();
        trace
            .spans
            .retain(|span| seen_spans.insert(span.span_id.clone()));
        let project_session_id = self.trace_session_id(trace_id, &trace).await?;
        self.authorize_session(&project_session_id, user_id, is_superuser, "trace")
            .await?;

        let mut cost = CostBreakdown::default();
        let mut seen_spans = HashSet::new();
        for span in &trace.spans {
            if !seen_spans.insert(&span.span_id) {
                continue;
            }
            let u = extract_usage_attrs(&span.attributes);
            if u.is_empty() {
                continue;
            }
            cost.add_assign(
                self.provider
                    .cost(nasiko_observability::CostRequest::from_usage(
                        nasiko_observability::span_provider(span),
                        u.model.as_deref(),
                        span.started_at,
                        &u,
                    ))
                    .await,
            );
        }

        let trace_latency_ms = match (trace.started_at, trace.ended_at) {
            (Some(s), Some(e)) => Some((e - s).num_milliseconds().max(0) as f64),
            _ => None,
        };

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
                    project_session_id: Some(project_session_id),
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
                        cache_read: CostOnly {
                            cost: cost.cache_read_usd,
                        },
                        cache_creation: CostOnly {
                            cost: cost.cache_creation_usd,
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
        user_id: &str,
        is_superuser: bool,
    ) -> Result<SpanDetailResponse, ObservabilityError> {
        // Resolve and authorize the trace's session before looking up the span
        // or its Loki content, so inaccessible traces reveal no span existence.
        let trace = self
            .provider
            .get_trace(trace_id)
            .await
            .map_err(|e| match e {
                ObservabilityError::NotFound(_) => {
                    ObservabilityError::NotFound("span not found".into())
                }
                other => other,
            })?;
        let session_id = self
            .trace_session_id(trace_id, &trace)
            .await
            .map_err(|e| match e {
                ObservabilityError::NotFound(_) => {
                    ObservabilityError::NotFound("span not found".into())
                }
                other => other,
            })?;
        self.authorize_session(&session_id, user_id, is_superuser, "span")
            .await?;
        let details = self
            .provider
            .get_span(trace_id, span_id)
            .await
            .map_err(|e| match e {
                ObservabilityError::NotFound(_) => {
                    ObservabilityError::NotFound("span not found".into())
                }
                other => other,
            })?;
        let span = &details.span;

        // Token counts come from the provider (it rolls the span's children up);
        // only the model name still has to be read off the raw attributes.
        let (.., model) = extract_token_attrs(&span.attributes);
        let usage = &details.token_usage;

        // Span kind: prefer openinference.span.kind (e.g. "LLM"), fallback to OTel kind
        let span_kind = span
            .attributes
            .get("openinference.span.kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| span_kind_str(span.kind).to_lowercase());

        // Input: prefer span attributes — OpenInference "input.value", then the
        // GenAI semconv "gen_ai.input.messages" — falling back to Loki content.
        let input_value = span
            .attributes
            .get("input.value")
            .or_else(|| span.attributes.get("gen_ai.input.messages"))
            .or_else(|| span.attributes.get("tool.arguments"))
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

        // Output: same precedence as input — OpenInference, GenAI semconv, Loki.
        let output_value = span
            .attributes
            .get("output.value")
            .or_else(|| span.attributes.get("gen_ai.output.messages"))
            .or_else(|| span.attributes.get("tool.result"))
            .or_else(|| span.attributes.get("error.message"))
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
                    name: span_display_name(span),
                    span_kind,
                    code: status.clone(),
                    status_code: status,
                    status_message: span.status_message.clone(),
                    start_time: Some(fmt_ts(span.started_at)),
                    end_time: span.ended_at.map(fmt_ts),
                    parent_id: span.parent_span_id.clone(),
                    latency_ms: span.duration_ms.map(|d| d as f64),
                    token_count_total: usage.total_tokens,
                    provider: first_str_attr(
                        &span.attributes,
                        &["gen_ai.system", "gen_ai.provider.name"],
                    ),
                    model: model
                        .or_else(|| first_str_attr(&span.attributes, &["gen_ai.response.model"])),
                    cache_read_tokens: usage.cache_read_tokens,
                    cache_creation_tokens: usage.cache_creation_tokens,
                    cost_summary: FullCostSummary {
                        total: CostWithTokens {
                            cost: details.cost.total_usd,
                            tokens: usage.total_tokens,
                        },
                        prompt: CostWithTokens {
                            cost: details.cost.prompt_usd,
                            tokens: usage.input_tokens,
                        },
                        completion: CostWithTokens {
                            cost: details.cost.completion_usd,
                            tokens: usage.output_tokens,
                        },
                        cache_read: CostWithTokens {
                            cost: details.cost.cache_read_usd,
                            tokens: usage.cache_read_tokens,
                        },
                        cache_creation: CostWithTokens {
                            cost: details.cost.cache_creation_usd,
                            tokens: usage.cache_creation_tokens,
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
                    // Tempo already parses these (oss/observability/src/tempo.rs);
                    // they used to be dropped on the floor here, which left the
                    // "Metadata & events" tab with nothing to render.
                    events: span
                        .events
                        .iter()
                        .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
                        .collect(),
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
        let end = Utc::now();

        #[derive(sqlx::FromRow)]
        struct StatsRow {
            trace_count: i64,
            total_cost: f64,
            prompt_cost: f64,
            completion_cost: f64,
            p50: Option<f64>,
            p99: Option<f64>,
        }

        let row: StatsRow = sqlx::query_as(
            r#"SELECT COUNT(*)::BIGINT AS trace_count,
                      COALESCE(SUM(cost_usd), 0)::FLOAT8 AS total_cost,
                      COALESCE(SUM(prompt_cost_usd), 0)::FLOAT8 AS prompt_cost,
                      COALESCE(SUM(completion_cost_usd), 0)::FLOAT8 AS completion_cost,
                      percentile_cont(0.5) WITHIN GROUP (ORDER BY latency_ms)::FLOAT8 AS p50,
                      percentile_cont(0.99) WITHIN GROUP (ORDER BY latency_ms)::FLOAT8 AS p99
               FROM trace_usage
               WHERE agent_name = $1 AND started_at >= $2 AND started_at < $3"#,
        )
        .bind(agent_id)
        .bind(start)
        .bind(end)
        .fetch_one(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        let cache_cost = self
            .provider
            .agent_stats(agent_id, start, end)
            .await
            .map(|stats| (stats.cost.cache_read_usd, stats.cost.cache_creation_usd))
            .unwrap_or_default();

        Ok(AgentStatsResponse {
            data: AgentStatsData {
                project: AgentProjectStats {
                    id: agent_id.to_string(),
                    trace_count: row.trace_count as usize,
                    cost_summary: NestedCostSummary {
                        total: CostOnly {
                            cost: row.total_cost,
                        },
                        prompt: CostOnly {
                            cost: row.prompt_cost,
                        },
                        completion: CostOnly {
                            cost: row.completion_cost,
                        },
                        cache_read: CostOnly { cost: cache_cost.0 },
                        cache_creation: CostOnly { cost: cache_cost.1 },
                    },
                    latency_ms_p50: row.p50,
                    latency_ms_p99: row.p99,
                    span_annotation_names: vec![],
                    document_evaluation_names: vec![],
                },
            },
            status_code: 200,
            message: "Agent stats retrieved successfully".into(),
        })
    }

    // ── 6. finops/dashboard ───────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub async fn get_finops_dashboard(
        &self,
        _user_id: &str,
        _role: Option<&str>,
        _department_id: Option<&str>,
        _team_id: Option<&str>,
        start_time: Option<&str>,
        end_time: Option<&str>,
        agent_name: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
        // User UUIDs to scope results to (EE org-unit filter). `None` = no filter.
        user_ids: Option<&[uuid::Uuid]>,
        accessible_agent_ids: Option<&[uuid::Uuid]>,
        // When `Some`, restricts the agent list to agents owned by this user UUID.
        owner_id: Option<&str>,
        view: &str,
    ) -> Result<FinopsDashboardResponse, ObservabilityError> {
        let parsed_owner_id = owner_id.and_then(|s| s.parse::<uuid::Uuid>().ok());
        let accessible: Option<HashSet<uuid::Uuid>> =
            accessible_agent_ids.map(|ids| ids.iter().copied().collect());
        let all_agents = self.get_agent_names(parsed_owner_id).await?;
        let mut agents: Vec<_> = match agent_name {
            Some(name) => all_agents
                .into_iter()
                .filter(|(_, n, _, _)| n == name)
                .collect(),
            None => all_agents,
        };
        if let Some(accessible) = &accessible {
            agents.retain(|(id, _, _, _)| accessible.contains(id));
        }
        // `is_internal` agents (Weave's dashboard-generator) are platform-owned,
        // not the caller's fleet, so a workspace holding nothing else has still
        // deployed nothing -- and the overview/tokenops pages read exactly that
        // off `total_agents` to decide between their first-run screen and a real
        // dashboard. Dropping them makes the list empty, which the early return
        // below turns into the zeroed payload those pages expect. Once the caller
        // has an agent of their own the internal one stays in, so its spend is
        // still attributed.
        //
        // Scoped to the agents actually in hand, and skipped entirely when there
        // are none: an unscoped `SELECT id FROM agents WHERE is_internal` reads
        // every internal agent in the deployment on every dashboard load, and ran
        // even for the no-agent first-run case this exists to serve, where its
        // answer cannot change anything.
        let mut dropped_internal: HashSet<uuid::Uuid> = HashSet::new();
        if !agents.is_empty() {
            let ids: Vec<uuid::Uuid> = agents.iter().map(|(id, _, _, _)| *id).collect();
            let internal_count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM agents \
                 WHERE id = ANY($1) AND is_internal AND deleted_at IS NULL",
            )
            .bind(&ids)
            .fetch_one(&self.db)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?;
            if internal_count as usize == ids.len() {
                dropped_internal = ids.into_iter().collect();
                agents.clear();
            }
        }
        let total_agents = agents.len();

        let start = parse_iso_or_default(start_time, 30);
        let real_now = Utc::now();
        let now = end_time
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or(real_now);
        let last_24h = real_now - Duration::hours(24);

        // Container-hours for the same window, one batched query. Includes
        // agents that have since been deleted, so the summary total stays
        // honest even when the per-agent rows below can't show them.
        // Fail-soft, matching the per-agent finops calls.
        let window_len = now - start;
        let prev_end = start;
        let prev_start = start - window_len;

        let mut hours_rows = hours_meter::windowed_agent_hours(&self.db, start, now, None)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "container hours aggregation failed");
                vec![]
            });
        if let Some(accessible) = &accessible {
            hours_rows.retain(|row| accessible.contains(&row.agent_id));
        }
        // An agent we just denied the existence of cannot keep billing hours into
        // the summary. Without this the first-run screen reports zero agents and
        // zero spend beside a non-zero container-hours figure -- the one number
        // the internal agent still contributed -- which reads as a bug rather
        // than as an empty workspace. Deleted agents' hours are untouched: those
        // are real history with no agent left to attribute them to, which is why
        // `empty_finops_response` takes the total rather than zeroing it.
        if !dropped_internal.is_empty() {
            hours_rows.retain(|row| !dropped_internal.contains(&row.agent_id));
        }
        let total_container_hours = round6(hours_rows.iter().map(|r| r.hours).sum());
        let hours_by_agent: HashMap<uuid::Uuid, f64> =
            hours_rows.iter().map(|r| (r.agent_id, r.hours)).collect();

        if agents.is_empty() {
            return Ok(empty_finops_response(total_container_hours));
        }

        // ── Postgres-backed aggregation from trace_usage ─────────────────────
        // Three SQL queries replace the ~5,000 Tempo HTTP calls.

        #[derive(sqlx::FromRow)]
        struct TraceUsageAgg {
            agent_name: String,
            operations: i64,
            input_tokens: i64,
            output_tokens: i64,
            cache_read_tokens: i64,
            cache_creation_tokens: i64,
            total_cost: f64,
            estimated_cost: f64,
            unknown_confidence_calls: i64,
            tool_call_count: i64,
            p50_latency: Option<f64>,
            p95_latency: Option<f64>,
            p99_latency: Option<f64>,
        }

        let agent_filter: Option<&str> = agent_name;
        let model_filter: Option<&str> = model;
        let provider_filter: Option<&str> = provider;
        // Empty slice means "no filter" — sqlx binds it as an empty array,
        // and `= ANY('{}'::uuid[])` matches nothing, so we use a NULL flag.
        let has_user_filter = user_ids.is_some_and(|ids| !ids.is_empty());
        let user_id_list: Vec<uuid::Uuid> = user_ids
            .filter(|ids| !ids.is_empty())
            .map(|ids| ids.to_vec())
            .unwrap_or_default();

        // Shared WHERE fragment: agent, model, provider, user_id filters.
        // $1/$2 = time window, $3 = agent, $4 = model, $5 = provider,
        // $6 = has_user_filter (bool), $7 = user_id_list (uuid[]).
        const TRACE_USAGE_AGG_QUERY: &str = r#"SELECT agent_name,
                      COUNT(*)::BIGINT AS operations,
                      COALESCE(SUM(input_tokens), 0)::BIGINT AS input_tokens,
                      COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
                      COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
                      COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
                      COALESCE(SUM(cost_usd), 0)::FLOAT8 AS total_cost,
                      COALESCE(SUM(cost_usd) FILTER (WHERE cost_estimated), 0)::FLOAT8 AS estimated_cost,
                      COUNT(*) FILTER (WHERE cost_estimated IS NULL)::BIGINT AS unknown_confidence_calls,
                      COALESCE(SUM(tool_call_count), 0)::BIGINT AS tool_call_count,
                      percentile_cont(0.5)  WITHIN GROUP (ORDER BY latency_ms)::FLOAT8 AS p50_latency,
                      percentile_cont(0.95) WITHIN GROUP (ORDER BY latency_ms)::FLOAT8 AS p95_latency,
                      percentile_cont(0.99) WITHIN GROUP (ORDER BY latency_ms)::FLOAT8 AS p99_latency
               FROM trace_usage
               WHERE started_at >= $1 AND started_at < $2
                 AND ($3::TEXT IS NULL OR agent_name = $3)
                 AND ($4::TEXT IS NULL OR model = $4)
                 AND ($5::TEXT IS NULL OR provider = $5)
                 AND (NOT $6::BOOL OR user_id = ANY($7::UUID[]))
               GROUP BY agent_name"#;

        // 1. Current window
        let current_rows: Vec<TraceUsageAgg> = sqlx::query_as(TRACE_USAGE_AGG_QUERY)
            .bind(start)
            .bind(now)
            .bind(agent_filter)
            .bind(model_filter)
            .bind(provider_filter)
            .bind(has_user_filter)
            .bind(&user_id_list)
            .fetch_all(&self.db)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        // 2. Previous window (for KPI %-change)
        let prev_rows: Vec<TraceUsageAgg> = sqlx::query_as(TRACE_USAGE_AGG_QUERY)
            .bind(prev_start)
            .bind(prev_end)
            .bind(agent_filter)
            .bind(model_filter)
            .bind(provider_filter)
            .bind(has_user_filter)
            .bind(&user_id_list)
            .fetch_all(&self.db)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        // 3. 24h operation count
        #[derive(sqlx::FromRow)]
        struct OpsCount {
            agent_name: String,
            cnt: i64,
        }
        let ops_24h_rows: Vec<OpsCount> = sqlx::query_as(
            r#"SELECT agent_name, COUNT(*)::BIGINT AS cnt
               FROM trace_usage
               WHERE started_at >= $1
                 AND ($2::TEXT IS NULL OR agent_name = $2)
                 AND ($3::TEXT IS NULL OR model = $3)
                 AND ($4::TEXT IS NULL OR provider = $4)
                 AND (NOT $5::BOOL OR user_id = ANY($6::UUID[]))
               GROUP BY agent_name"#,
        )
        .bind(last_24h)
        .bind(agent_filter)
        .bind(model_filter)
        .bind(provider_filter)
        .bind(has_user_filter)
        .bind(&user_id_list)
        .fetch_all(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        // 4. Unpriced calls in the window: ran (had tokens) but recorded no cost,
        //    because no price row backs the model. `SUM(cost_usd)` skips them, so
        //    surface the count as a known gap. Same filters as the main aggregation.
        let unpriced_calls: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*)::BIGINT
               FROM trace_usage
               WHERE started_at >= $1 AND started_at < $2
                 AND ($3::TEXT IS NULL OR agent_name = $3)
                 AND ($4::TEXT IS NULL OR model = $4)
                 AND ($5::TEXT IS NULL OR provider = $5)
                 AND (NOT $6::BOOL OR user_id = ANY($7::UUID[]))
                 AND COALESCE(cost_usd, 0) = 0
                 AND (COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)) > 0"#,
        )
        .bind(start)
        .bind(now)
        .bind(agent_filter)
        .bind(model_filter)
        .bind(provider_filter)
        .bind(has_user_filter)
        .bind(&user_id_list)
        .fetch_one(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        // Index lookups for joining.
        let current_by_name: HashMap<&str, &TraceUsageAgg> = current_rows
            .iter()
            .map(|r| (r.agent_name.as_str(), r))
            .collect();
        let prev_by_name: HashMap<&str, &TraceUsageAgg> = prev_rows
            .iter()
            .map(|r| (r.agent_name.as_str(), r))
            .collect();
        let ops24h_by_name: HashMap<&str, i64> = ops_24h_rows
            .iter()
            .map(|r| (r.agent_name.as_str(), r.cnt))
            .collect();

        // Build per-agent rows, joined with agents table for UUID/display_name/version.
        let mut agent_rows: Vec<AgentFinopsRow> = Vec::new();
        let mut grand_input = 0u64;
        let mut grand_output = 0u64;
        let mut grand_cache_read = 0u64;
        let mut grand_cache_creation = 0u64;
        let mut grand_cost = 0f64;
        let mut total_ops = 0usize;
        let mut total_ops_24h = 0usize;
        let mut active = 0usize;
        let mut prev_grand_cost = 0f64;
        let mut prev_grand_total_tokens = 0u64;
        let mut prev_total_ops = 0usize;
        let mut prev_latency_samples: Vec<f64> = Vec::new();
        let mut latency_samples: Vec<f64> = Vec::new();
        let mut latency_samples_p95: Vec<f64> = Vec::new();
        let mut latency_samples_p99: Vec<f64> = Vec::new();
        let mut prev_latency_samples_p95: Vec<f64> = Vec::new();
        let mut prev_latency_samples_p99: Vec<f64> = Vec::new();
        let mut grand_tool_calls = 0u64;
        let mut prev_grand_tool_calls = 0u64;
        let mut prev_active = 0usize;

        for (agent_uuid, name, display_name, version) in &agents {
            let cur = current_by_name.get(name.as_str());
            let prev = prev_by_name.get(name.as_str());
            let ops_24h = ops24h_by_name.get(name.as_str()).copied().unwrap_or(0) as usize;

            let operations = cur.map(|c| c.operations as usize).unwrap_or(0);
            let input = cur.map(|c| c.input_tokens as u64).unwrap_or(0);
            let output = cur.map(|c| c.output_tokens as u64).unwrap_or(0);
            let cache_read = cur.map(|c| c.cache_read_tokens as u64).unwrap_or(0);
            let cache_creation = cur.map(|c| c.cache_creation_tokens as u64).unwrap_or(0);
            let cost = cur.map(|c| c.total_cost).unwrap_or(0.0);
            let tool_calls = cur.map(|c| c.tool_call_count as u64).unwrap_or(0);
            let p50 = cur.and_then(|c| c.p50_latency);
            let p95 = cur.and_then(|c| c.p95_latency);
            let p99 = cur.and_then(|c| c.p99_latency);

            if operations > 0 {
                active += 1;
            }
            let avg_cost = if operations > 0 {
                round6(cost / operations as f64)
            } else {
                0.0
            };

            grand_input += input;
            grand_output += output;
            grand_cache_read += cache_read;
            grand_cache_creation += cache_creation;
            grand_cost += cost;
            grand_tool_calls += tool_calls;
            total_ops += operations;
            total_ops_24h += ops_24h;
            if let Some(l) = p50 {
                latency_samples.push(l);
            }
            if let Some(l) = p95 {
                latency_samples_p95.push(l);
            }
            if let Some(l) = p99 {
                latency_samples_p99.push(l);
            }

            if let Some(p) = prev {
                prev_grand_cost += p.total_cost;
                prev_grand_total_tokens += (p.input_tokens + p.output_tokens) as u64;
                prev_total_ops += p.operations as usize;
                prev_grand_tool_calls += p.tool_call_count as u64;
                if p.operations > 0 {
                    prev_active += 1;
                }
                if let Some(l) = p.p50_latency {
                    prev_latency_samples.push(l);
                }
                if let Some(l) = p.p95_latency {
                    prev_latency_samples_p95.push(l);
                }
                if let Some(l) = p.p99_latency {
                    prev_latency_samples_p99.push(l);
                }
            }

            agent_rows.push(AgentFinopsRow {
                agent_id: agent_uuid.to_string(),
                agent_name: display_name.clone(),
                total_cost: cost,
                operations,
                is_capped: false,
                avg_cost_per_operation: avg_cost,
                prompt_tokens: input,
                completion_tokens: output,
                cache_read_tokens: cache_read,
                cache_creation_tokens: cache_creation,
                total_tokens: input + output + cache_read + cache_creation,
                avg_latency_ms: p50,
                avg_latency_p95_ms: p95,
                avg_latency_p99_ms: p99,
                tool_call_count: tool_calls,
                version: Some(version.clone()),
                container_hours: round6(hours_by_agent.get(agent_uuid).copied().unwrap_or(0.0)),
            });
        }

        let avg_cost = if total_ops > 0 {
            round6(grand_cost / total_ops as f64)
        } else {
            0.0
        };
        let grand_total_tokens =
            grand_input + grand_output + grand_cache_read + grand_cache_creation;
        let avg_tpo = if total_ops > 0 {
            grand_total_tokens / total_ops as u64
        } else {
            0
        };

        let avg_latency = |samples: &[f64]| -> f64 {
            if samples.is_empty() {
                0.0
            } else {
                samples.iter().sum::<f64>() / samples.len() as f64
            }
        };
        let prev_avg_cost = if prev_total_ops > 0 {
            prev_grand_cost / prev_total_ops as f64
        } else {
            0.0
        };
        let kpis = FinopsKpis {
            total_spend: KpiValue::new(round6(grand_cost), round6(prev_grand_cost)),
            total_tokens: KpiValue::new(grand_total_tokens as f64, prev_grand_total_tokens as f64),
            cost_per_operation: KpiValue::new(avg_cost, round6(prev_avg_cost)),
            avg_latency_ms: KpiValue::new(
                avg_latency(&latency_samples),
                avg_latency(&prev_latency_samples),
            ),
            // previous = 0 → change_pct = None (headcount, not window-relative)
            total_agents: KpiValue::new(total_agents as f64, 0.0),
            active_agents: KpiValue::new(active as f64, prev_active as f64),
            total_operations: KpiValue::new(total_ops as f64, prev_total_ops as f64),
            total_tool_calls: KpiValue::new(grand_tool_calls as f64, prev_grand_tool_calls as f64),
            latency_p95_ms: KpiValue::new(
                avg_latency(&latency_samples_p95),
                avg_latency(&prev_latency_samples_p95),
            ),
            latency_p99_ms: KpiValue::new(
                avg_latency(&latency_samples_p99),
                avg_latency(&prev_latency_samples_p99),
            ),
        };

        const TOP_N: usize = 5;
        let spend_by_agent = {
            let total = round6(grand_cost);
            let mut sorted = agent_rows.clone();
            sorted.sort_by(|a, b| {
                b.total_cost
                    .partial_cmp(&a.total_cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let pct = |v: f64| {
                if total > 0.0 {
                    (v / total * 10_000.0).round() / 100.0
                } else {
                    0.0
                }
            };
            let mut slices: Vec<SpendPieSlice> = sorted
                .iter()
                .take(TOP_N)
                .map(|r| SpendPieSlice {
                    agent_name: r.agent_name.clone(),
                    spend_usd: round6(r.total_cost),
                    pct: pct(r.total_cost),
                })
                .collect();
            let others: f64 = sorted.iter().skip(TOP_N).map(|r| r.total_cost).sum();
            if others > 0.0 {
                slices.push(SpendPieSlice {
                    agent_name: "Others".into(),
                    spend_usd: round6(others),
                    pct: pct(others),
                });
            }
            SpendByAgentBreakdown {
                slices,
                total_spend_usd: total,
            }
        };

        let attributions = if view == "workflow" {
            let rows = self
                .get_workflow_finops_rows(start, now, agent_name)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "workflow finops aggregation failed");
                    vec![]
                });
            FinopsAttributions::Workflow { rows }
        } else {
            FinopsAttributions::Agent {
                rows: agent_rows.clone(),
            }
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
                    unpriced_calls: unpriced_calls.max(0) as usize,
                    estimated_cost: current_rows.iter().map(|row| row.estimated_cost).sum(),
                    unknown_confidence_calls: current_rows
                        .iter()
                        .map(|row| row.unknown_confidence_calls.max(0) as usize)
                        .sum(),
                },
                agents: agent_rows,
                token_usage: FinopsTokenUsage {
                    total_tokens: grand_total_tokens,
                    prompt_tokens: grand_input,
                    completion_tokens: grand_output,
                    cache_read_tokens: grand_cache_read,
                    cache_creation_tokens: grand_cache_creation,
                    avg_tokens_per_operation: avg_tpo,
                },
                kpis,
                attributions,
                spend_by_agent,
            },
            status_code: 200,
            message: "FinOps dashboard data retrieved successfully".into(),
        })
    }

    // ── 6b. finops/spend-timeseries ───────────────────────────────────────────

    pub async fn get_finops_spend_timeseries(
        &self,
        start_time: Option<&str>,
        end_time: Option<&str>,
        range: Option<&str>,
        agent_name: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
    ) -> Result<FinopsSpendTimeseriesResponse, ObservabilityError> {
        let (start, end, bucket) = resolve_window(start_time, end_time, range)?;
        let trunc = match bucket {
            TimeBucket::Hour => "hour",
            TimeBucket::Day => "day",
        };

        #[derive(sqlx::FromRow)]
        struct BucketRow {
            bucket_start: DateTime<Utc>,
            spend_usd: f64,
            operations: i64,
            tool_calls: i64,
            top_agent_name: Option<String>,
            top_agent_spend_usd: Option<f64>,
            p50_latency_ms: Option<f64>,
            p95_latency_ms: Option<f64>,
            p99_latency_ms: Option<f64>,
        }

        // CTEs:
        //  • buckets  — per-(bucket, agent) spend + op count
        //  • ranked   — top-spender per bucket for the hover label
        //  • latency  — p50/p95/p99 over all traces in each bucket
        //               (computed directly from raw rows, not from per-agent
        //               aggregates, so the percentiles are exact)
        let query = format!(
            r#"WITH buckets AS (
                   SELECT date_trunc('{trunc}', started_at) AS bucket_start,
                          agent_name,
                          SUM(cost_usd) AS agent_spend,
                          COUNT(*)::BIGINT AS ops,
                          COALESCE(SUM(tool_call_count), 0)::BIGINT AS agent_tool_calls
                   FROM trace_usage
                   WHERE started_at >= $1 AND started_at < $2
                     AND ($3::TEXT IS NULL OR agent_name = $3)
                     AND ($4::TEXT IS NULL OR model = $4)
                     AND ($5::TEXT IS NULL OR provider = $5)
                   GROUP BY bucket_start, agent_name
               ),
               ranked AS (
                   SELECT *, ROW_NUMBER() OVER (PARTITION BY bucket_start ORDER BY agent_spend DESC) AS rn
                   FROM buckets
               ),
               latency AS (
                   SELECT date_trunc('{trunc}', started_at) AS bucket_start,
                          percentile_cont(0.5)  WITHIN GROUP (ORDER BY latency_ms)::FLOAT8 AS p50,
                          percentile_cont(0.95) WITHIN GROUP (ORDER BY latency_ms)::FLOAT8 AS p95,
                          percentile_cont(0.99) WITHIN GROUP (ORDER BY latency_ms)::FLOAT8 AS p99
                   FROM trace_usage
                   WHERE started_at >= $1 AND started_at < $2
                     AND latency_ms IS NOT NULL
                     AND ($3::TEXT IS NULL OR agent_name = $3)
                     AND ($4::TEXT IS NULL OR model = $4)
                     AND ($5::TEXT IS NULL OR provider = $5)
                   GROUP BY bucket_start
               )
               SELECT r.bucket_start,
                      SUM(r.agent_spend)::FLOAT8 AS spend_usd,
                      SUM(r.ops)::BIGINT AS operations,
                      SUM(r.agent_tool_calls)::BIGINT AS tool_calls,
                      MAX(CASE WHEN r.rn = 1 THEN r.agent_name END) AS top_agent_name,
                      MAX(CASE WHEN r.rn = 1 THEN r.agent_spend END)::FLOAT8 AS top_agent_spend_usd,
                      l.p50 AS p50_latency_ms,
                      l.p95 AS p95_latency_ms,
                      l.p99 AS p99_latency_ms
               FROM ranked r
               LEFT JOIN latency l ON l.bucket_start = r.bucket_start
               GROUP BY r.bucket_start, l.p50, l.p95, l.p99
               ORDER BY r.bucket_start"#
        );

        let rows: Vec<BucketRow> = sqlx::query_as(&query)
            .bind(start)
            .bind(end)
            .bind(agent_name)
            .bind(model)
            .bind(provider)
            .fetch_all(&self.db)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        Ok(FinopsSpendTimeseriesResponse {
            data: FinopsSpendTimeseries {
                bucket: bucket_label(bucket).to_string(),
                points: rows
                    .into_iter()
                    .map(|r| SpendTimeseriesPoint {
                        bucket_start: fmt_ts(r.bucket_start),
                        spend_usd: round6(r.spend_usd),
                        operations: r.operations as usize,
                        tool_calls: r.tool_calls.max(0) as u64,
                        top_agent_name: r.top_agent_name,
                        top_agent_spend_usd: r.top_agent_spend_usd.map(round6),
                        p50_latency_ms: r.p50_latency_ms,
                        p95_latency_ms: r.p95_latency_ms,
                        p99_latency_ms: r.p99_latency_ms,
                    })
                    .collect(),
            },
            status_code: 200,
            message: "Spend timeseries retrieved successfully".into(),
        })
    }

    // ── 6c. finops/spend-calendar ─────────────────────────────────────────────

    pub async fn get_finops_spend_calendar(
        &self,
        month: &str,
        range: Option<&str>,
        agent_name: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
    ) -> Result<FinopsSpendCalendarResponse, ObservabilityError> {
        let month_start = chrono::NaiveDateTime::parse_from_str(
            &format!("{month}-01 00:00:00"),
            "%Y-%m-%d %H:%M:%S",
        )
        .map(|d| d.and_utc())
        .map_err(|_| ObservabilityError::BadRequest(format!("invalid month '{month}'")))?;
        let next_month = if month_start.month() == 12 {
            Utc.with_ymd_and_hms(month_start.year() + 1, 1, 1, 0, 0, 0)
        } else {
            Utc.with_ymd_and_hms(month_start.year(), month_start.month() + 1, 1, 0, 0, 0)
        }
        .single()
        .ok_or_else(|| ObservabilityError::Internal("month arithmetic failed".into()))?;

        #[derive(sqlx::FromRow)]
        struct DayRow {
            date: chrono::NaiveDate,
            spend_usd: f64,
            operations: i64,
        }

        let rows: Vec<DayRow> = sqlx::query_as(
            r#"SELECT DATE(started_at) AS date,
                      COALESCE(SUM(cost_usd), 0)::FLOAT8 AS spend_usd,
                      COUNT(*)::BIGINT AS operations
               FROM trace_usage
               WHERE started_at >= $1 AND started_at < $2
                 AND ($3::TEXT IS NULL OR agent_name = $3)
                 AND ($4::TEXT IS NULL OR model = $4)
                 AND ($5::TEXT IS NULL OR provider = $5)
               GROUP BY DATE(started_at)
               ORDER BY date"#,
        )
        .bind(month_start)
        .bind(next_month)
        .bind(agent_name)
        .bind(model)
        .bind(provider)
        .fetch_all(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        let max_spend = rows.iter().map(|r| r.spend_usd).fold(0.0_f64, f64::max);
        let mut days: Vec<SpendCalendarDay> = rows
            .into_iter()
            .map(|r| {
                let date_str = r.date.format("%Y-%m-%d").to_string();
                SpendCalendarDay {
                    intensity: if max_spend > 0.0 {
                        r.spend_usd / max_spend
                    } else {
                        0.0
                    },
                    date: date_str,
                    spend_usd: round6(r.spend_usd),
                    operations: r.operations as usize,
                }
            })
            .collect();
        days.sort_by(|a, b| a.date.cmp(&b.date));

        let highlighted_dates = range
            .and_then(range_hours)
            .map(|hours| {
                let range_start = Utc::now() - Duration::hours(hours);
                let cutoff = range_start.format("%Y-%m-%d").to_string();
                days.iter()
                    .filter(|d| d.date >= cutoff)
                    .map(|d| d.date.clone())
                    .collect()
            })
            .unwrap_or_default();

        Ok(FinopsSpendCalendarResponse {
            data: FinopsSpendCalendar {
                days,
                highlighted_dates,
            },
            status_code: 200,
            message: "Spend calendar retrieved successfully".into(),
        })
    }

    // ── 6d. finops/spend-calendar/day ─────────────────────────────────────────

    pub async fn get_finops_spend_calendar_day(
        &self,
        date: &str,
        agent_name: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
    ) -> Result<FinopsDayDrilldownResponse, ObservabilityError> {
        let day_start =
            chrono::NaiveDateTime::parse_from_str(&format!("{date} 00:00:00"), "%Y-%m-%d %H:%M:%S")
                .map(|d| d.and_utc())
                .map_err(|_| ObservabilityError::BadRequest(format!("invalid date '{date}'")))?;
        let day_end = day_start + Duration::days(1);

        #[derive(sqlx::FromRow)]
        struct HourAgentRow {
            hour: i32,
            agent_name: String,
            spend_usd: f64,
        }

        let rows: Vec<HourAgentRow> = sqlx::query_as(
            r#"SELECT EXTRACT(HOUR FROM started_at)::INT AS hour,
                      agent_name,
                      COALESCE(SUM(cost_usd), 0)::FLOAT8 AS spend_usd
               FROM trace_usage
               WHERE started_at >= $1 AND started_at < $2
                 AND ($3::TEXT IS NULL OR agent_name = $3)
                 AND ($4::TEXT IS NULL OR model = $4)
                 AND ($5::TEXT IS NULL OR provider = $5)
               GROUP BY hour, agent_name
               ORDER BY hour"#,
        )
        .bind(day_start)
        .bind(day_end)
        .bind(agent_name)
        .bind(model)
        .bind(provider)
        .fetch_all(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        // Resolve display names from the agents table.
        let all_agents = self.get_agent_names(None).await.unwrap_or_default();
        let display_by_name: HashMap<&str, &str> = all_agents
            .iter()
            .map(|(_, name, display, _)| (name.as_str(), display.as_str()))
            .collect();

        let resolve =
            |name: &str| -> String { display_by_name.get(name).unwrap_or(&name).to_string() };

        // Day-level per-agent totals → determine the top-N agents for the day.
        // The same top-N names are used in every hourly breakdown so the
        // stacked bar chart has consistent colors/ordering across hours.
        let mut day_agent_spend: HashMap<&str, f64> = HashMap::new();
        for r in &rows {
            *day_agent_spend.entry(r.agent_name.as_str()).or_insert(0.0) += r.spend_usd;
        }
        let mut day_per_agent: Vec<(&str, f64)> = day_agent_spend.into_iter().collect();
        day_per_agent.retain(|(_, spend)| *spend > 0.0);
        day_per_agent.sort_by(|a, b| b.1.total_cmp(&a.1));

        const TOP_N: usize = 4;
        let top_names: Vec<&str> = day_per_agent.iter().take(TOP_N).map(|(n, _)| *n).collect();

        // Per-hour breakdown: group by (hour, agent), then split into top-N + others.
        let mut hour_agent: Vec<HashMap<&str, f64>> = (0..24).map(|_| HashMap::new()).collect();
        for r in &rows {
            let h = r.hour as usize;
            if h < 24 {
                *hour_agent[h].entry(r.agent_name.as_str()).or_insert(0.0) += r.spend_usd;
            }
        }

        let hours: Vec<SpendHourPoint> = (0..24u8)
            .map(|h| {
                let agents = &hour_agent[h as usize];
                let total: f64 = agents.values().sum();
                let ha: Vec<AgentSpendSlice> = top_names
                    .iter()
                    .filter_map(|name| {
                        agents
                            .get(name)
                            .filter(|s| **s > 0.0)
                            .map(|s| AgentSpendSlice {
                                agent_name: resolve(name),
                                spend_usd: round6(*s),
                            })
                    })
                    .collect();
                let top_sum: f64 = top_names.iter().filter_map(|n| agents.get(n)).sum();
                SpendHourPoint {
                    hour: h,
                    spend_usd: round6(total),
                    top_agents: ha,
                    others_spend_usd: round6((total - top_sum).max(0.0) + 0.0),
                }
            })
            .collect();

        let avg = hours.iter().map(|h| h.spend_usd).sum::<f64>() / 24.0;

        // Day-level top agents summary.
        let top_agents: Vec<AgentSpendSlice> = day_per_agent
            .iter()
            .take(TOP_N)
            .map(|(name, spend)| AgentSpendSlice {
                agent_name: resolve(name),
                spend_usd: round6(*spend),
            })
            .collect();
        let others_sum: f64 = day_per_agent.iter().skip(TOP_N).map(|(_, s)| s).sum();
        let others_spend_usd = round6(others_sum) + 0.0;

        Ok(FinopsDayDrilldownResponse {
            data: FinopsDayDrilldown {
                date: date.to_string(),
                hours,
                avg_hourly_spend_usd: round6(avg),
                top_agents,
                others_spend_usd,
            },
            status_code: 200,
            message: "Day drilldown retrieved successfully".into(),
        })
    }

    // ── 6e. finops/attributions ───────────────────────────────────────────────

    /// Single Postgres GROUP BY, no Tempo calls — MAF already persists
    /// per-step cost/tokens (see `oss/orchestrator/src/maf`), so workflow
    /// attribution is naturally fast without any live trace aggregation.
    async fn get_workflow_finops_rows(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        agent_name: Option<&str>,
    ) -> Result<Vec<WorkflowFinopsRow>, ObservabilityError> {
        #[derive(sqlx::FromRow)]
        struct ExecRow {
            maf_id: Option<uuid::Uuid>,
            workflow_name: Option<String>,
            cost_usd: f64,
            duration_ms: Option<i64>,
            step_results: Option<serde_json::Value>,
        }

        let rows: Vec<ExecRow> = sqlx::query_as(
            r#"SELECT e.maf_id, m.name AS workflow_name, e.cost_usd, e.duration_ms, e.step_results
               FROM maf_executions e
               LEFT JOIN mafs m ON m.id = e.maf_id
               WHERE e.status = 'success'
                 AND e.started_at >= $1 AND e.started_at < $2
                 AND (
                     $3::text IS NULL
                     OR EXISTS (
                         SELECT 1 FROM jsonb_array_elements(e.step_results) elem
                         WHERE elem->>'agent_name' = $3
                     )
                 )"#,
        )
        .bind(start)
        .bind(end)
        .bind(agent_name)
        .fetch_all(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        #[derive(Default)]
        struct Acc {
            name: String,
            executions: usize,
            cost: f64,
            input_tokens: u64,
            output_tokens: u64,
            cache_read_tokens: u64,
            cache_creation_tokens: u64,
            latencies: Vec<f64>,
        }

        let get_step_i64 = |step: &serde_json::Value, key: &str| -> u64 {
            step.get(key).and_then(|v| v.as_i64()).unwrap_or(0).max(0) as u64
        };

        let mut by_workflow: HashMap<String, Acc> = HashMap::new();
        for row in rows {
            let Some(maf_id) = row.maf_id else { continue };
            let key = maf_id.to_string();
            let (mut input, mut output, mut cr, mut cc) = (0u64, 0u64, 0u64, 0u64);
            if let Some(steps) = row.step_results.as_ref().and_then(|v| v.as_array()) {
                for step in steps {
                    input += get_step_i64(step, "input_tokens");
                    output += get_step_i64(step, "output_tokens");
                    cr += get_step_i64(step, "cache_read_tokens");
                    cc += get_step_i64(step, "cache_creation_tokens");
                }
            }
            let acc = by_workflow.entry(key).or_insert_with(|| Acc {
                name: row
                    .workflow_name
                    .clone()
                    .unwrap_or_else(|| "(deleted workflow)".to_string()),
                ..Default::default()
            });
            acc.executions += 1;
            acc.cost += row.cost_usd;
            acc.input_tokens += input;
            acc.output_tokens += output;
            acc.cache_read_tokens += cr;
            acc.cache_creation_tokens += cc;
            if let Some(d) = row.duration_ms {
                acc.latencies.push(d as f64);
            }
        }

        let mut out: Vec<WorkflowFinopsRow> = by_workflow
            .into_iter()
            .map(|(maf_id, acc)| {
                let avg_cost = if acc.executions > 0 {
                    round6(acc.cost / acc.executions as f64)
                } else {
                    0.0
                };
                let avg_latency = if acc.latencies.is_empty() {
                    None
                } else {
                    Some(acc.latencies.iter().sum::<f64>() / acc.latencies.len() as f64)
                };
                WorkflowFinopsRow {
                    maf_id,
                    workflow_name: acc.name,
                    total_cost: round6(acc.cost),
                    executions: acc.executions,
                    avg_cost_per_execution: avg_cost,
                    prompt_tokens: acc.input_tokens,
                    completion_tokens: acc.output_tokens,
                    cache_read_tokens: acc.cache_read_tokens,
                    cache_creation_tokens: acc.cache_creation_tokens,
                    total_tokens: acc.input_tokens + acc.output_tokens,
                    avg_latency_ms: avg_latency,
                }
            })
            .collect();
        out.sort_by(|a, b| b.total_cost.total_cmp(&a.total_cost));
        Ok(out)
    }

    /// Standalone attributions endpoint — same per-view row sources as
    /// `get_finops_dashboard`'s `attributions` field, but with server-side
    /// sort/pagination so a table sort/page click doesn't re-run the
    /// KPI/timeseries work.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub async fn get_finops_attributions(
        &self,
        start_time: Option<&str>,
        end_time: Option<&str>,
        agent_name: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
        view: &str,
        sort_by: Option<&str>,
        sort_dir: Option<&str>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<FinopsAttributionsResponse, ObservabilityError> {
        let start = parse_iso_or_default(start_time, 30);
        let end = end_time
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        let desc = sort_dir.map(|d| d != "asc").unwrap_or(true);

        if view == "workflow" {
            let mut rows = self
                .get_workflow_finops_rows(start, end, agent_name)
                .await?;
            sort_workflow_rows(&mut rows, sort_by, desc);
            Ok(FinopsAttributionsResponse {
                data: FinopsAttributions::Workflow {
                    rows: paginate(rows, limit, offset),
                },
                status_code: 200,
                message: "Attributions retrieved successfully".into(),
            })
        } else {
            let dashboard = self
                .get_finops_dashboard(
                    "", None, None, None, start_time, end_time, agent_name, model, provider, None,
                    None, None, "agent",
                )
                .await?;
            let mut rows = dashboard.data.agents;
            sort_agent_rows(&mut rows, sort_by, desc);
            Ok(FinopsAttributionsResponse {
                data: FinopsAttributions::Agent {
                    rows: paginate(rows, limit, offset),
                },
                status_code: 200,
                message: "Attributions retrieved successfully".into(),
            })
        }
    }

    // ── 7. finops/insights ────────────────────────────────────────────────────

    pub async fn get_finops_insights(
        &self,
        payload: &InsightsRequest,
    ) -> Result<InsightsResponseEnvelope, ObservabilityError> {
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

        Ok(InsightsResponseEnvelope {
            data: InsightsData { insights },
            status_code: 200,
            message: "Insights generated successfully".into(),
        })
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
        accessible_agent_ids: Option<&[uuid::Uuid]>,
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

        let accessible: Option<HashSet<uuid::Uuid>> =
            accessible_agent_ids.map(|ids| ids.iter().copied().collect());
        let mut rows = hours_meter::windowed_agent_hours(&self.db, start, end, agent_filter)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?;
        if let Some(accessible) = &accessible {
            rows.retain(|row| accessible.contains(&row.agent_id));
        }

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
                let mut per_agent = hours_meter::windowed_hours_series_by_agent(
                    &self.db,
                    start,
                    end,
                    b,
                    agent_filter,
                )
                .await
                .map_err(|e| ObservabilityError::Internal(e.to_string()))?;
                if let Some(accessible) = &accessible {
                    per_agent.retain(|row| accessible.contains(&row.agent_id));
                }

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
                        .map(|row| {
                            let agents = by_bucket.remove(&row.bucket_start).unwrap_or_default();
                            let total_hours = if accessible.is_some() {
                                round6(agents.iter().map(|agent| agent.hours).sum())
                            } else {
                                round6(row.hours)
                            };
                            AgentHoursBucket {
                                start: fmt_ts(row.bucket_start),
                                total_hours,
                                agents,
                            }
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
                unpriced_calls: 0,
                estimated_cost: 0.0,
                unknown_confidence_calls: 0,
            },
            agents: vec![],
            token_usage: FinopsTokenUsage {
                total_tokens: 0,
                prompt_tokens: 0,
                completion_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                avg_tokens_per_operation: 0,
            },
            kpis: FinopsKpis {
                total_spend: KpiValue::new(0.0, 0.0),
                total_tokens: KpiValue::new(0.0, 0.0),
                cost_per_operation: KpiValue::new(0.0, 0.0),
                avg_latency_ms: KpiValue::new(0.0, 0.0),
                total_agents: KpiValue::new(0.0, 0.0),
                active_agents: KpiValue::new(0.0, 0.0),
                total_operations: KpiValue::new(0.0, 0.0),
                total_tool_calls: KpiValue::new(0.0, 0.0),
                latency_p95_ms: KpiValue::new(0.0, 0.0),
                latency_p99_ms: KpiValue::new(0.0, 0.0),
            },
            attributions: FinopsAttributions::Agent { rows: vec![] },
            spend_by_agent: SpendByAgentBreakdown {
                slices: vec![],
                total_spend_usd: 0.0,
            },
        },
        status_code: 200,
        message: "FinOps dashboard data retrieved successfully".into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use nasiko_observability::{CostBreakdown, SessionDetails, Span};

    use super::*;

    #[test]
    fn session_trace_window_is_anchored_to_historical_session() {
        let created = Utc.with_ymd_and_hms(2025, 1, 2, 3, 4, 5).unwrap();
        let updated = Utc.with_ymd_and_hms(2025, 1, 2, 4, 4, 5).unwrap();

        let (start, end) = session_trace_window(created, updated);

        assert_eq!(start, Utc.with_ymd_and_hms(2025, 1, 2, 2, 59, 5).unwrap());
        assert_eq!(end, Utc.with_ymd_and_hms(2025, 1, 2, 4, 9, 5).unwrap());
    }

    #[test]
    fn span_tree_total_includes_cache_classes() {
        let mut attributes = HashMap::new();
        attributes.insert("gen_ai.usage.input_tokens".into(), serde_json::json!(10));
        attributes.insert("gen_ai.usage.output_tokens".into(), serde_json::json!(5));
        attributes.insert(
            "gen_ai.usage.cache_read_input_tokens".into(),
            serde_json::json!(2),
        );
        attributes.insert(
            "gen_ai.usage.cache_creation_input_tokens".into(),
            serde_json::json!(3),
        );
        let child = Span {
            span_id: "model".into(),
            parent_span_id: Some("root".into()),
            name: "chat model".into(),
            started_at: Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap(),
            ended_at: None,
            duration_ms: None,
            service_name: "agent".into(),
            kind: 3,
            status_code: 0,
            status_message: String::new(),
            attributes,
            events: vec![],
        };
        let root = Span {
            span_id: "root".into(),
            parent_span_id: None,
            name: "coding_agent.turn".into(),
            started_at: child.started_at,
            ended_at: None,
            duration_ms: None,
            service_name: "agent".into(),
            kind: 1,
            status_code: 0,
            status_message: String::new(),
            attributes: HashMap::new(),
            events: vec![],
        };

        let (roots, _) = build_span_tree(&[root, child]);
        // `input_tokens: 10` is the whole prompt, of which 5 were cached (2 read + 3
        // written) — the semconv reading, which `extract_usage_attrs` normalizes to a
        // fresh count of 5. The total is therefore 5 fresh + 5 cached + 5 output = 15,
        // not 20: summing the raw attribute alongside the cache classes would charge
        // every cached token twice.
        assert_eq!(roots[0].token_count_total, 15);
        assert_eq!(roots[0].cache_read_tokens, 2);
        assert_eq!(roots[0].cache_creation_tokens, 3);
        assert_eq!(roots[0].children[0].token_count_total, 15);
    }

    #[test]
    fn tool_span_name_includes_a_bounded_argument_summary() {
        let mut attributes = HashMap::new();
        attributes.insert(
            "gen_ai.operation.name".into(),
            serde_json::json!("execute_tool"),
        );
        attributes.insert("tool.name".into(), serde_json::json!("Bash"));
        attributes.insert(
            "tool.arguments".into(),
            serde_json::json!(r#"{"command":"git status --short"}"#),
        );
        let span = Span {
            span_id: "tool".into(),
            parent_span_id: None,
            name: "execute_tool Bash".into(),
            started_at: Utc.with_ymd_and_hms(2026, 8, 27, 12, 0, 0).unwrap(),
            ended_at: None,
            duration_ms: Some(4),
            service_name: "agent".into(),
            kind: 1,
            status_code: 1,
            status_message: String::new(),
            attributes,
            events: vec![],
        };

        let (roots, _) = build_span_tree(&[span]);
        assert_eq!(roots[0].name, "Bash: git status --short");
    }

    #[test]
    fn session_summary_total_and_cost_include_cache() {
        let details = SessionDetails {
            session_id: "session".into(),
            traces: vec![],
            trace_count: 1,
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 2,
            cache_creation_tokens: 3,
            model_used: Some("claude-sonnet-4".into()),
            latency_ms_p50: None,
            latency_ms_p99: None,
            latency_ms_avg: None,
            has_more_traces: false,
            metrics_complete: true,
            cost: CostBreakdown {
                prompt_usd: 1.0,
                completion_usd: 2.0,
                cache_read_usd: 0.25,
                cache_creation_usd: 0.75,
                total_usd: 4.0,
                estimated: false,
            },
        };

        let summary = ObservabilityService::session_summary_from_traces(
            "session".into(),
            "agent".into(),
            &details,
        );
        assert_eq!(summary.token_usage.total, Some(20));
        assert_eq!(summary.cost_summary.total.cost, Some(4.0));
    }

    // ── KpiValue::new ────────────────────────────────────────────────────────

    #[test]
    fn kpi_value_change_pct_is_none_when_previous_is_zero() {
        let kpi = KpiValue::new(500.0, 0.0);
        assert_eq!(kpi.current, 500.0);
        assert_eq!(kpi.previous, 0.0);
        assert_eq!(
            kpi.change_pct, None,
            "0 -> N is an undefined %, not +inf or 0"
        );
    }

    #[test]
    fn kpi_value_change_pct_is_none_when_both_are_zero() {
        let kpi = KpiValue::new(0.0, 0.0);
        assert_eq!(kpi.change_pct, None);
    }

    #[test]
    fn kpi_value_change_pct_positive_increase() {
        let kpi = KpiValue::new(150.0, 100.0);
        assert_eq!(kpi.change_pct, Some(50.0));
    }

    #[test]
    fn kpi_value_change_pct_negative_decrease() {
        let kpi = KpiValue::new(50.0, 100.0);
        assert_eq!(kpi.change_pct, Some(-50.0));
    }

    #[test]
    fn kpi_value_change_pct_no_change_is_exactly_zero_not_none() {
        let kpi = KpiValue::new(100.0, 100.0);
        assert_eq!(
            kpi.change_pct,
            Some(0.0),
            "unchanged-but-nonzero must be Some(0.0), distinct from the undefined-previous None case"
        );
    }

    #[test]
    fn kpi_value_change_pct_rounds_to_two_decimal_places() {
        // 1/3 * 100 = 33.333...% -> must round to 33.33, not truncate or
        // carry extra float noise into the serialized response.
        let kpi = KpiValue::new(4.0, 3.0);
        assert_eq!(kpi.change_pct, Some(33.33));
    }

    #[test]
    fn kpi_value_change_pct_handles_a_full_wipeout_to_zero() {
        let kpi = KpiValue::new(0.0, 100.0);
        assert_eq!(kpi.change_pct, Some(-100.0));
    }

    // ── range_hours / bucket_label / resolve_window ─────────────────────────

    #[test]
    fn range_hours_known_values() {
        assert_eq!(range_hours("24h"), Some(24));
        assert_eq!(range_hours("7d"), Some(168));
        assert_eq!(range_hours("30d"), Some(720));
    }

    #[test]
    fn range_hours_rejects_unknown_values() {
        assert_eq!(range_hours("1h"), None);
        assert_eq!(range_hours(""), None);
        assert_eq!(range_hours("7D"), None, "case-sensitive, not normalized");
    }

    #[test]
    fn bucket_label_matches_enum_variant() {
        assert_eq!(bucket_label(TimeBucket::Hour), "hour");
        assert_eq!(bucket_label(TimeBucket::Day), "day");
    }

    #[test]
    fn resolve_window_24h_range_uses_hour_bucket() {
        let (start, end, bucket) = resolve_window(None, None, Some("24h")).unwrap();
        assert_eq!(bucket, TimeBucket::Hour);
        assert_eq!((end - start).num_hours(), 24);
    }

    #[test]
    fn resolve_window_7d_and_30d_range_use_day_bucket() {
        let (start7, end7, bucket7) = resolve_window(None, None, Some("7d")).unwrap();
        assert_eq!(bucket7, TimeBucket::Day);
        assert_eq!((end7 - start7).num_hours(), 168);

        let (start30, end30, bucket30) = resolve_window(None, None, Some("30d")).unwrap();
        assert_eq!(bucket30, TimeBucket::Day);
        assert_eq!((end30 - start30).num_hours(), 720);
    }

    #[test]
    fn resolve_window_range_wins_over_start_time_when_both_given() {
        // A stale/irrelevant start_time must be ignored once `range` is set —
        // this is the documented precedence, not an arbitrary choice.
        let stale_start = "2000-01-01T00:00:00Z";
        let (start, end, _) = resolve_window(Some(stale_start), None, Some("24h")).unwrap();
        assert_eq!((end - start).num_hours(), 24);
        assert!(start.to_rfc3339() != stale_start);
    }

    #[test]
    fn resolve_window_rejects_an_invalid_range() {
        let err = resolve_window(None, None, Some("bogus")).unwrap_err();
        assert!(matches!(err, ObservabilityError::BadRequest(_)));
    }

    #[test]
    fn resolve_window_no_range_falls_back_to_30_day_default_with_day_bucket() {
        let (start, end, bucket) = resolve_window(None, None, None).unwrap();
        assert_eq!(bucket, TimeBucket::Day);
        // parse_iso_or_default(None, 30) — allow a little slack for wall-clock
        // drift between `Utc::now()` calls in the test and in the function.
        let hours = (end - start).num_hours();
        assert!(
            (719..=721).contains(&hours),
            "expected ~30 days, got {hours}h"
        );
    }

    #[test]
    fn resolve_window_end_time_override_is_respected() {
        let end_time = "2024-06-15T12:00:00Z";
        let (_, end, _) = resolve_window(None, Some(end_time), Some("24h")).unwrap();
        assert_eq!(end.to_rfc3339(), "2024-06-15T12:00:00+00:00");
    }

    // ── sort_agent_rows / sort_workflow_rows / paginate ─────────────────────

    fn agent_row(
        name: &str,
        cost: f64,
        tokens: u64,
        ops: usize,
        latency: Option<f64>,
    ) -> AgentFinopsRow {
        AgentFinopsRow {
            agent_id: format!("id-{name}"),
            agent_name: name.to_string(),
            total_cost: cost,
            operations: ops,
            is_capped: false,
            avg_cost_per_operation: 0.0,
            prompt_tokens: tokens / 2,
            completion_tokens: tokens - tokens / 2,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            total_tokens: tokens,
            avg_latency_ms: latency,
            avg_latency_p95_ms: None,
            avg_latency_p99_ms: None,
            tool_call_count: 0,
            version: None,
            container_hours: 0.0,
        }
    }

    fn workflow_row(name: &str, cost: f64, tokens: u64, executions: usize) -> WorkflowFinopsRow {
        WorkflowFinopsRow {
            maf_id: format!("maf-{name}"),
            workflow_name: name.to_string(),
            total_cost: cost,
            executions,
            avg_cost_per_execution: 0.0,
            prompt_tokens: tokens / 2,
            completion_tokens: tokens - tokens / 2,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            total_tokens: tokens,
            avg_latency_ms: None,
        }
    }

    #[test]
    fn sort_agent_rows_default_is_cost_descending() {
        let mut rows = vec![
            agent_row("cheap", 1.0, 100, 1, Some(10.0)),
            agent_row("expensive", 100.0, 100, 1, Some(10.0)),
            agent_row("mid", 50.0, 100, 1, Some(10.0)),
        ];
        sort_agent_rows(&mut rows, None, true);
        let names: Vec<_> = rows.iter().map(|r| r.agent_name.as_str()).collect();
        assert_eq!(names, vec!["expensive", "mid", "cheap"]);
    }

    #[test]
    fn sort_agent_rows_ascending_flips_the_default() {
        let mut rows = vec![
            agent_row("cheap", 1.0, 100, 1, None),
            agent_row("expensive", 100.0, 100, 1, None),
        ];
        sort_agent_rows(&mut rows, None, false);
        let names: Vec<_> = rows.iter().map(|r| r.agent_name.as_str()).collect();
        assert_eq!(names, vec!["cheap", "expensive"]);
    }

    #[test]
    fn sort_agent_rows_by_tokens_operations_latency_hours_and_name() {
        let mut by_tokens = vec![
            agent_row("a", 0.0, 500, 1, None),
            agent_row("b", 0.0, 10, 1, None),
        ];
        sort_agent_rows(&mut by_tokens, Some("tokens"), true);
        assert_eq!(by_tokens[0].agent_name, "a");

        let mut by_ops = vec![
            agent_row("few-ops", 0.0, 0, 1, None),
            agent_row("many-ops", 0.0, 0, 99, None),
        ];
        sort_agent_rows(&mut by_ops, Some("operations"), true);
        assert_eq!(by_ops[0].agent_name, "many-ops");

        let mut by_latency = vec![
            agent_row("slow", 0.0, 0, 0, Some(900.0)),
            agent_row("fast", 0.0, 0, 0, Some(10.0)),
            agent_row("unknown-latency", 0.0, 0, 0, None),
        ];
        sort_agent_rows(&mut by_latency, Some("avg_latency"), true);
        assert_eq!(
            by_latency[0].agent_name, "slow",
            "None latency must sort as if it were 0, landing last in descending order"
        );

        let mut by_name = vec![
            agent_row("zeta", 0.0, 0, 0, None),
            agent_row("alpha", 0.0, 0, 0, None),
        ];
        sort_agent_rows(&mut by_name, Some("name"), false);
        assert_eq!(
            by_name
                .iter()
                .map(|r| r.agent_name.clone())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );

        let mut unknown_key = vec![
            agent_row("cheap", 1.0, 0, 0, None),
            agent_row("pricey", 9.0, 0, 0, None),
        ];
        sort_agent_rows(&mut unknown_key, Some("not-a-real-column"), true);
        assert_eq!(
            unknown_key[0].agent_name, "pricey",
            "an unrecognized sort_by must fall back to cost, not panic or no-op"
        );
    }

    #[test]
    fn sort_workflow_rows_default_and_by_name() {
        let mut rows = vec![
            workflow_row("cheap-flow", 1.0, 0, 0),
            workflow_row("pricey-flow", 50.0, 0, 0),
        ];
        sort_workflow_rows(&mut rows, None, true);
        assert_eq!(rows[0].workflow_name, "pricey-flow");

        let mut by_name = vec![
            workflow_row("zeta", 0.0, 0, 0),
            workflow_row("alpha", 0.0, 0, 0),
        ];
        sort_workflow_rows(&mut by_name, Some("name"), false);
        assert_eq!(by_name[0].workflow_name, "alpha");
    }

    #[test]
    fn paginate_applies_offset_then_limit() {
        let rows: Vec<i32> = (0..10).collect();
        assert_eq!(paginate(rows.clone(), Some(3), Some(2)), vec![2, 3, 4]);
        assert_eq!(paginate(rows.clone(), None, None), rows);
        assert_eq!(
            paginate(rows.clone(), Some(100), Some(0)),
            rows,
            "limit beyond len returns everything"
        );
        assert_eq!(
            paginate(rows.clone(), Some(3), Some(50)),
            Vec::<i32>::new(),
            "offset beyond len returns empty, not a panic"
        );
        assert_eq!(
            paginate(rows.clone(), Some(-1), Some(0)),
            rows,
            "a negative limit is treated as unset, not zero-truncated"
        );
        assert_eq!(
            paginate(rows, Some(0), Some(0)),
            Vec::<i32>::new(),
            "limit 0 truly means zero rows"
        );
    }
}
