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
use nasiko_config::Config;
use nasiko_event_store::flows::FlowRow;
use nasiko_event_store::trace_usage::TraceUsageRow;
use nasiko_observability::{
    CostBreakdown, ObservabilityError, ObservabilityProvider, TimeBucket, extract_token_attrs,
    extract_usage_attrs,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::agents::hours_meter;
use crate::agents::hours_meter::sandbox as sandbox_meter;

// ─── Session listing tuning ───────────────────────────────────────────────────

/// Sessions per page when the caller doesn't ask for a size. A page is now one
/// indexed SQL read, so this is a display preference rather than the work bound
/// it was while every row cost a trace-store lookup.
const DEFAULT_SESSION_PAGE: i64 = 25;

// ─── Presentation helpers ─────────────────────────────────────────────────────

/// Format a timestamp as RFC 3339 with millisecond precision.
///
/// Tempo stores span timestamps at nanosecond precision; Dart's DateTime.parse
/// only handles up to microseconds. Capping at millis is safe for all consumers.
pub(super) fn fmt_ts(dt: DateTime<Utc>) -> String {
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

/// A span's status as the trace view reports it. An agent state read that found nothing
/// ([`nasiko_observability::types::is_agent_state_miss`]) is `UNSET`, not `ERROR`: the UI
/// marks a trace failing on any unrecovered ERROR span, and this 404 is an answer.
fn span_status_str(span: &nasiko_observability::Span) -> &'static str {
    if nasiko_observability::types::is_agent_state_miss(span) {
        return "UNSET";
    }
    status_code_str(span.status_code)
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
        "sandbox_minutes" => rows.sort_by(|a, b| a.sandbox_minutes.total_cmp(&b.sandbox_minutes)),
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

/// `model` is the model that answered the span's call, when it has one.
fn span_display_name(span: &nasiko_observability::Span, model: Option<&str>) -> String {
    let operation = span
        .attributes
        .get("gen_ai.operation.name")
        .and_then(Value::as_str);
    if operation != Some("execute_tool") {
        // GenAI semconv names an LLM span `{operation} {request model}`. Through the LLM
        // gateway the agent's routing config picks the model, not the request (often just
        // the platform's injected `OPENAI_MODEL` default), so name the one that answered.
        let requested = span
            .attributes
            .get("gen_ai.request.model")
            .and_then(Value::as_str);
        if let (Some(op), Some(requested), Some(model)) = (operation, requested, model)
            && model != requested
            && span.name == format!("{op} {requested}")
        {
            return format!("{op} {model}");
        }
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

/// The model, and the provider when known, that answered an agent's LLM call.
#[derive(Debug, Clone)]
struct Answer {
    model: String,
    provider: Option<String>,
}

/// The answer for each LLM span whose own attributes do not say what actually ran.
///
/// An agent's call through the LLM gateway nests the gateway's `gen_ai.chat` span, whose
/// `gen_ai.response.model` is the model the agent's routing config picked, under the
/// agent's own LLM span. The Rust and Go agents' manual spans record just the requested
/// model, so without this they show the platform's `OPENAI_MODEL` default. The gateway's
/// own span ([`nasiko_observability::types::is_router_llm_span`]) also overrides an agent
/// span that records a response model: the OpenAI SDK the agent called names the provider
/// `openai` whichever provider the gateway routed to, and only the gateway's span knows.
fn answered_models(spans: &[nasiko_observability::Span]) -> HashMap<&str, Answer> {
    let by_id: HashMap<&str, &nasiko_observability::Span> =
        spans.iter().map(|s| (s.span_id.as_str(), s)).collect();
    let parent_of = |s: &nasiko_observability::Span| {
        s.parent_span_id
            .as_deref()
            .and_then(|id| by_id.get(id))
            .copied()
    };
    let mut answered: HashMap<&str, (Answer, bool)> = HashMap::new();
    for span in spans {
        let Some(model) = first_str_attr(&span.attributes, &["gen_ai.response.model"]) else {
            continue;
        };
        // Bounded by the span count, so a malformed parent cycle can't loop forever.
        let nearest_llm = std::iter::successors(parent_of(span), |s| parent_of(s))
            .take(spans.len())
            .find(|s| is_llm_call(s));
        let Some(llm) = nearest_llm else {
            continue;
        };
        let from_router = nasiko_observability::types::is_router_llm_span(span);
        if !from_router && llm.attributes.contains_key("gen_ai.response.model") {
            continue;
        }
        let answer = Answer {
            model,
            provider: first_str_attr(&span.attributes, &["gen_ai.provider.name", "gen_ai.system"]),
        };
        match answered.get(llm.span_id.as_str()) {
            Some((_, true)) => {}
            Some((_, false)) if !from_router => {}
            _ => {
                answered.insert(llm.span_id.as_str(), (answer, from_router));
            }
        }
    }
    answered
        .into_iter()
        .map(|(id, (answer, _))| (id, answer))
        .collect()
}

/// Whether the span is a model call, not just a span that names a model: an agent's
/// `invoke_agent` span often carries `gen_ai.request.model` too, and must not take a
/// gateway answer meant for one of the calls under it.
fn is_llm_call(span: &nasiko_observability::Span) -> bool {
    let attr = |key| span.attributes.get(key).and_then(Value::as_str);
    if let Some(operation) = attr("gen_ai.operation.name") {
        return matches!(operation, "chat" | "text_completion" | "generate_content");
    }
    if attr("openinference.span.kind").is_some_and(|k| k.eq_ignore_ascii_case("llm")) {
        return true;
    }
    let (input, output, model) = extract_token_attrs(&span.attributes);
    model.is_some() && (input > 0 || output > 0)
}

fn build_span_tree(
    spans: &[nasiko_observability::Span],
) -> (Vec<SpanNode>, HashMap<String, SpanNode>) {
    let answered = answered_models(spans);
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
            let model = answered
                .get(s.span_id.as_str())
                .map(|a| a.model.clone())
                .or(u.model);
            (u.input, u.output, model, u.cache_read, u.cache_creation)
        };
        SpanNode {
            id: encode_span_id(&s.span_id),
            span_id: s.span_id.clone(),
            name: span_display_name(s, model.as_deref()),
            span_kind: span_kind_str(s.kind).to_string(),
            status_code: span_status_str(s).to_string(),
            status_message: (!s.status_message.is_empty()).then(|| s.status_message.clone()),
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
                &[
                    "gen_ai.operation.name",
                    "rpc.method",
                    "code.function",
                    "http.route",
                ],
            ),
            provider: answered
                .get(s.span_id.as_str())
                .and_then(|a| a.provider.clone())
                .or_else(|| {
                    first_str_attr(&s.attributes, &["gen_ai.system", "gen_ai.provider.name"])
                }),
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
    /// Rows matching the filters, so a caller can say "3 failed sessions" from
    /// one request instead of counting a sampled page. Exact when `status`
    /// narrows the set; otherwise capped (see `sessions_sql`).
    pub matching_count: i64,
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
    /// `running` | `ok` | `failed` | `timed_out`, derived from the session's flow rows — the
    /// outcome recorded at the proxy. `null` when the session has no recorded
    /// flow, which is "unknown", never "ok".
    pub status: Option<String>,
    /// Requests in this session that ended in an error.
    pub failed_flows: u32,
    /// Requests the proxy recorded for this session. `0` for a session that
    /// never crossed it, which has no flow to open.
    pub flow_count: u32,
    /// Unrecovered error spans across the session's traces (the SE-3 rule).
    /// `null` while none of them has been materialized yet -- "not yet known",
    /// never "zero".
    pub error_count: Option<u32>,
    /// The session that spawned this one, when it is a subagent; `null` at the top level.
    pub parent_session_id: Option<String>,
    /// The parent's tool call that spawned this session.
    pub spawn_tool_call_id: Option<String>,
    /// The subagent's kind as the coding agent names it (e.g. "Explore").
    pub subagent_type: Option<String>,
    /// Sessions this one spawned.
    pub subagent_count: u32,
    /// Those sessions' own totals. Never included in this row's own
    /// `num_traces`, `token_usage` or `cost_summary`.
    pub subagents: super::subagents_sql::SubagentTotals,
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
    /// The session that spawned this one, when it is a subagent.
    pub parent_session_id: Option<String>,
    /// The parent's tool call that spawned this session.
    pub spawn_tool_call_id: Option<String>,
    /// The subagent's kind as the coding agent names it (e.g. "Explore").
    pub subagent_type: Option<String>,
    /// Sessions this one spawned, oldest first, each with its own totals —
    /// never included in this session's own figures.
    pub subagents: Vec<super::subagents_sql::SubagentRun>,
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
    /// Absent when it cannot be narrowed to the caller's own calls (see `get_agent_stats`), so a
    /// missing figure never reads as a real $0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<CostOnly>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation: Option<CostOnly>,
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
    /// The span's status description, when it has one — lets the UI tell the a2a SDK's
    /// end-of-stream `QueueShutDown` from a real failure, exactly as the server does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
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
    /// Who spent it: one row per user in the window, largest spender first, under the same
    /// filters as everything above. A caller who may not see other users gets only their own
    /// row (`handler::get_finops_dashboard`).
    pub users: Vec<UserFinopsRow>,
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
    /// Sandbox-minutes (Running time) consumed in the dashboard window, deleted agents
    /// included. Container-hours carry no price here, so sandbox time is reported in minutes
    /// only and is not part of `total_cost`. `finops/insights` does not carry it: that endpoint
    /// only relays client-supplied figures to the model.
    pub total_sandbox_minutes: f64,
    /// Calls in the window that consumed tokens with no price row for the model
    /// (e.g. a custom provider with no price book): they carry no cost, or a cost
    /// at the default rate (`metadata.pricing.source = 'Default'`). Either way
    /// `total_cost` is not a looked-up figure for them; this surfaces the gap as a
    /// known number.
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
    /// Sandbox-minutes this agent consumed in the dashboard window.
    pub sandbox_minutes: f64,
    /// This agent's figures for the preceding window of the same length, present
    /// only with `compare=previous`.
    ///
    /// The previous window is already queried on every dashboard call (the KPI
    /// strip's `change_pct` needs it) and was simply discarded per agent, so a
    /// caller that wanted per-agent comparison had to issue a second whole
    /// dashboard request for the earlier window. Omitted by default so the
    /// default payload is byte-identical to before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<AgentFinopsPrevious>,
}

/// One agent's preceding-window totals. Deliberately a subset: enough to compute
/// a delta, not a second full row.
#[derive(Serialize, Clone, ToSchema)]
pub struct AgentFinopsPrevious {
    pub total_cost: f64,
    pub operations: usize,
    pub total_tokens: u64,
    pub tool_call_count: u64,
    pub avg_latency_ms: Option<f64>,
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
    /// Per-agent series, present only with `group_by=agent`. The top `top`
    /// agents by spend over the whole window, plus one `other` series carrying
    /// everything below them, so the series still sum to `points`.
    ///
    /// Without this a caller charting the top N drivers had to issue one
    /// `agent_id=`-filtered request per driver — a fan-out whose size grew with
    /// the chart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series: Option<Vec<SpendAgentSeries>>,
    /// The preceding window of the same length, present only with
    /// `compare=previous`. Same bucketing, so points line up index-for-index
    /// once both are zero-filled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<Vec<SpendTimeseriesPoint>>,
}

/// One agent's spend over the window's buckets.
#[derive(Serialize, ToSchema)]
pub struct SpendAgentSeries {
    /// Raw trace name, matching `top_agent_name` on the fleet points. `None` for
    /// the aggregate `other` series.
    pub agent_name: Option<String>,
    /// Resolved agent UUID when exactly one live agent carries that name, so
    /// callers key on the id rather than the name (TK-3, H-5).
    pub agent_id: Option<String>,
    /// True for the single aggregate series holding every agent outside the top.
    pub is_other: bool,
    pub total_spend_usd: f64,
    pub points: Vec<SpendAgentPoint>,
}

#[derive(Serialize, ToSchema)]
pub struct SpendAgentPoint {
    pub bucket_start: String,
    pub spend_usd: f64,
    pub operations: usize,
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

/// One user's usage in the dashboard window.
#[derive(Serialize, Clone, Debug, ToSchema, sqlx::FromRow)]
pub struct UserFinopsRow {
    /// `None` for calls no user made: the platform's own routing and planning.
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub email: Option<String>,
    pub total_cost: f64,
    pub operations: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    /// Prompt, completion and both cache counts, as the dashboard's `token_usage` counts them.
    pub total_tokens: i64,
    pub tool_call_count: i64,
    /// How many agents the user's calls went to.
    pub agents: i64,
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
    store: nasiko_event_store::EventStore,
    /// In-process LLM router; serves insights when the user saved no OpenAI key.
    llm_backend: nasiko_orchestrator::llm_backend::SharedLlmBackend,
}

/// `trace_usage.agent_name` for finops insight calls.
const INSIGHTS_COMPONENT: &str =
    nasiko_orchestrator::llm_backend::PlatformComponent::FinopsInsights.as_str();

#[derive(Debug, PartialEq, Eq)]
pub enum EnsureSessionOutcome {
    Created,
    Existing,
    Conflict,
}

/// One turn's token/cost usage within a chat session, from the authoritative
/// billing ledger (`token_usage` joined to `flows` — one flow == one turn).
/// Independent of Tempo/the materializer.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct TurnUsage {
    /// The user's message for this turn (`flows.title`), when known.
    pub title: Option<String>,
    /// The turn's flow id — what `token_usage.session_id` actually stores.
    pub flow_id: String,
    /// LLM model calls the agent made in this turn.
    pub calls: i64,
    pub tokens: i64,
    pub cost_usd: f64,
    pub turn_start: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, serde::Serialize)]
pub struct SessionTurnsResponse {
    pub turns: Vec<TurnUsage>,
}

#[derive(Clone, Copy, Default)]
pub struct FinopsAccess<'a> {
    pub user_ids: Option<&'a [uuid::Uuid]>,
    pub accessible_agent_ids: Option<&'a [uuid::Uuid]>,
    pub workflow_owner: Option<uuid::Uuid>,
}

struct ScopeBinds {
    agent_names: Option<Vec<String>>,
    has_user_filter: bool,
    user_ids: Vec<uuid::Uuid>,
}

/// The agents a dashboard-shaped FinOps view covers, and the SQL scope that keeps its queries to
/// them. One answer for the dashboard and [`ObservabilityService::get_finops_spend_by_user_agent`],
/// so a breakdown of the dashboard's spend always adds up to the dashboard's own total.
struct AgentsInView {
    /// `(id, name, display_name, version)` of every live agent in view.
    agents: Vec<(uuid::Uuid, String, String, String)>,
    /// The agents the caller may see, or `None` for every agent.
    accessible: Option<HashSet<uuid::Uuid>>,
    /// Internal agents left out because the caller has no other agent in view.
    dropped_internal: HashSet<uuid::Uuid>,
    /// A caller who sees only some agents aggregates only their rows, in SQL.
    /// Narrowing afterwards made every member's dashboard cost as much as the
    /// whole organization's history, and the summary figures summed straight
    /// from the rows (unpriced, estimated, unknown-confidence) counted other
    /// users' spend. `None`: the caller sees every agent.
    scoped_names: Option<Vec<String>>,
    own_platform_user: Option<uuid::Uuid>,
    /// Whether the platform's own rows count toward the view's totals: all of them for a caller
    /// who sees the whole fleet, the caller's own for a scoped one.
    platform_rows: bool,
}

/// One user's spend on one agent in a FinOps window
/// ([`ObservabilityService::get_finops_spend_by_user_agent`]).
#[derive(Debug, Clone)]
pub struct UserAgentSpend {
    /// `None`: the call carried no user.
    pub user_id: Option<uuid::Uuid>,
    /// The registered agent, or `None` for the platform's own calls (orchestrator, MAF).
    pub agent_id: Option<uuid::Uuid>,
    /// The agent's display name, or the platform row's name.
    pub agent_name: String,
    pub total_cost: f64,
    pub operations: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub tool_call_count: i64,
}

/// The rows of [`ObservabilityService::get_finops_spend_by_user_agent`] and the window they cover.
#[derive(Debug, Clone)]
pub struct FinopsUserAgentSpend {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub rows: Vec<UserAgentSpend>,
}

impl ObservabilityService {
    /// Per-turn tokens + cost for a chat session (its A2A contextId), read from
    /// the `token_usage` ledger joined to `flows`, grouped by flow (one row per
    /// turn). `title` is drawn from the turn's own prompt (`flows.title`), so this is
    /// conversation content, not an aggregate: scoped to the session's own user,
    /// never a superuser bypass — same rule as `authorize_session`. This is the
    /// authoritative per-turn view and does not depend on the Tempo materializer.
    pub async fn get_session_turns(
        &self,
        context_id: &str,
        user_id: &str,
    ) -> Result<SessionTurnsResponse, ObservabilityError> {
        let turns = sqlx::query_as::<_, TurnUsage>(
            r#"SELECT f.title                         AS title,
                      t.session_id                    AS flow_id,
                      sum(t.calls)::bigint            AS calls,
                      sum(t.total_tokens)::bigint     AS tokens,
                      round(sum(t.cost_usd), 6)::float8 AS cost_usd,
                      min(t.created_at)               AS turn_start
               -- Hot trace_usage rows plus per-trace totals of tiered rows (trace_totals),
               -- counted the same way on both sides: one call per row, tokens including
               -- both cache counts, so tiering a turn never changes its figures.
               FROM (SELECT trace_id AS session_id, started_at AS created_at, 1::bigint AS calls,
                            (input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens)::bigint
                                AS total_tokens,
                            cost_usd::numeric AS cost_usd
                       FROM trace_usage
                     UNION ALL
                     SELECT trace_id, first_at, row_count, tokens, cost_usd::numeric
                       FROM trace_totals) t
               JOIN flows f ON f.flow_id = t.session_id
               WHERE f.session_id = $1
                 AND f.user_id = $2::uuid
               GROUP BY f.flow_id, f.title, t.session_id
               ORDER BY turn_start"#,
        )
        .bind(context_id)
        .bind(user_id)
        .fetch_all(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;
        Ok(SessionTurnsResponse { turns })
    }

    pub fn from_state(state: &crate::state::AppState) -> Self {
        Self {
            provider: state.observability.clone(),
            db: state.db.clone(),
            http_client: state.http_client.clone(),
            config: state.config.clone(),
            store: state.event_store(),
            llm_backend: Arc::clone(&state.llm_backend),
        }
    }

    /// Returns Vec<(id, name, display_name, version)> for all agents in the DB. `name`
    /// doubles as the Tempo `service.name` (the injector sets OTEL_SERVICE_NAME
    /// to the agent name); `id` is the UUID reported to callers.
    /// OSS: returns all agents (NoopAuthorizer). EE adds RBAC at a higher layer.
    async fn scope_binds(
        &self,
        access: FinopsAccess<'_>,
    ) -> Result<ScopeBinds, ObservabilityError> {
        let agent_names = match access.accessible_agent_ids {
            None => None,
            Some(ids) => Some(
                sqlx::query_scalar::<_, String>(
                    "SELECT name FROM agents WHERE id = ANY($1) AND deleted_at IS NULL",
                )
                .bind(ids.to_vec())
                .fetch_all(&self.db)
                .await
                .map_err(|e| ObservabilityError::Internal(e.to_string()))?,
            ),
        };
        let user_ids = access
            .user_ids
            .filter(|ids| !ids.is_empty())
            .map(<[uuid::Uuid]>::to_vec)
            .unwrap_or_default();
        Ok(ScopeBinds {
            agent_names,
            has_user_filter: !user_ids.is_empty(),
            user_ids,
        })
    }

    /// The agents a dashboard-shaped view covers: live agents, narrowed to one by `agent_name`,
    /// to the caller's own by `owner_id`, and to those the caller may see.
    async fn agents_in_view(
        &self,
        caller: &str,
        agent_name: Option<&str>,
        accessible_agent_ids: Option<&[uuid::Uuid]>,
        owner_id: Option<uuid::Uuid>,
    ) -> Result<AgentsInView, ObservabilityError> {
        let accessible: Option<HashSet<uuid::Uuid>> =
            accessible_agent_ids.map(|ids| ids.iter().copied().collect());
        let own_platform_user = own_platform_rows_user(
            accessible.is_some(),
            owner_id.is_some() || agent_name.is_some(),
            caller,
        );
        let all_agents = self.get_agent_names(owner_id).await?;
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
        // in the dashboard turns into the zeroed payload those pages expect. Once
        // the caller has an agent of their own the internal one stays in, so its
        // spend is still attributed.
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
        let scoped_names = (accessible.is_some() || owner_id.is_some())
            .then(|| agents.iter().map(|(_, name, _, _)| name.clone()).collect());
        let fleet_view = accessible.is_none() && owner_id.is_none() && agent_name.is_none();
        Ok(AgentsInView {
            agents,
            accessible,
            dropped_internal,
            scoped_names,
            own_platform_user,
            platform_rows: fleet_view || own_platform_user.is_some(),
        })
    }

    /// Spend per user per agent: the dashboard's totals, under the same filters and agent scope,
    /// broken down by who made each call. `access.user_ids` narrows it to those users, and an
    /// empty list means nobody, not everybody.
    #[allow(clippy::too_many_arguments)]
    pub async fn get_finops_spend_by_user_agent(
        &self,
        caller: &str,
        start_time: Option<&str>,
        end_time: Option<&str>,
        agent_name: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
        access: FinopsAccess<'_>,
    ) -> Result<FinopsUserAgentSpend, ObservabilityError> {
        let start = parse_iso_or_default(start_time, 30);
        let end = end_time
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let nothing = FinopsUserAgentSpend {
            start,
            end,
            rows: vec![],
        };
        if access.user_ids.is_some_and(<[uuid::Uuid]>::is_empty) {
            return Ok(nothing);
        }
        let view = self
            .agents_in_view(caller, agent_name, access.accessible_agent_ids, None)
            .await?;
        if view.agents.is_empty() && view.own_platform_user.is_none() {
            return Ok(nothing);
        }
        let user_ids: Vec<uuid::Uuid> = access.user_ids.map(<[_]>::to_vec).unwrap_or_default();

        #[derive(sqlx::FromRow)]
        struct Row {
            user_id: Option<uuid::Uuid>,
            agent_name: String,
            operations: i64,
            input_tokens: i64,
            output_tokens: i64,
            cache_read_tokens: i64,
            cache_creation_tokens: i64,
            total_cost: f64,
            tool_call_count: i64,
            platform: bool,
        }
        // The dashboard's aggregation (`get_finops_dashboard`), grouped by user as well.
        let rows: Vec<Row> = sqlx::query_as(
            r#"WITH s AS (
                 SELECT user_id, agent_name,
                        COALESCE(SUM(traces), 0)::BIGINT AS operations,
                        COALESCE(SUM(input_tokens), 0)::BIGINT AS input_tokens,
                        COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
                        COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
                        COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
                        COALESCE(SUM(cost_usd), 0)::FLOAT8 AS total_cost,
                        COALESCE(SUM(tool_call_count), 0)::BIGINT AS tool_call_count
                 FROM trace_spend
                 WHERE ts >= $1 AND ts < $2
                   AND ($3::TEXT IS NULL OR agent_name = $3)
                   AND ($4::TEXT IS NULL OR model = $4)
                   AND ($5::TEXT IS NULL OR provider = $5)
                   AND (NOT $6::BOOL OR user_id = ANY($7::UUID[]))
                   AND ($8::TEXT[] IS NULL OR agent_name = ANY($8)
                        OR (user_id = $9 AND (agent_name = ANY($10) OR agent_name LIKE $11)))
                 GROUP BY user_id, agent_name),
               h AS (
                 SELECT agent_name, BOOL_AND(agent_id IS NULL) AS platform
                 FROM trace_usage
                 WHERE started_at >= $1 AND started_at < $2
                   AND ($3::TEXT IS NULL OR agent_name = $3)
                   AND ($4::TEXT IS NULL OR model = $4)
                   AND ($5::TEXT IS NULL OR provider = $5)
                   AND (NOT $6::BOOL OR user_id = ANY($7::UUID[]))
                   AND ($8::TEXT[] IS NULL OR agent_name = ANY($8)
                        OR (user_id = $9 AND (agent_name = ANY($10) OR agent_name LIKE $11)))
                 GROUP BY agent_name)
               SELECT s.*, COALESCE(h.platform, false) AS platform
               FROM s LEFT JOIN h USING (agent_name)"#,
        )
        // Unprepared, like every usage read here with an optional filter: a prepared statement
        // gets Postgres's generic plan from its sixth run, which can't tell `$n IS NULL` from a
        // value, never uses the user index, and reads every caller's rows in the window.
        .persistent(false)
        .bind(start)
        .bind(end)
        .bind(agent_name)
        .bind(model)
        .bind(provider)
        .bind(!user_ids.is_empty())
        .bind(&user_ids)
        .bind(&view.scoped_names)
        .bind(view.own_platform_user)
        .bind(platform_row_names())
        .bind(PLATFORM_STEP_ROW_PATTERN)
        .fetch_all(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        // Kept exactly as the dashboard's totals keep them: a live agent in view, or a platform
        // row when the view counts those. Calls to a deleted agent are in neither.
        let registered: HashMap<&str, (uuid::Uuid, &str)> = view
            .agents
            .iter()
            .map(|(id, name, display, _)| (name.as_str(), (*id, display.as_str())))
            .collect();
        let rows = rows
            .into_iter()
            .filter_map(|r| {
                let (agent_id, agent_name) = match registered.get(r.agent_name.as_str()) {
                    Some((id, display)) => (Some(*id), display.to_string()),
                    None if view.platform_rows
                        && (r.platform || is_platform_row_name(&r.agent_name)) =>
                    {
                        (None, r.agent_name)
                    }
                    None => return None,
                };
                Some(UserAgentSpend {
                    user_id: r.user_id,
                    agent_id,
                    agent_name,
                    total_cost: r.total_cost,
                    operations: r.operations,
                    prompt_tokens: r.input_tokens,
                    completion_tokens: r.output_tokens,
                    cache_read_tokens: r.cache_read_tokens,
                    cache_creation_tokens: r.cache_creation_tokens,
                    tool_call_count: r.tool_call_count,
                })
            })
            .collect();
        Ok(FinopsUserAgentSpend { start, end, rows })
    }

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
        // `token_usage`/`flows` queries, which are name/agent_id-keyed,
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

    /// A session's own conversation (detail, trace, span, turns) is readable only by the user who
    /// had it — never a superuser bypass. This is deliberately stricter than `session/list`'s
    /// scope (`sessions_sql::scope_clause`), which a superuser still queries unscoped for
    /// ops/cost triage: that endpoint reports `first_input`/`last_output` as "(private)" for a row
    /// it doesn't own, rather than refusing the row outright, because the fleet-wide status/cost
    /// view has to keep listing it. This one has nothing to show but that content, so it 404s.
    async fn authorize_session(
        &self,
        session_id: &str,
        user_id: &str,
        resource: &str,
    ) -> Result<(DateTime<Utc>, DateTime<Utc>), ObservabilityError> {
        let caller: uuid::Uuid = user_id
            .parse()
            .map_err(|_| ObservabilityError::Internal("invalid user id in claims".into()))?;
        sqlx::query_as(
            r#"SELECT created_at, updated_at FROM chat_sessions
               WHERE session_id = $1 AND deleted_at IS NULL AND user_id = $2"#,
        )
        .bind(session_id)
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
    ) -> Result<(), ObservabilityError> {
        self.authorize_session(session_id, user_id, "session")
            .await
            .map(|_| ())
    }

    /// The session a trace belongs to, for authorizing the trace view.
    ///
    /// The server's own `flows` row (`flow_id` = trace id) is the authority:
    /// every server path that starts or forwards a turn writes its `session_id`
    /// after checking the caller owns the session. Span attributes are written by agents, and any agent
    /// in a flow can set `session.id` to a session its own owner controls, so
    /// a span's claim is used only when the server recorded nothing. Even then
    /// it is refused when the trace is a flow the server knows belongs to
    /// someone other than that session's owner.
    async fn trace_session_id(
        &self,
        trace_id: &str,
        trace: &nasiko_observability::TraceDetails,
    ) -> Result<String, ObservabilityError> {
        let internal = |e: sqlx::Error| ObservabilityError::Internal(e.to_string());
        let not_found = || ObservabilityError::NotFound("trace not found".into());

        let recorded: Option<String> =
            sqlx::query_scalar("SELECT session_id FROM flows WHERE flow_id = $1 LIMIT 1")
                .bind(trace_id)
                .fetch_optional(&self.db)
                .await
                .map_err(internal)?;
        if let Some(session_id) = recorded {
            return Ok(session_id);
        }

        let claimed = trace
            .spans
            .iter()
            .find_map(|span| {
                span.attributes
                    .get("session.id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .ok_or_else(not_found)?;

        // A known flow must belong to the claimed session's owner.
        let consistent: bool = sqlx::query_scalar(
            r#"SELECT NOT EXISTS (
                   SELECT 1 FROM flows f
                    WHERE f.flow_id = $1
                      AND NOT EXISTS (SELECT 1 FROM chat_sessions c
                                       WHERE c.session_id = $2 AND c.user_id = f.user_id))"#,
        )
        .bind(trace_id)
        .bind(&claimed)
        .fetch_one(&self.db)
        .await
        .map_err(internal)?;
        if !consistent {
            tracing::warn!(%trace_id, session_id = %claimed, "trace's session.id conflicts with its flow owner");
            return Err(not_found());
        }
        Ok(claimed)
    }

    // ── 1. session/list ──────────────────────────────────────────────────────

    /// `session/list`, served entirely from SQL — see `super::sessions_sql` for
    /// the query and for why no field needs the trace store any more.
    ///
    /// This used to enrich every row with `provider.get_session`, which is two
    /// Tempo searches plus a full trace download and three Loki queries per
    /// trace, per row. A 100-row page cost on the order of 1,400 backend
    /// requests; it now costs two SQL statements, and the cost no longer grows
    /// with the number of sessions in the window.
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
        end_time: Option<&str>,
        status: Option<&str>,
        cursor: Option<&str>,
        parent: Option<&str>,
    ) -> Result<SessionListResponse, ObservabilityError> {
        let viewer = user_id
            .parse::<uuid::Uuid>()
            .map_err(|_| ObservabilityError::Internal("invalid user id in claims".into()))?;
        // A superuser's query is unscoped (ops/cost triage needs the whole fleet), but that only
        // ever widens which ROWS come back — `Row::into_summary` still redacts a row's own
        // conversation content unless `viewer` is that row's `user_id`.
        let caller = if is_superuser { None } else { Some(viewer) };
        let limit = limit.unwrap_or(DEFAULT_SESSION_PAGE).clamp(1, 100);
        let status = status
            .map(super::sessions_sql::StatusFilter::parse)
            .transpose()?;
        // A present-but-unparseable bound is a 400: silently widening it to the
        // default window would answer a different question than the one asked.
        let start = parse_iso_param("start_time", start_time)?
            .unwrap_or_else(|| Utc::now() - Duration::days(7));
        let end = parse_iso_param("end_time", end_time)?;
        let level = match parent {
            Some(parent) if parent.trim().is_empty() => {
                return Err(ObservabilityError::BadRequest(
                    "parent must be a session id".into(),
                ));
            }
            Some(parent) => super::sessions_sql::ListLevel::ChildrenOf(parent.to_owned()),
            None => super::sessions_sql::ListLevel::TopLevel,
        };

        // `cursor` is the paging contract going forward; `offset` is still
        // accepted because the Sessions page pages on it. An offset is turned
        // into a cursor by reading the key of the row it would land on, so both
        // callers share one keyset query rather than two paging implementations.
        let cursor = match (cursor, offset.filter(|&o| o > 0)) {
            (Some(raw), _) => Some(super::sessions_sql::decode_cursor(raw).ok_or_else(|| {
                ObservabilityError::BadRequest("cursor is not a valid page cursor".into())
            })?),
            (None, Some(offset)) => {
                match self.cursor_at_offset(caller, start, &level, offset).await? {
                    Some(anchor) => Some(anchor),
                    // The offset is past the end. An absent anchor would otherwise
                    // read as "no cursor" and hand back the *first* page. Anchoring
                    // at `(start, "")` excludes every row -- the page requires
                    // `created_at >= start`, so nothing can also sort before
                    // `(start, "")` -- while still running the count, so
                    // `matching_count` stays right. `DateTime::MIN_UTC` would be
                    // the obvious sentinel and is outside `timestamptz`'s range.
                    None => Some((start, String::new())),
                }
            }
            (None, None) => None,
        };

        super::sessions_sql::list_sessions(
            &self.db,
            &super::sessions_sql::SessionListQuery {
                caller,
                viewer,
                start,
                end,
                status,
                level,
                limit,
                cursor,
            },
        )
        .await
    }

    /// The keyset anchor an `offset` would land on, for the legacy paging path.
    /// One indexed scan of `offset` keys — no aggregates, no joins. Same level
    /// as the page (`$4` is the parent), so the offset counts the rows it lists.
    async fn cursor_at_offset(
        &self,
        caller: Option<uuid::Uuid>,
        start: chrono::DateTime<Utc>,
        level: &super::sessions_sql::ListLevel,
        offset: i64,
    ) -> Result<Option<(chrono::DateTime<Utc>, String)>, ObservabilityError> {
        let window = match level {
            super::sessions_sql::ListLevel::TopLevel => "AND s.created_at >= $2",
            super::sessions_sql::ListLevel::ChildrenOf(_) => "",
        };
        let sql = format!(
            "SELECT s.created_at, s.session_id FROM chat_sessions s \
             WHERE s.deleted_at IS NULL AND ($1::uuid IS NULL OR s.user_id = $1) {window} \
               AND {} \
             ORDER BY s.created_at DESC, s.session_id DESC \
             LIMIT 1 OFFSET $3",
            level.predicate()
        );
        sqlx::query_as::<_, (chrono::DateTime<Utc>, String)>(&sql)
            .bind(caller)
            .bind(start)
            .bind(offset - 1)
            .bind(level.parent())
            .fetch_optional(&self.db)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))
    }

    // ── 2. session/{session_id} ──────────────────────────────────────────────

    /// `(title, start, end)` for a session drill-down.
    ///
    /// The trace store is queried over a window, and this used to hardcode
    /// `now() - 7d`: opening any session older than a week returned an empty
    /// trace list. The session's own rows say when it actually ran —
    /// `chat_sessions` for chat-originated sessions, `flows` (one row per
    /// turn, keyed by `session_id`) for the rest — so take the
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
                         FROM flows WHERE session_id = $1) t
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
    ) -> Result<SessionDetailResponse, ObservabilityError> {
        self.authorize_session(session_id, user_id, "session")
            .await?;
        // An orchestrator-routed session has no agent of its own; name the one that did the work.
        let ran_agent = super::sessions_sql::ran_agent_name("cs.session_id");
        let agent_name: Option<String> = sqlx::query_scalar(&format!(
            r#"SELECT COALESCE(
                 CASE
                   WHEN a.coding_agent_integration_id IS NOT NULL
                        AND u.username IS NOT NULL
                        AND a.name NOT LIKE u.username || '-%'
                     THEN u.username || '-' || a.name
                   ELSE a.name
                 END,
                 {ran_agent})
               FROM chat_sessions cs
               LEFT JOIN agents a ON a.id = cs.agent_id
               LEFT JOIN users u ON u.id = cs.user_id
               WHERE cs.session_id = $1"#
        ))
        .bind(session_id)
        .fetch_optional(&self.db)
        .await
        .map_err(|error| ObservabilityError::Internal(error.to_string()))?
        .flatten();
        // Anchor Tempo's seven-day maximum query range to this session rather
        // than to today. Otherwise an old session that still exists in
        // Postgres can never find its historical traces.
        let (title, start, end) = self.session_window(session_id).await;
        // Traces, tokens and cost come from `flows` + `trace_usage` (hot + cold), so
        // sessions older than Tempo's retention, seeded or tiered, still list them.
        // Tempo only adds span attributes and prompt/completion text while it has them.
        let flows = self
            .store
            .load_flows_for_session(session_id)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?;
        let flow_ids: Vec<String> = flows.iter().map(|f| f.flow_id.clone()).collect();
        let usage = self
            .store
            .load_trace_usage_for_session(session_id, &flow_ids)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?;
        // A coding-agent session (a subagent's included) has no flows: its turns are its usage rows.
        let turns = super::session_store::flows_with_usage_turns(&flows, &usage);
        let mut details = super::session_store::session_from_rows(session_id, &turns, &usage);
        match self.provider.get_session(session_id, start, end).await {
            Ok(live) if flows.is_empty() => details = live,
            Ok(live) => super::session_store::enrich_from_tempo(&mut details, &live),
            Err(e) => tracing::debug!(%session_id, error = %e, "tempo enrichment skipped"),
        }
        let lineage = super::subagents_sql::session_lineage(&self.db, session_id).await?;

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
                    parent_session_id: lineage.parent_session_id,
                    spawn_tool_call_id: lineage.spawn_tool_call_id,
                    subagent_type: lineage.subagent_type,
                    subagents: lineage.subagents,
                    traces: trace_entries,
                    pagination: Pagination {
                        end_cursor,
                        has_next_page: details.has_more_traces,
                    },
                },
            },
        })
    }

    /// What the server recorded of one turn, for a trace Tempo cannot serve: its
    /// `flows` row, else (a coding-agent turn, which never crosses the proxy) a
    /// turn rebuilt from its `trace_usage` rows. `None` when it recorded neither.
    async fn recorded_turn(
        &self,
        trace_id: &str,
    ) -> Result<Option<(FlowRow, Vec<TraceUsageRow>)>, ObservabilityError> {
        let internal = |e: anyhow::Error| ObservabilityError::Internal(e.to_string());
        if let Some(flow) = self.store.load_flow(trace_id).await.map_err(internal)? {
            let usage = self
                .store
                .load_trace_usage_for_session(&flow.session_id, std::slice::from_ref(&flow.flow_id))
                .await
                .map_err(internal)?;
            return Ok(Some((flow, usage)));
        }
        let usage = self
            .store
            .load_trace_usage_for_trace(trace_id)
            .await
            .map_err(internal)?;
        Ok(super::session_store::flow_from_usage(trace_id, &usage).map(|flow| (flow, usage)))
    }

    // ── 3. trace/{trace_id} ──────────────────────────────────────────────────

    pub async fn get_trace_details(
        &self,
        trace_id: &str,
        user_id: &str,
    ) -> Result<TraceDetailResponse, ObservabilityError> {
        // Tempo first; when it no longer has the trace (retention, seeded or tiered
        // turns), rebuild it from the flow + `trace_usage` rows, hot + cold.
        let mut sql_cost: Option<CostBreakdown> = None;
        let mut trace = match self.provider.get_trace(trace_id).await {
            Ok(t) if !t.spans.is_empty() => t,
            // Any Tempo miss or failure (retention, non-hex seeded ids, outage) falls
            // back to SQL; a trace the server recorded nothing for is still a 404.
            Ok(_) | Err(_) => {
                let internal = |e: anyhow::Error| ObservabilityError::Internal(e.to_string());
                let (flow, usage) = self
                    .recorded_turn(trace_id)
                    .await?
                    .ok_or_else(|| ObservabilityError::NotFound("trace not found".into()))?;
                let (mut t, c) = super::session_store::trace_from_rows(&flow, &usage);
                let calls = self
                    .store
                    .session_tool_calls_all(&flow.session_id)
                    .await
                    .map_err(internal)?;
                super::session_store::add_tool_call_spans(&mut t, &calls);
                sql_cost = Some(c);
                t
            }
        };
        let mut seen_spans = HashSet::new();
        trace
            .spans
            .retain(|span| seen_spans.insert(span.span_id.clone()));
        let project_session_id = self.trace_session_id(trace_id, &trace).await?;
        self.authorize_session(&project_session_id, user_id, "trace")
            .await?;

        let from_sql = sql_cost.is_some();
        let mut cost = sql_cost.unwrap_or_default();
        let mut seen_spans = HashSet::new();
        for span in trace.spans.iter().filter(|_| !from_sql) {
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
                        cache_read: Some(CostOnly {
                            cost: cost.cache_read_usd,
                        }),
                        cache_creation: Some(CostOnly {
                            cost: cost.cache_creation_usd,
                        }),
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
    ) -> Result<SpanDetailResponse, ObservabilityError> {
        // Resolve and authorize the trace's session before looking up the span
        // or its Loki content, so inaccessible traces reveal no span existence.
        let span_nf = || ObservabilityError::NotFound("span not found".into());
        let (trace, from_sql) = match self.provider.get_trace(trace_id).await {
            Ok(t) if !t.spans.is_empty() => (t, false),
            Ok(_) | Err(_) => {
                let (flow, usage) = self.recorded_turn(trace_id).await?.ok_or_else(span_nf)?;
                (super::session_store::trace_from_rows(&flow, &usage).0, true)
            }
        };
        let session_id = self
            .trace_session_id(trace_id, &trace)
            .await
            .map_err(|e| match e {
                ObservabilityError::NotFound(_) => {
                    ObservabilityError::NotFound("span not found".into())
                }
                other => other,
            })?;
        self.authorize_session(&session_id, user_id, "span").await?;
        let details = if from_sql {
            let span = trace
                .spans
                .iter()
                .find(|s| s.span_id == span_id)
                .cloned()
                .ok_or_else(span_nf)?;
            super::session_store::span_details_from_sql(span)
        } else {
            match self.provider.get_span(trace_id, span_id).await {
                Ok(details) => details,
                Err(ObservabilityError::NotFound(_)) => return Err(span_nf()),
                // The trace was just read, so this is the trace store failing the
                // second read (an overloaded Tempo answers 429). Serve the span from
                // that read, without the prompt and completion text, rather than a 500.
                Err(e) => {
                    tracing::warn!(
                        %trace_id, %span_id, error = %e,
                        "span read failed; serving the span from its trace"
                    );
                    let span = trace
                        .spans
                        .iter()
                        .find(|s| s.span_id == span_id)
                        .cloned()
                        .ok_or_else(span_nf)?;
                    super::session_store::span_details_from_sql(span)
                }
            }
        };
        let span = &details.span;

        // Token counts come from the provider (it rolls the span's children up);
        // only the model name still has to be read off the raw attributes, after the one
        // its gateway span answered with, so the panel agrees with the trace tree.
        let answer = answered_models(&trace.spans).remove(span.span_id.as_str());
        let provider = answer
            .as_ref()
            .and_then(|a| a.provider.clone())
            .or_else(|| {
                first_str_attr(&span.attributes, &["gen_ai.system", "gen_ai.provider.name"])
            });
        let model = answer
            .map(|a| a.model)
            .or_else(|| extract_token_attrs(&span.attributes).2);
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

        let status = span_status_str(span).to_string();

        Ok(SpanDetailResponse {
            data: SpanDetailData {
                span: SpanDetail {
                    id: encode_span_id(&span.span_id),
                    span_id: span.span_id.clone(),
                    trace: SpanTraceRef {
                        id: encode_trace_id(trace_id),
                        trace_id: trace_id.to_string(),
                    },
                    name: span_display_name(span, model.as_deref()),
                    span_kind,
                    code: status.clone(),
                    status_code: status,
                    status_message: span.status_message.clone(),
                    start_time: Some(fmt_ts(span.started_at)),
                    end_time: span.ended_at.map(fmt_ts),
                    parent_id: span.parent_span_id.clone(),
                    latency_ms: span.duration_ms.map(|d| d as f64),
                    token_count_total: usage.total_tokens,
                    provider,
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

    /// `user_ids` narrows every figure to those people's calls (`None`: everyone's).
    pub async fn get_agent_stats(
        &self,
        agent_id: &str,
        start_time: Option<&str>,
        user_ids: Option<&[uuid::Uuid]>,
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
               WHERE agent_name = $1 AND started_at >= $2 AND started_at < $3
                 AND ($4::uuid[] IS NULL OR user_id = ANY($4))"#,
        )
        .persistent(false)
        .bind(agent_id)
        .bind(start)
        .bind(end)
        .bind(user_ids)
        .fetch_one(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        // Tempo's cache figures cover every caller of the agent and cannot be narrowed to some of
        // them, so a narrowed read leaves them out (absent, not $0) rather than show other people's.
        let cache_cost = match user_ids {
            Some(_) => None,
            None => Some(
                self.provider
                    .agent_stats(agent_id, start, end)
                    .await
                    .map(|stats| (stats.cost.cache_read_usd, stats.cost.cache_creation_usd))
                    .unwrap_or_default(),
            ),
        };

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
                        cache_read: cache_cost.map(|(read, _)| CostOnly { cost: read }),
                        cache_creation: cache_cost.map(|(_, write)| CostOnly { cost: write }),
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
        user_id: &str,
        _role: Option<&str>,
        _department_id: Option<&str>,
        _team_id: Option<&str>,
        start_time: Option<&str>,
        end_time: Option<&str>,
        agent_name: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
        access: FinopsAccess<'_>,
        // When `Some`, restricts the agent list to agents owned by this user UUID.
        owner_id: Option<&str>,
        view: &str,
        // `compare=previous`: include each agent's preceding-window totals. The
        // query runs regardless (the KPI strip needs it); this only controls
        // whether the per-agent detail is kept instead of discarded.
        compare_previous: bool,
    ) -> Result<FinopsDashboardResponse, ObservabilityError> {
        let FinopsAccess {
            user_ids,
            accessible_agent_ids,
            ..
        } = access;
        let parsed_owner_id = owner_id.and_then(|s| s.parse::<uuid::Uuid>().ok());
        let AgentsInView {
            agents,
            accessible,
            dropped_internal,
            scoped_names,
            own_platform_user,
            platform_rows,
        } = self
            .agents_in_view(user_id, agent_name, accessible_agent_ids, parsed_owner_id)
            .await?;
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

        let mut hours_rows =
            hours_meter::windowed_agent_hours(&self.db, start, now, accessible_agent_ids)
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
        let mut minutes_rows =
            sandbox_meter::windowed_sandbox_minutes(&self.db, start, now, accessible_agent_ids)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "sandbox minutes aggregation failed");
                    vec![]
                });
        if let Some(accessible) = &accessible {
            minutes_rows.retain(|row| accessible.contains(&row.agent_id));
        }
        if !dropped_internal.is_empty() {
            minutes_rows.retain(|row| !dropped_internal.contains(&row.agent_id));
        }
        let total_sandbox_minutes = round6(minutes_rows.iter().map(|r| r.minutes).sum());
        let minutes_by_agent: HashMap<uuid::Uuid, f64> = minutes_rows
            .iter()
            .map(|r| (r.agent_id, r.minutes))
            .collect();
        let total_container_hours = round6(hours_rows.iter().map(|r| r.hours).sum());
        let hours_by_agent: HashMap<uuid::Uuid, f64> =
            hours_rows.iter().map(|r| (r.agent_id, r.hours)).collect();

        // A scoped caller with no agents can still have spent through the platform (a chat the
        // orchestrator answered itself, a generated MAF plan), so only skip the queries when
        // those rows could not be in view.
        if agents.is_empty() && own_platform_user.is_none() {
            return Ok(empty_finops_response(
                total_container_hours,
                total_sandbox_minutes,
            ));
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
            /// Every row has no agent id: the platform's own calls (orchestrator routing and
            /// MAF planning, service `nasiko-cp`), which belong to no registered agent.
            platform: bool,
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
        // Counts, tokens and cost come from `trace_spend` (hot rows + tiered days), so
        // the totals survive tiering. Cost confidence and latency percentiles exist only
        // per hot row, so they describe the hot part of the window.
        const TRACE_USAGE_AGG_QUERY: &str = r#"WITH s AS (
                 SELECT agent_name,
                        COALESCE(SUM(traces), 0)::BIGINT AS operations,
                        COALESCE(SUM(input_tokens), 0)::BIGINT AS input_tokens,
                        COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
                        COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
                        COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
                        COALESCE(SUM(cost_usd), 0)::FLOAT8 AS total_cost,
                        COALESCE(SUM(tool_call_count), 0)::BIGINT AS tool_call_count
                 FROM trace_spend
                 WHERE ts >= $1 AND ts < $2
                   AND ($3::TEXT IS NULL OR agent_name = $3)
                   AND ($4::TEXT IS NULL OR model = $4)
                   AND ($5::TEXT IS NULL OR provider = $5)
                   AND (NOT $6::BOOL OR user_id = ANY($7::UUID[]))
                   AND ($8::TEXT[] IS NULL OR agent_name = ANY($8)
                        OR (user_id = $9 AND (agent_name = ANY($10) OR agent_name LIKE $11)))
                 GROUP BY agent_name),
               h AS (
                 SELECT agent_name,
                        COALESCE(SUM(cost_usd) FILTER (WHERE cost_estimated), 0)::FLOAT8 AS estimated_cost,
                        COUNT(*) FILTER (WHERE cost_estimated IS NULL)::BIGINT AS unknown_confidence_calls,
                        percentile_cont(0.5)  WITHIN GROUP (ORDER BY latency_ms)::FLOAT8 AS p50_latency,
                        percentile_cont(0.95) WITHIN GROUP (ORDER BY latency_ms)::FLOAT8 AS p95_latency,
                        percentile_cont(0.99) WITHIN GROUP (ORDER BY latency_ms)::FLOAT8 AS p99_latency,
                        BOOL_AND(agent_id IS NULL) AS platform
                 FROM trace_usage
                 WHERE started_at >= $1 AND started_at < $2
                   AND ($3::TEXT IS NULL OR agent_name = $3)
                   AND ($4::TEXT IS NULL OR model = $4)
                   AND ($5::TEXT IS NULL OR provider = $5)
                   AND (NOT $6::BOOL OR user_id = ANY($7::UUID[]))
                   AND ($8::TEXT[] IS NULL OR agent_name = ANY($8)
                        OR (user_id = $9 AND (agent_name = ANY($10) OR agent_name LIKE $11)))
                 GROUP BY agent_name)
               SELECT s.agent_name, s.operations, s.input_tokens, s.output_tokens,
                      s.cache_read_tokens, s.cache_creation_tokens, s.total_cost,
                      COALESCE(h.estimated_cost, 0)::FLOAT8 AS estimated_cost,
                      COALESCE(h.unknown_confidence_calls, 0)::BIGINT AS unknown_confidence_calls,
                      s.tool_call_count, h.p50_latency, h.p95_latency, h.p99_latency,
                       COALESCE(h.platform, false) AS platform
               FROM s LEFT JOIN h USING (agent_name)"#;

        // 1. Current window
        let current_rows: Vec<TraceUsageAgg> = sqlx::query_as(TRACE_USAGE_AGG_QUERY)
            .persistent(false)
            .bind(start)
            .bind(now)
            .bind(agent_filter)
            .bind(model_filter)
            .bind(provider_filter)
            .bind(has_user_filter)
            .bind(&user_id_list)
            .bind(&scoped_names)
            .bind(own_platform_user)
            .bind(platform_row_names())
            .bind(PLATFORM_STEP_ROW_PATTERN)
            .fetch_all(&self.db)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        // 2. Previous window (for KPI %-change)
        let prev_rows: Vec<TraceUsageAgg> = sqlx::query_as(TRACE_USAGE_AGG_QUERY)
            .persistent(false)
            .bind(prev_start)
            .bind(prev_end)
            .bind(agent_filter)
            .bind(model_filter)
            .bind(provider_filter)
            .bind(has_user_filter)
            .bind(&user_id_list)
            .bind(&scoped_names)
            .bind(own_platform_user)
            .bind(platform_row_names())
            .bind(PLATFORM_STEP_ROW_PATTERN)
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
                 AND ($7::TEXT[] IS NULL OR agent_name = ANY($7)
                      OR (user_id = $8 AND (agent_name = ANY($9) OR agent_name LIKE $10)))
               GROUP BY agent_name"#,
        )
        .persistent(false)
        .bind(last_24h)
        .bind(agent_filter)
        .bind(model_filter)
        .bind(provider_filter)
        .bind(has_user_filter)
        .bind(&user_id_list)
        .bind(&scoped_names)
        .bind(own_platform_user)
        .bind(platform_row_names())
        .bind(PLATFORM_STEP_ROW_PATTERN)
        .fetch_all(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        // 4. Unpriced calls in the window: ran (had tokens) but no price row backs the
        //    model, so they recorded no cost or a cost at the default rate. Surface the
        //    count as a known gap. Same filters as the main aggregation.
        let unpriced_calls: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*)::BIGINT
               FROM trace_usage
               WHERE started_at >= $1 AND started_at < $2
                 AND ($3::TEXT IS NULL OR agent_name = $3)
                 AND ($4::TEXT IS NULL OR model = $4)
                 AND ($5::TEXT IS NULL OR provider = $5)
                 AND (NOT $6::BOOL OR user_id = ANY($7::UUID[]))
                 AND (COALESCE(cost_usd, 0) = 0
                      OR metadata->'pricing'->>'source' = 'Default')
                 AND (COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)) > 0
                 AND ($8::TEXT[] IS NULL OR agent_name = ANY($8)
                      OR (user_id = $9 AND (agent_name = ANY($10) OR agent_name LIKE $11)))"#,
        )
        .persistent(false)
        .bind(start)
        .bind(now)
        .bind(agent_filter)
        .bind(model_filter)
        .bind(provider_filter)
        .bind(has_user_filter)
        .bind(&user_id_list)
        .bind(&scoped_names)
        .bind(own_platform_user)
        .bind(platform_row_names())
        .bind(PLATFORM_STEP_ROW_PATTERN)
        .fetch_one(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        // 5. Per user, under the same filters: who spent what.
        let users: Vec<UserFinopsRow> = sqlx::query_as(
            r#"SELECT s.user_id::TEXT AS user_id, u.username, u.email,
                      COALESCE(SUM(s.cost_usd), 0)::FLOAT8 AS total_cost,
                      COALESCE(SUM(s.traces), 0)::BIGINT AS operations,
                      COALESCE(SUM(s.input_tokens), 0)::BIGINT AS prompt_tokens,
                      COALESCE(SUM(s.output_tokens), 0)::BIGINT AS completion_tokens,
                      COALESCE(SUM(s.cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
                      COALESCE(SUM(s.cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
                      COALESCE(SUM(s.input_tokens + s.output_tokens + s.cache_read_tokens
                                   + s.cache_creation_tokens), 0)::BIGINT AS total_tokens,
                      COALESCE(SUM(s.tool_call_count), 0)::BIGINT AS tool_call_count,
                      COUNT(DISTINCT s.agent_name)::BIGINT AS agents
               FROM trace_spend s
               LEFT JOIN users u ON u.id = s.user_id
               WHERE s.ts >= $1 AND s.ts < $2
                 AND ($3::TEXT IS NULL OR s.agent_name = $3)
                 AND ($4::TEXT IS NULL OR s.model = $4)
                 AND ($5::TEXT IS NULL OR s.provider = $5)
                 AND (NOT $6::BOOL OR s.user_id = ANY($7::UUID[]))
                 AND ($8::TEXT[] IS NULL OR s.agent_name = ANY($8)
                      OR (s.user_id = $9 AND (s.agent_name = ANY($10) OR s.agent_name LIKE $11)))
               GROUP BY s.user_id, u.username, u.email
               ORDER BY total_cost DESC, total_tokens DESC, s.user_id"#,
        )
        .persistent(false)
        .bind(start)
        .bind(now)
        .bind(agent_filter)
        .bind(model_filter)
        .bind(provider_filter)
        .bind(has_user_filter)
        .bind(&user_id_list)
        .bind(&scoped_names)
        .bind(own_platform_user)
        .bind(platform_row_names())
        .bind(PLATFORM_STEP_ROW_PATTERN)
        .fetch_all(&self.db)
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
                sandbox_minutes: round6(minutes_by_agent.get(agent_uuid).copied().unwrap_or(0.0)),
                previous: compare_previous.then(|| AgentFinopsPrevious {
                    total_cost: prev.map(|p| round6(p.total_cost)).unwrap_or(0.0),
                    operations: prev.map(|p| p.operations as usize).unwrap_or(0),
                    total_tokens: prev
                        .map(|p| {
                            (p.input_tokens
                                + p.output_tokens
                                + p.cache_read_tokens
                                + p.cache_creation_tokens) as u64
                        })
                        .unwrap_or(0),
                    tool_call_count: prev.map(|p| p.tool_call_count as u64).unwrap_or(0),
                    avg_latency_ms: prev.and_then(|p| p.p50_latency),
                }),
            });
        }

        // The platform's own calls are real spend under no registered agent, so the loop above
        // never meets them. A caller who sees the whole fleet gets them in the totals, and as
        // one slice of the spend chart; a scoped caller gets the same for their own calls only,
        // which is all the queries above let through for them.
        let mut platform_cost = 0.0;
        if platform_rows {
            let registered: HashSet<&str> =
                agents.iter().map(|(_, name, _, _)| name.as_str()).collect();
            // The orchestrator's and MAF's rows carry the routed agent's id (other views name the
            // agent a session ran from it), so they are recognised by name, not by a NULL id.
            let is_platform = |r: &&TraceUsageAgg| {
                (r.platform || is_platform_row_name(&r.agent_name))
                    && !registered.contains(r.agent_name.as_str())
            };
            // Orchestrator rows store `latency_ms = 0`; a zero would drag the KPI latency down.
            let measured = |latency: Option<f64>| latency.filter(|l| *l > 0.0);
            for r in current_rows.iter().filter(is_platform) {
                grand_input += r.input_tokens as u64;
                grand_output += r.output_tokens as u64;
                grand_cache_read += r.cache_read_tokens as u64;
                grand_cache_creation += r.cache_creation_tokens as u64;
                grand_cost += r.total_cost;
                grand_tool_calls += r.tool_call_count as u64;
                total_ops += r.operations as usize;
                latency_samples.extend(measured(r.p50_latency));
                latency_samples_p95.extend(measured(r.p95_latency));
                latency_samples_p99.extend(measured(r.p99_latency));
                platform_cost += r.total_cost;
            }
            for r in prev_rows.iter().filter(is_platform) {
                prev_grand_cost += r.total_cost;
                prev_grand_total_tokens += (r.input_tokens + r.output_tokens) as u64;
                prev_total_ops += r.operations as usize;
                prev_grand_tool_calls += r.tool_call_count as u64;
                prev_latency_samples.extend(measured(r.p50_latency));
                prev_latency_samples_p95.extend(measured(r.p95_latency));
                prev_latency_samples_p99.extend(measured(r.p99_latency));
            }
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
            if platform_cost > 0.0 {
                slices.push(SpendPieSlice {
                    agent_name: "Platform (routing and planning)".into(),
                    spend_usd: round6(platform_cost),
                    pct: pct(platform_cost),
                });
            }
            SpendByAgentBreakdown {
                slices,
                total_spend_usd: total,
            }
        };

        let attributions = if view == "workflow" {
            let rows = self
                .get_workflow_finops_rows(start, now, agent_name, access)
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
                    total_sandbox_minutes,
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
                users,
            },
            status_code: 200,
            message: "FinOps dashboard data retrieved successfully".into(),
        })
    }

    // ── 6b. finops/spend-timeseries ───────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub async fn get_finops_spend_timeseries(
        &self,
        start_time: Option<&str>,
        end_time: Option<&str>,
        range: Option<&str>,
        agent_name: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
        // `group_by=agent`: also return the top `top` agents' own series plus an
        // `other` aggregate.
        group_by_agent: bool,
        top: i64,
        // `compare=previous`: also return the preceding window of equal length.
        compare_previous: bool,
        access: FinopsAccess<'_>,
    ) -> Result<FinopsSpendTimeseriesResponse, ObservabilityError> {
        let scope = self.scope_binds(access).await?;
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
                     AND ($6::TEXT[] IS NULL OR agent_name = ANY($6::TEXT[]))
                     AND (NOT $7::BOOL OR user_id = ANY($8::UUID[]))
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
                     AND ($6::TEXT[] IS NULL OR agent_name = ANY($6::TEXT[]))
                     AND (NOT $7::BOOL OR user_id = ANY($8::UUID[]))
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

        let fetch = |from: DateTime<Utc>, to: DateTime<Utc>| {
            sqlx::query_as::<_, BucketRow>(&query)
                .persistent(false)
                .bind(from)
                .bind(to)
                .bind(agent_name)
                .bind(model)
                .bind(provider)
                .bind(&scope.agent_names)
                .bind(scope.has_user_filter)
                .bind(&scope.user_ids)
                .fetch_all(&self.db)
        };

        let to_point = |r: BucketRow| SpendTimeseriesPoint {
            bucket_start: fmt_ts(r.bucket_start),
            spend_usd: round6(r.spend_usd),
            operations: r.operations as usize,
            tool_calls: r.tool_calls.max(0) as u64,
            top_agent_name: r.top_agent_name,
            top_agent_spend_usd: r.top_agent_spend_usd.map(round6),
            p50_latency_ms: r.p50_latency_ms,
            p95_latency_ms: r.p95_latency_ms,
            p99_latency_ms: r.p99_latency_ms,
        };

        let rows = fetch(start, end)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        let previous = if compare_previous {
            let window = end - start;
            let rows = fetch(start - window, start)
                .await
                .map_err(|e| ObservabilityError::Internal(e.to_string()))?;
            Some(rows.into_iter().map(to_point).collect())
        } else {
            None
        };

        let series = if group_by_agent {
            Some(
                self.spend_series_by_agent(
                    start, end, trunc, agent_name, model, provider, top, &scope,
                )
                .await?,
            )
        } else {
            None
        };

        Ok(FinopsSpendTimeseriesResponse {
            data: FinopsSpendTimeseries {
                bucket: bucket_label(bucket).to_string(),
                points: rows.into_iter().map(to_point).collect(),
                series,
                previous,
            },
            status_code: 200,
            message: "Spend timeseries retrieved successfully".into(),
        })
    }

    /// The top `top` agents' own spend series plus one `other` series holding
    /// everything else, so the series still sum to the fleet points.
    ///
    /// One query: the window's top spenders are picked in SQL and every bucket
    /// is labelled in the same pass, rather than issuing one filtered timeseries
    /// request per driver.
    #[allow(clippy::too_many_arguments)]
    async fn spend_series_by_agent(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        trunc: &str,
        agent_name: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
        top: i64,
        scope: &ScopeBinds,
    ) -> Result<Vec<SpendAgentSeries>, ObservabilityError> {
        #[derive(sqlx::FromRow)]
        struct SeriesRow {
            agent_name: Option<String>,
            bucket_start: DateTime<Utc>,
            spend_usd: f64,
            operations: i64,
        }

        // `ranked` orders agents by their whole-window spend; anything outside
        // the top `top` collapses to a NULL agent_name, which becomes `other`.
        let query = format!(
            r#"WITH filtered AS (
                   SELECT agent_name, cost_usd, started_at
                     FROM trace_usage
                    WHERE started_at >= $1 AND started_at < $2
                      AND ($3::TEXT IS NULL OR agent_name = $3)
                      AND ($4::TEXT IS NULL OR model = $4)
                      AND ($5::TEXT IS NULL OR provider = $5)
                      AND ($7::TEXT[] IS NULL OR agent_name = ANY($7::TEXT[]))
                      AND (NOT $8::BOOL OR user_id = ANY($9::UUID[]))
               ),
               ranked AS (
                   SELECT agent_name,
                          ROW_NUMBER() OVER (ORDER BY SUM(cost_usd) DESC, agent_name) AS rn
                     FROM filtered
                    GROUP BY agent_name
               )
               SELECT CASE WHEN r.rn <= $6 THEN f.agent_name END AS agent_name,
                      date_trunc('{trunc}', f.started_at) AS bucket_start,
                      SUM(f.cost_usd)::FLOAT8 AS spend_usd,
                      COUNT(*)::BIGINT AS operations
                 FROM filtered f
                 JOIN ranked r ON r.agent_name = f.agent_name
                GROUP BY 1, 2
                ORDER BY 2"#
        );

        let rows: Vec<SeriesRow> = sqlx::query_as(&query)
            .persistent(false)
            .bind(start)
            .bind(end)
            .bind(agent_name)
            .bind(model)
            .bind(provider)
            .bind(top.clamp(1, 20))
            .bind(&scope.agent_names)
            .bind(scope.has_user_filter)
            .bind(&scope.user_ids)
            .fetch_all(&self.db)
            .await
            .map_err(|e| ObservabilityError::Internal(e.to_string()))?;

        // Raw trace name → UUID, but only when exactly one live agent carries
        // that name: a name shared by two agents identifies neither (TK-3).
        let mut id_by_name: HashMap<String, Option<String>> = HashMap::new();
        for (id, name, _display, _version) in self.get_agent_names(None).await.unwrap_or_default() {
            id_by_name
                .entry(name)
                .and_modify(|slot| *slot = None)
                .or_insert_with(|| Some(id.to_string()));
        }

        let mut by_agent: HashMap<Option<String>, Vec<SpendAgentPoint>> = HashMap::new();
        let mut totals: HashMap<Option<String>, f64> = HashMap::new();
        for r in rows {
            *totals.entry(r.agent_name.clone()).or_insert(0.0) += r.spend_usd;
            by_agent
                .entry(r.agent_name)
                .or_default()
                .push(SpendAgentPoint {
                    bucket_start: fmt_ts(r.bucket_start),
                    spend_usd: round6(r.spend_usd),
                    operations: r.operations as usize,
                });
        }

        let mut series: Vec<SpendAgentSeries> = by_agent
            .into_iter()
            .map(|(name, points)| SpendAgentSeries {
                agent_id: name
                    .as_ref()
                    .and_then(|n| id_by_name.get(n).cloned().flatten()),
                is_other: name.is_none(),
                total_spend_usd: round6(totals.get(&name).copied().unwrap_or(0.0)),
                agent_name: name,
                points,
            })
            .collect();
        // Named series by spend, `other` last whatever it totals.
        series.sort_by(|a, b| match (a.is_other, b.is_other) {
            (false, true) => std::cmp::Ordering::Less,
            (true, false) => std::cmp::Ordering::Greater,
            _ => b
                .total_spend_usd
                .partial_cmp(&a.total_spend_usd)
                .unwrap_or(std::cmp::Ordering::Equal),
        });
        Ok(series)
    }

    // ── 6c. finops/spend-calendar ─────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub async fn get_finops_spend_calendar(
        &self,
        month: &str,
        range: Option<&str>,
        agent_name: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
        source: Option<&str>,
        access: FinopsAccess<'_>,
    ) -> Result<FinopsSpendCalendarResponse, ObservabilityError> {
        let scope = self.scope_binds(access).await?;
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
            r#"SELECT DATE(ts) AS date,
                      COALESCE(SUM(cost_usd), 0)::FLOAT8 AS spend_usd,
                      COALESCE(SUM(traces), 0)::BIGINT AS operations
               FROM trace_spend
               WHERE ts >= $1 AND ts < $2
                 AND ($3::TEXT IS NULL OR agent_name = $3)
                 AND ($4::TEXT IS NULL OR model = $4)
                 AND ($5::TEXT IS NULL OR provider = $5)
                 AND ($6::TEXT[] IS NULL OR agent_name = ANY($6::TEXT[]))
                 AND (NOT $7::BOOL OR user_id = ANY($8::UUID[]))
                 AND ($9::TEXT IS NULL OR source = $9)
               GROUP BY DATE(ts)
               ORDER BY date"#,
        )
        .persistent(false)
        .bind(month_start)
        .bind(next_month)
        .bind(agent_name)
        .bind(model)
        .bind(provider)
        .bind(&scope.agent_names)
        .bind(scope.has_user_filter)
        .bind(&scope.user_ids)
        .bind(source)
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
        access: FinopsAccess<'_>,
    ) -> Result<FinopsDayDrilldownResponse, ObservabilityError> {
        let scope = self.scope_binds(access).await?;
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
                 AND ($6::TEXT[] IS NULL OR agent_name = ANY($6::TEXT[]))
                 AND (NOT $7::BOOL OR user_id = ANY($8::UUID[]))
               GROUP BY hour, agent_name
               ORDER BY hour"#,
        )
        .persistent(false)
        .bind(day_start)
        .bind(day_end)
        .bind(agent_name)
        .bind(model)
        .bind(provider)
        .bind(&scope.agent_names)
        .bind(scope.has_user_filter)
        .bind(&scope.user_ids)
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

    /// Postgres only, no Tempo calls. A run's cost is MAF's own calls
    /// (`maf_executions.cost_usd`) plus every agent step's usage, read from
    /// `trace_usage` by the trace id each step recorded — the same lookup as
    /// `GET /api/maf/execution/{id}/usage`.
    async fn get_workflow_finops_rows(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        agent_name: Option<&str>,
        access: FinopsAccess<'_>,
    ) -> Result<Vec<WorkflowFinopsRow>, ObservabilityError> {
        let org_user_ids = access
            .user_ids
            .filter(|ids| !ids.is_empty())
            .map(<[uuid::Uuid]>::to_vec)
            .unwrap_or_default();
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
                 )
                 AND ($4::UUID IS NULL OR e.user_id = $4)
                 AND (NOT $5::BOOL OR e.user_id = ANY($6::UUID[]))"#,
        )
        .bind(start)
        .bind(end)
        .bind(agent_name)
        .bind(access.workflow_owner)
        .bind(!org_user_ids.is_empty())
        .bind(&org_user_ids)
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

        // Each run's step traces. A retried step reuses its trace id, so dedupe per run.
        let step_traces = |row: &ExecRow| -> HashSet<String> {
            row.step_results
                .as_ref()
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter_map(|step| step.get("trace_id")?.as_str().map(str::to_string))
                .collect()
        };
        let trace_ids: Vec<String> = rows.iter().flat_map(step_traces).collect();
        let step_usage = self.step_trace_usage(&trace_ids).await?;

        let mut by_workflow: HashMap<String, Acc> = HashMap::new();
        for row in rows {
            let Some(maf_id) = row.maf_id else { continue };
            let key = maf_id.to_string();
            let mut steps = StepTraceUsage::default();
            for trace in step_traces(&row) {
                if let Some(usage) = step_usage.get(&trace) {
                    steps.add(usage);
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
            acc.cost += row.cost_usd + steps.cost_usd;
            acc.input_tokens += steps.input_tokens.max(0) as u64;
            acc.output_tokens += steps.output_tokens.max(0) as u64;
            acc.cache_read_tokens += steps.cache_read_tokens.max(0) as u64;
            acc.cache_creation_tokens += steps.cache_creation_tokens.max(0) as u64;
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

    /// Agent usage per MAF step trace, hot rows plus tiered totals. MAF's own rows are left out:
    /// their cost is already `maf_executions.cost_usd`.
    async fn step_trace_usage(
        &self,
        trace_ids: &[String],
    ) -> Result<HashMap<String, StepTraceUsage>, ObservabilityError> {
        if trace_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<StepTraceUsage> = sqlx::query_as(
            r#"SELECT trace_id,
                      COALESCE(SUM(input_tokens), 0)::BIGINT          AS input_tokens,
                      COALESCE(SUM(output_tokens), 0)::BIGINT         AS output_tokens,
                      COALESCE(SUM(cache_read_tokens), 0)::BIGINT     AS cache_read_tokens,
                      COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
                      COALESCE(SUM(cost_usd), 0)::FLOAT8              AS cost_usd
               FROM (SELECT trace_id, input_tokens::bigint AS input_tokens, output_tokens::bigint AS output_tokens,
                            cache_read_tokens::bigint AS cache_read_tokens,
                            cache_creation_tokens::bigint AS cache_creation_tokens, cost_usd
                       FROM trace_usage
                      WHERE agent_name <> 'maf' AND agent_name NOT LIKE $2
                     UNION ALL
                     SELECT trace_id, input_tokens, output_tokens, cache_read_tokens,
                            cache_creation_tokens, cost_usd
                       FROM trace_totals) u
               WHERE trace_id = ANY($1)
               GROUP BY trace_id"#,
        )
        .bind(trace_ids)
        .bind(PLATFORM_STEP_ROW_PATTERN)
        .fetch_all(&self.db)
        .await
        .map_err(|e| ObservabilityError::Internal(e.to_string()))?;
        Ok(rows.into_iter().map(|r| (r.trace_id.clone(), r)).collect())
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
        access: FinopsAccess<'_>,
    ) -> Result<FinopsAttributionsResponse, ObservabilityError> {
        let start = parse_iso_or_default(start_time, 30);
        let end = end_time
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        let desc = sort_dir.map(|d| d != "asc").unwrap_or(true);

        if view == "workflow" {
            let mut rows = self
                .get_workflow_finops_rows(start, end, agent_name, access)
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
                    "", None, None, None, start_time, end_time, agent_name, model, provider,
                    access, None, "agent", false,
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
        user_id: uuid::Uuid,
    ) -> Result<InsightsResponseEnvelope, ObservabilityError> {
        let saved_key: Option<String> =
            nasiko_orchestrator::user_key::saved_openai_key(&self.db, user_id).await;

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
            "model": &self.config.capability_generator_model,
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": 200,
            "temperature": 0.3,
        });

        let json: Value = match saved_key {
            Some(api_key) => self.insights_with_user_key(&api_key, &body).await?,
            None => {
                let call = nasiko_orchestrator::llm_backend::LlmCallContext {
                    user_id,
                    component: INSIGHTS_COMPONENT.to_string(),
                    session_id: None,
                };
                self.llm_backend
                    .chat(&call, body)
                    .await
                    .map_err(|e| ObservabilityError::Internal(e.message))?
            }
        };

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

    /// Insights on the user's own saved OpenAI key; bypasses the router (user pays).
    async fn insights_with_user_key(
        &self,
        api_key: &str,
        body: &Value,
    ) -> Result<Value, ObservabilityError> {
        let base_url: &str = self
            .config
            .openai_base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1");
        // Inherited task model may be another provider's (e.g. Gemini `DEFAULT_MODEL`);
        // swap it only on the official OpenAI API, keep it on custom compatible endpoints.
        let mut body: Value = body.clone();
        if nasiko_orchestrator::providers::is_official_openai(base_url) {
            let requested: String = body["model"].as_str().unwrap_or_default().to_owned();
            let fallback: String = nasiko_orchestrator::providers::direct_openai_fallback();
            body["model"] = Value::String(nasiko_orchestrator::providers::direct_openai_model(
                &requested, &fallback,
            ));
        }
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
        resp.json()
            .await
            .map_err(|e| ObservabilityError::Deserialization(e.to_string()))
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
        let agents = match &agent_filter {
            Some(id) => Some(std::slice::from_ref(id)),
            None => accessible_agent_ids,
        };
        let mut rows = hours_meter::windowed_agent_hours(&self.db, start, end, agents)
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
/// The `trace_usage.agent_name`s the platform's own model calls are written under, which belong
/// to no registered agent: the chat orchestrator (`router::usage_meta::PricedTurn::persist`), the
/// routing selector (`nasiko_orchestrator` `write_selector_usage`) and MAF's run-level phases
/// (`executor::maf_agent_name(None)`) and every router-billed component; the list lives in
/// `PlatformComponent`. MAF's per-step phases match [`PLATFORM_STEP_ROW_PATTERN`].
fn platform_row_names() -> Vec<&'static str> {
    nasiko_orchestrator::llm_backend::PlatformComponent::all_names()
}

/// SQL `LIKE` pattern for MAF's per-step rows, `maf:step<i>` (`executor::maf_agent_name`).
const PLATFORM_STEP_ROW_PATTERN: &str = "maf:step%";

/// Whether `agent_name` is one of the platform's own rows: [`platform_row_names`], or a MAF
/// per-step row matching [`PLATFORM_STEP_ROW_PATTERN`].
fn is_platform_row_name(agent_name: &str) -> bool {
    nasiko_orchestrator::llm_backend::PlatformComponent::is_platform_row(agent_name)
}

/// The agent usage booked on one MAF step's trace (`step_trace_usage`), or a run's sum of them.
#[derive(sqlx::FromRow, Default)]
struct StepTraceUsage {
    trace_id: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    cost_usd: f64,
}

impl StepTraceUsage {
    fn add(&mut self, other: &Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_creation_tokens += other.cache_creation_tokens;
        self.cost_usd += other.cost_usd;
    }
}

/// The caller whose own platform rows a scoped FinOps view keeps, or `None` when it keeps none.
///
/// A scoped caller sees only the agents they may access, which a filter on agent name enforces;
/// platform rows carry no agent, so that filter dropped them all, and a member's own
/// orchestrator turns never reached their dashboard. They are matched on the row's user
/// instead, so a caller sees their own and never anyone else's. An unscoped caller already sees
/// every platform row, and a view narrowed to one agent or to the caller's own agents is about
/// agents, so neither needs this.
fn own_platform_rows_user(scoped: bool, narrowed: bool, caller: &str) -> Option<uuid::Uuid> {
    if !scoped || narrowed {
        return None;
    }
    caller.parse().ok()
}

fn empty_finops_response(
    total_container_hours: f64,
    total_sandbox_minutes: f64,
) -> FinopsDashboardResponse {
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
                total_sandbox_minutes,
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
            users: vec![],
        },
        status_code: 200,
        message: "FinOps dashboard data retrieved successfully".into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use nasiko_observability::Span;

    use super::*;

    #[test]
    fn a_scoped_caller_keeps_their_own_platform_rows() {
        let caller = uuid::Uuid::new_v4();
        assert_eq!(
            own_platform_rows_user(true, false, &caller.to_string()),
            Some(caller)
        );
    }

    #[test]
    fn an_unscoped_or_narrowed_view_adds_no_own_platform_rows() {
        let caller = uuid::Uuid::new_v4().to_string();
        assert_eq!(own_platform_rows_user(false, false, &caller), None);
        assert_eq!(own_platform_rows_user(true, true, &caller), None);
    }

    #[test]
    fn a_caller_without_a_user_id_sees_no_platform_rows() {
        assert_eq!(own_platform_rows_user(true, false, ""), None);
    }

    /// The names are matched by value in SQL, so they must stay the ones the writers use.
    #[test]
    fn platform_row_names_match_the_maf_writers() {
        use nasiko_orchestrator::maf::executor::maf_agent_name;
        assert!(platform_row_names().contains(&maf_agent_name(None).as_str()));
        let step_prefix = PLATFORM_STEP_ROW_PATTERN.trim_end_matches('%');
        assert!(maf_agent_name(Some(2)).starts_with(step_prefix));
    }

    /// Q1-3: the orchestrator's and MAF's rows carry an agent id, so the dashboard totals find
    /// them by name. An agent's own name must not match.
    #[test]
    fn platform_rows_are_recognised_by_name() {
        use nasiko_orchestrator::maf::executor::maf_agent_name;
        assert!(is_platform_row_name(
            nasiko_observability::types::ORCHESTRATOR_USAGE_AGENT
        ));
        assert!(is_platform_row_name("router_selection"));
        assert!(is_platform_row_name(&maf_agent_name(None)));
        assert!(is_platform_row_name(&maf_agent_name(Some(3))));
        assert!(!is_platform_row_name("crm-agent"));
        assert!(!is_platform_row_name("mafia-agent"));
    }

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

    fn span_with(id: &str, parent: Option<&str>, name: &str, attrs: &[(&str, &str)]) -> Span {
        Span {
            span_id: id.into(),
            parent_span_id: parent.map(String::from),
            name: name.into(),
            started_at: Utc.with_ymd_and_hms(2026, 11, 10, 12, 0, 0).unwrap(),
            ended_at: None,
            duration_ms: None,
            service_name: "agent".into(),
            kind: 3,
            status_code: 0,
            status_message: String::new(),
            attributes: attrs
                .iter()
                .map(|(k, v)| ((*k).into(), serde_json::json!(v)))
                .collect(),
            events: vec![],
        }
    }

    #[test]
    fn llm_span_is_named_after_the_model_that_answered() {
        // openai-v2 names the span after the model the agent asked for; the gateway
        // answered with the agent's configured one.
        let span = span_with(
            "llm",
            None,
            "chat gpt-4o-mini",
            &[
                ("gen_ai.operation.name", "chat"),
                ("gen_ai.request.model", "gpt-4o-mini"),
                ("gen_ai.response.model", "gpt-4.1"),
            ],
        );

        let (roots, _) = build_span_tree(&[span]);
        assert_eq!(roots[0].name, "chat gpt-4.1");
        assert_eq!(roots[0].model.as_deref(), Some("gpt-4.1"));
    }

    #[test]
    fn llm_span_without_a_response_model_takes_the_one_its_gateway_span_answered_with() {
        // The Rust and Go agents record only the requested model; the gateway's nested
        // span (under the agent's HTTP client span) records the one that answered.
        let agent = span_with(
            "llm",
            None,
            "chat gpt-4o-mini",
            &[
                ("gen_ai.operation.name", "chat"),
                ("gen_ai.request.model", "gpt-4o-mini"),
            ],
        );
        let http = span_with("http", Some("llm"), "POST", &[]);
        let gateway = span_with(
            "gateway",
            Some("http"),
            "gen_ai.chat",
            &[
                ("gen_ai.request.model", "gpt-4.1"),
                ("gen_ai.response.model", "gpt-4.1"),
            ],
        );

        let (roots, _) = build_span_tree(&[agent, http, gateway]);
        assert_eq!(roots[0].name, "chat gpt-4.1");
        assert_eq!(roots[0].model.as_deref(), Some("gpt-4.1"));
    }

    #[test]
    fn agent_state_read_that_finds_nothing_is_not_reported_as_an_error() {
        let mut miss = span_with(
            "miss",
            None,
            "GET",
            &[
                (
                    "http.url",
                    "http://10.24.0.15:8080/api/agent-state/kv/lgh%3Ac%3A",
                ),
                ("http.status_code", "404"),
            ],
        );
        miss.status_code = 2;
        let mut failed = span_with(
            "failed",
            None,
            "GET",
            &[
                ("http.url", "http://10.24.0.15:8080/api/agent-state/kv/k"),
                ("http.status_code", "500"),
            ],
        );
        failed.status_code = 2;

        let (_, lookup) = build_span_tree(&[miss, failed]);
        let status = |id: &str| lookup[&encode_span_id(id)].status_code.clone();
        assert_eq!(status("miss"), "UNSET");
        assert_eq!(status("failed"), "ERROR");
    }

    #[test]
    fn llm_span_takes_the_provider_its_gateway_span_routed_to() {
        // openai-v2 records `openai` and the model it asked for even when the gateway's
        // routing config sent the call to Gemini; its response model already says Gemini.
        let agent = span_with(
            "llm",
            None,
            "chat gpt-4o-mini",
            &[
                ("gen_ai.operation.name", "chat"),
                ("gen_ai.provider.name", "openai"),
                ("gen_ai.request.model", "gpt-4o-mini"),
                ("gen_ai.response.model", "gemini-4-hydrogen"),
            ],
        );
        let http = span_with("http", Some("llm"), "POST", &[]);
        let gateway = span_with(
            "gateway",
            Some("http"),
            "gen_ai.chat",
            &[
                ("gen_ai.agent.id", "93595357-5f48-44a7-a17e-c46e8a20cd82"),
                ("gen_ai.provider.name", "gemini"),
                ("gen_ai.request.model", "gemini-4-hydrogen"),
                ("gen_ai.response.model", "gemini-4-hydrogen"),
            ],
        );

        let (roots, _) = build_span_tree(&[agent, http, gateway]);
        assert_eq!(roots[0].name, "chat gemini-4-hydrogen");
        assert_eq!(roots[0].model.as_deref(), Some("gemini-4-hydrogen"));
        assert_eq!(roots[0].provider.as_deref(), Some("gemini"));
    }

    #[test]
    fn agent_span_naming_a_model_does_not_take_its_gateway_calls_answer() {
        // An uninstrumented LLM client leaves the gateway span directly under the
        // agent's `invoke_agent` span, which names a model but is not a model call.
        let agent = span_with(
            "agent",
            None,
            "invoke_agent hr",
            &[
                ("gen_ai.operation.name", "invoke_agent"),
                ("gen_ai.request.model", "gpt-4o-mini"),
            ],
        );
        let gateway = span_with(
            "gateway",
            Some("agent"),
            "gen_ai.chat",
            &[("gen_ai.response.model", "gpt-4.1")],
        );

        let (roots, _) = build_span_tree(&[agent, gateway]);
        assert_eq!(roots[0].name, "invoke_agent hr");
        assert_eq!(roots[0].model.as_deref(), Some("gpt-4o-mini"));
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
            sandbox_minutes: 0.0,
            previous: None,
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
