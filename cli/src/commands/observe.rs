use anyhow::Result;
use nasiko_utils::display::{opt_cost, opt_dash, opt_lat_ms, opt_round, opt_started, trunc};
use serde::Deserialize;
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

use crate::api::Client;

// ─── Response types (ObservabilityService) ───────────────────────────────────

#[derive(Deserialize, Tabled)]
struct SessionSummary {
    #[tabled(rename = "SESSION ID")]
    session_id: String,
    #[tabled(rename = "AGENT", display("trunc", 20))]
    agent_id: String,
    #[tabled(rename = "STARTED", display = "opt_started")]
    start_time: Option<String>,
    #[tabled(rename = "DUR(ms)", display = "opt_dash")]
    duration_ms: Option<u64>,
    #[tabled(rename = "TRACES", display = "opt_dash")]
    #[serde(default)]
    num_traces: Option<u32>,
    #[tabled(rename = "TOKENS", display = "token_total")]
    #[serde(default)]
    token_usage: TokenUsageSummary,
    #[tabled(rename = "p50(ms)", display = "opt_round")]
    #[serde(default)]
    trace_latency_ms_p50: Option<f64>,
    #[tabled(rename = "p99(ms)", display = "opt_round")]
    #[serde(default)]
    trace_latency_ms_p99: Option<f64>,
    #[tabled(rename = "COST", display = "session_cost")]
    #[serde(default)]
    cost_summary: SimpleCostSummary,
}

fn token_total(t: &TokenUsageSummary) -> String {
    opt_dash(&t.total)
}

fn session_cost(c: &SimpleCostSummary) -> String {
    opt_cost(&c.total.cost)
}

#[derive(Deserialize, Default)]
struct TokenUsageSummary {
    total: Option<u64>,
}

#[derive(Deserialize, Default)]
struct SimpleCostSummary {
    total: CostEntry,
}

#[derive(Deserialize, Default)]
struct CostEntry {
    cost: Option<f64>,
}

#[derive(Deserialize)]
struct SessionListResponse {
    data: SessionListData,
}

#[derive(Deserialize)]
struct SessionListData {
    sessions: Vec<SessionSummary>,
    total_agents: usize,
    successful_agents: usize,
}

#[derive(Deserialize)]
struct SessionDetailResponse {
    data: SessionDetailData,
}

#[derive(Deserialize)]
struct SessionDetailData {
    session: SessionDetail,
}

#[derive(Deserialize)]
struct SessionDetail {
    session_id: String,
    num_traces: usize,
    latency_p50: Option<f64>,
    #[serde(default)]
    token_usage: TokenUsageSummary,
    cost_summary: FullCostSummary,
    traces: Vec<TraceEntry>,
}

#[derive(Deserialize)]
struct FullCostSummary {
    total: CostWithTokens,
    prompt: CostWithTokens,
    completion: CostWithTokens,
}

#[derive(Deserialize)]
struct CostWithTokens {
    cost: f64,
    tokens: u64,
}

#[derive(Deserialize, Tabled)]
struct TraceEntry {
    #[tabled(rename = "TRACE ID", display("trunc", 32))]
    trace_id: String,
    #[tabled(inline)]
    root_span: RootSpanEntry,
}

#[derive(Deserialize, Tabled)]
struct RootSpanEntry {
    #[tabled(rename = "ROOT SPAN", display("trunc", 16))]
    span_id: String,
    #[tabled(rename = "STARTED", display = "opt_started")]
    start_time: Option<String>,
    #[tabled(rename = "LAT(ms)", format = "{:.0}")]
    latency_ms: f64,
    #[tabled(rename = "TOKENS")]
    cumulative_token_count_total: u64,
}

#[derive(Deserialize)]
struct TraceDetailResponse {
    data: TraceDetailData,
}

#[derive(Deserialize)]
struct TraceDetailData {
    trace: TraceDetail,
}

#[derive(Deserialize)]
struct TraceDetail {
    id: String,
    #[serde(default)]
    project_session_id: Option<String>,
    num_spans: usize,
    latency_ms: Option<f64>,
    cost_summary: NestedCostSummary,
    spans: Vec<SpanNode>,
}

#[derive(Deserialize)]
struct NestedCostSummary {
    total: CostOnly,
    prompt: CostOnly,
    completion: CostOnly,
}

#[derive(Deserialize)]
struct CostOnly {
    cost: f64,
}

#[derive(Deserialize, Tabled)]
struct SpanNode {
    #[tabled(rename = "SPAN ID", display("trunc", 16))]
    span_id: String,
    #[tabled(rename = "KIND", display("trunc", 8))]
    span_kind: String,
    #[tabled(rename = "MODEL", display("model_or_dash", 20))]
    #[serde(default)]
    model: Option<String>,
    #[tabled(rename = "LAT(ms)", display = "opt_round")]
    latency_ms: Option<f64>,
    #[tabled(rename = "TOK IN")]
    #[serde(default)]
    input_tokens: u64,
    #[tabled(rename = "TOK OUT")]
    #[serde(default)]
    output_tokens: u64,
    #[tabled(rename = "STARTED", display = "opt_started")]
    start_time: Option<String>,
    // Blank for the overwhelmingly common UNSET/OK — only errors deserve ink.
    #[tabled(rename = "STATUS", display = "status_if_notable")]
    status_code: String,
    #[tabled(rename = "NAME")]
    name: String,
    #[tabled(skip)]
    #[serde(default)]
    children: Vec<SpanNode>,
}

fn model_or_dash(m: &Option<String>, max: usize) -> String {
    match m {
        Some(s) => trunc(s, max),
        None => "-".to_owned(),
    }
}

fn status_if_notable(code: &str) -> String {
    match code {
        "UNSET" | "OK" => String::new(),
        other => other.to_owned(),
    }
}

#[derive(Deserialize)]
struct SpanDetailResponse {
    data: SpanDetailData,
}

#[derive(Deserialize)]
struct SpanDetailData {
    span: SpanDetail,
}

#[derive(Deserialize)]
struct SpanTraceRef {
    trace_id: String,
}

#[derive(Deserialize)]
struct SpanDetail {
    span_id: String,
    trace: SpanTraceRef,
    name: String,
    span_kind: String,
    status_code: String,
    #[serde(default)]
    status_message: String,
    start_time: Option<String>,
    end_time: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
    latency_ms: Option<f64>,
    token_count_total: u64,
    input: ContentField,
    output: ContentField,
    /// Nested JSON object — server unflattenss dot-separated keys into a tree.
    attributes: serde_json::Value,
}

#[derive(Deserialize)]
struct ContentField {
    value: String,
}

#[derive(Deserialize)]
struct ProjectStatsResponse {
    data: ProjectStatsData,
}

#[derive(Deserialize)]
struct ProjectStatsData {
    project: AgentProjectStats,
}

#[derive(Deserialize)]
struct AgentProjectStats {
    id: String,
    trace_count: usize,
    cost_summary: NestedCostSummary,
    latency_ms_p50: Option<f64>,
    latency_ms_p99: Option<f64>,
}

#[derive(Deserialize)]
struct FinopsDashboardResponse {
    data: FinopsDashboardData,
}

#[derive(Deserialize)]
struct FinopsDashboardData {
    summary: FinopsSummary,
    agents: Vec<AgentFinopsRow>,
    token_usage: FinopsTokenUsage,
}

#[derive(Deserialize)]
struct FinopsSummary {
    total_cost: f64,
    total_operations: usize,
    #[serde(default)]
    operations_last_24h: usize,
    average_cost: f64,
    active_agents: usize,
    total_agents: usize,
}

#[derive(Deserialize, Tabled)]
struct AgentFinopsRow {
    #[tabled(rename = "AGENT ID", display("trunc", 24))]
    agent_id: String,
    #[tabled(rename = "NAME", display("name_ver", &self.version))]
    agent_name: String,
    #[tabled(rename = "OPS")]
    operations: usize,
    #[tabled(rename = "PROMPT TOK")]
    prompt_tokens: u64,
    #[tabled(rename = "COMPL TOK")]
    completion_tokens: u64,
    #[tabled(rename = "TOTAL TOK")]
    total_tokens: u64,
    #[tabled(rename = "AVG LAT", display = "opt_lat_ms")]
    avg_latency_ms: Option<f64>,
    #[tabled(rename = "TOTAL $", format = "${:.4}")]
    total_cost: f64,
    #[tabled(rename = "COST/OP", format = "${:.4}")]
    #[serde(default)]
    avg_cost_per_operation: f64,
    #[tabled(skip)]
    #[serde(default)]
    version: Option<String>,
}

fn name_ver(name: &String, version: &Option<String>) -> String {
    let full = match version.as_deref() {
        Some(v) if !v.is_empty() => format!("{name} ({v})"),
        _ => name.clone(),
    };
    trunc(&full, 20)
}

#[derive(Deserialize)]
struct FinopsTokenUsage {
    total_tokens: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
    #[serde(default)]
    avg_tokens_per_operation: u64,
}

#[derive(Deserialize)]
struct InsightsResponse {
    insights: Vec<String>,
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Flatten a nested JSON object back to dot-separated key/value pairs.
/// e.g. `{"gen_ai": {"usage": {"input_tokens": 312}}}` → `("gen_ai.usage.input_tokens", "312")`
fn flatten_json(prefix: &str, val: &serde_json::Value, out: &mut Vec<(String, String)>) {
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_json(&key, v, out);
            }
        }
        other => {
            // Strip surrounding quotes from JSON strings for cleaner output.
            let display = match other {
                serde_json::Value::String(s) => s.clone(),
                v => v.to_string(),
            };
            out.push((prefix.to_string(), display));
        }
    }
}

// ─── Commands (protected_router / ObservabilityService) ───────────────────────

/// List sessions across all agents via ObservabilityService.
///
/// Hits: GET /api/observability/session/list
/// Fetch `path` and pretty-print the raw API response (for `--json`).
fn print_raw_json(client: &Client, path: &str) -> Result<()> {
    let raw: serde_json::Value = client.get_json(path)?;
    println!("{}", serde_json::to_string_pretty(&raw)?);
    Ok(())
}

pub fn sessions(start_time: Option<&str>, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let path = match start_time {
        Some(t) => format!("/observability/session/list?start_time={}", crate::api::urlencode(t)),
        None => "/observability/session/list".to_string(),
    };
    if json {
        return print_raw_json(&client, &path);
    }

    let resp: SessionListResponse = client.get_json(&path)?;
    let data = resp.data;

    println!(
        "Agents: {}/{} responding",
        data.successful_agents, data.total_agents
    );
    println!();

    if data.sessions.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    println!("{}", Table::new(&data.sessions).with(Style::blank()).with(Alignment::left()));
    println!("\n{} session(s).", data.sessions.len());
    Ok(())
}

/// Show full detail for a session including all traces.
///
/// Hits: GET /api/observability/session/{session_id}
pub fn session_detail(session_id: &str, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let path = format!("/observability/session/{session_id}");
    if json {
        return print_raw_json(&client, &path);
    }
    let resp: SessionDetailResponse = client.get_json(&path)?;
    let s = resp.data.session;

    println!("Session:    {}", s.session_id);
    println!("Traces:     {}", s.num_traces);
    if let Some(p50) = s.latency_p50 {
        println!("p50 lat:    {:.0} ms", p50);
    }
    println!("Tokens:     {} total  ({} prompt / {} completion)",
        s.token_usage.total.unwrap_or(0),
        s.cost_summary.prompt.tokens,
        s.cost_summary.completion.tokens,
    );
    println!("Cost:       ${:.6}  (prompt ${:.6} / completion ${:.6})",
        s.cost_summary.total.cost,
        s.cost_summary.prompt.cost,
        s.cost_summary.completion.cost,
    );
    println!();

    if s.traces.is_empty() {
        println!("No traces.");
        return Ok(());
    }

    println!("{}", Table::new(&s.traces).with(Style::blank()).with(Alignment::left()));
    Ok(())
}

/// Show full trace detail (span tree + costs) via ObservabilityService.
///
/// Hits: GET /api/observability/trace/{trace_id}
pub fn trace_detail(trace_id: &str, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let path = format!("/observability/trace/{trace_id}");
    if json {
        return print_raw_json(&client, &path);
    }
    let resp: TraceDetailResponse = client.get_json(&path)?;
    let t = resp.data.trace;

    println!("Trace:    {}", t.id);
    if let Some(ref sid) = t.project_session_id {
        println!("Session:  {}", sid);
    }
    println!("Spans:    {}", t.num_spans);
    if let Some(lat) = t.latency_ms {
        println!("Latency:  {:.0} ms", lat);
    }
    println!("Cost:     ${:.6}  (prompt ${:.6} / completion ${:.6})",
        t.cost_summary.total.cost,
        t.cost_summary.prompt.cost,
        t.cost_summary.completion.cost,
    );
    println!();

    if t.spans.is_empty() {
        println!("No spans.");
        return Ok(());
    }

    // The server returns a nested tree: t.spans contains only root nodes,
    // children are embedded in SpanNode.children (not repeated in the top-level slice).
    // Flatten it, baking the tree depth into the NAME column as indentation.
    fn flatten_span_node(span: &SpanNode, depth: usize, out: &mut Vec<SpanNode>) {
        out.push(SpanNode {
            status_code: span.status_code.clone(),
            span_id: span.span_id.clone(),
            span_kind: span.span_kind.clone(),
            model: span.model.clone(),
            latency_ms: span.latency_ms,
            input_tokens: span.input_tokens,
            output_tokens: span.output_tokens,
            start_time: span.start_time.clone(),
            name: format!("{}{}", "  ".repeat(depth), span.name),
            children: Vec::new(),
        });
        for child in &span.children {
            flatten_span_node(child, depth + 1, out);
        }
    }
    let mut rows = Vec::new();
    for root in &t.spans {
        flatten_span_node(root, 0, &mut rows);
    }
    println!("{}", Table::new(&rows).with(Style::blank()).with(Alignment::left()));

    Ok(())
}

/// Show detail for a single span including prompt/completion content.
///
/// Hits: GET /api/observability/span/{trace_id}/{span_id}
pub fn span_detail(trace_id: &str, span_id: &str, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let path = format!("/observability/span/{trace_id}/{span_id}");
    if json {
        return print_raw_json(&client, &path);
    }
    let resp: SpanDetailResponse = client.get_json(&path)?;
    let s = resp.data.span;

    println!("Span:       {}", s.span_id);
    println!("Trace:      {}", s.trace.trace_id);
    if let Some(p) = &s.parent_id {
        println!("Parent:     {}", p);
    }
    println!("Name:       {}", s.name);
    println!("Kind:       {}", s.span_kind);
    println!("Status:     {}", s.status_code);
    if !s.status_message.is_empty() {
        println!("Message:    {}", s.status_message);
    }
    if let Some(t) = &s.start_time {
        println!("Started:    {}", t.get(..19).unwrap_or(t));
    }
    if let Some(t) = &s.end_time {
        println!("Ended:      {}", t.get(..19).unwrap_or(t));
    }
    if let Some(lat) = s.latency_ms {
        println!("Latency:    {:.0} ms", lat);
    }
    println!("Tokens:     {}", s.token_count_total);
    println!();

    if !s.input.value.is_empty() {
        println!("── Input ──────────────────────────────────────────");
        println!("{}", s.input.value);
        println!();
    }
    if !s.output.value.is_empty() {
        println!("── Output ─────────────────────────────────────────");
        println!("{}", s.output.value);
        println!();
    }

    let mut flat_attrs: Vec<(String, String)> = Vec::new();
    flatten_json("", &s.attributes, &mut flat_attrs);
    flat_attrs.sort_by(|a, b| a.0.cmp(&b.0));
    let gen_ai_attrs: Vec<_> = flat_attrs
        .iter()
        .filter(|(k, _)| k.starts_with("gen_ai.") || k.starts_with("llm.") || k.starts_with("openinference."))
        .collect();
    if !gen_ai_attrs.is_empty() {
        println!("── Attributes ─────────────────────────────────────");
        for (k, v) in &gen_ai_attrs {
            println!("  {k}: {v}");
        }
    }
    Ok(())
}

/// Show project-level stats for an agent via ObservabilityService.
///
/// Hits: GET /api/observability/agent/{agent_id}/stats?start_time=...
pub fn project_stats(agent_id: &str, start_time: Option<&str>, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let t = start_time.unwrap_or("");
    let path = if t.is_empty() {
        // server requires start_time — default to 24h ago
        let default = (chrono::Utc::now() - chrono::Duration::hours(24))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        format!("/observability/agent/{agent_id}/stats?start_time={}", crate::api::urlencode(&default))
    } else {
        format!("/observability/agent/{agent_id}/stats?start_time={}", crate::api::urlencode(t))
    };
    if json {
        return print_raw_json(&client, &path);
    }

    let resp: ProjectStatsResponse = client.get_json(&path)?;
    let p = resp.data.project;

    println!("Agent ID:   {}", p.id);
    println!("Traces:     {}", p.trace_count);
    if let Some(p50) = p.latency_ms_p50 {
        println!("p50 lat:    {:.0} ms", p50);
    }
    if let Some(p99) = p.latency_ms_p99 {
        println!("p99 lat:    {:.0} ms", p99);
    }
    println!("Cost:       ${:.6}  (prompt ${:.6} / completion ${:.6})",
        p.cost_summary.total.cost,
        p.cost_summary.prompt.cost,
        p.cost_summary.completion.cost,
    );
    Ok(())
}

/// Show the FinOps cost dashboard via ObservabilityService.
///
/// Hits: GET /api/observability/finops/dashboard
pub fn finops_dashboard(start_time: Option<&str>, json: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let path = match start_time {
        Some(t) => format!("/observability/finops/dashboard?start_time={}", crate::api::urlencode(t)),
        None => "/observability/finops/dashboard".to_string(),
    };
    if json {
        return print_raw_json(&client, &path);
    }

    let resp: FinopsDashboardResponse = client.get_json(&path)?;
    let data = resp.data;
    let s = &data.summary;

    println!("Total cost:   ${:.4}", s.total_cost);
    println!("Operations:   {} total  ({} last 24h, avg ${:.4}/op)", s.total_operations, s.operations_last_24h, s.average_cost);
    println!("Agents:       {} active / {} total", s.active_agents, s.total_agents);
    println!("Tokens:       {} total  ({} prompt / {} completion, avg {}/op)",
        data.token_usage.total_tokens,
        data.token_usage.prompt_tokens,
        data.token_usage.completion_tokens,
        data.token_usage.avg_tokens_per_operation,
    );
    println!();

    if data.agents.is_empty() {
        println!("No agent data.");
        return Ok(());
    }

    println!("{}", Table::new(&data.agents).with(Style::blank()).with(Alignment::left()));
    Ok(())
}

/// Fetch AI-powered cost insights.
///
/// Internally calls /finops/dashboard to gather data, then posts to
/// /finops/insights.
///
/// Hits: GET /api/observability/finops/dashboard
///       POST /api/observability/finops/insights
pub fn insights(start_time: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;

    // Step 1: fetch dashboard data to use as input.
    let path = match start_time {
        Some(t) => format!("/observability/finops/dashboard?start_time={}", crate::api::urlencode(t)),
        None => "/observability/finops/dashboard".to_string(),
    };
    let dashboard: serde_json::Value = client.get_json(&path)?;

    let kpi = dashboard.pointer("/data/summary").cloned().unwrap_or(serde_json::json!({}));
    let agent_costs = dashboard.pointer("/data/agents")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Step 2: post to insights.
    let body = serde_json::json!({ "kpi": kpi, "agent_costs": agent_costs });
    let resp: InsightsResponse = client.post_json("/observability/finops/insights", &body)?;

    if resp.insights.is_empty() {
        println!("No insights returned.");
        return Ok(());
    }

    println!("Cost insights:");
    println!("{}", "-".repeat(70));
    for (i, insight) in resp.insights.iter().enumerate() {
        println!("{}. {}", i + 1, insight);
    }
    Ok(())
}