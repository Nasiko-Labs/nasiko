#![allow(dead_code)]
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;

use crate::api::Client;

// ─── Response types (mirrors oss/observability/src/types.rs + routes.rs) ────

#[derive(Debug, Deserialize)]
struct AgentStatsResponse {
    agent_id: String,
    period_start: DateTime<Utc>,
    total_requests: u64,
    error_rate: f64,
    avg_latency_ms: f64,
    #[serde(default)]
    p50_latency_ms: Option<f64>,
    #[serde(default)]
    p95_latency_ms: Option<f64>,
    total_input_tokens: u64,
    total_output_tokens: u64,
    source: String,
}

#[derive(Debug, Deserialize)]
struct Session {
    trace_id: String,
    #[serde(default)]
    agent_ids: Vec<String>,
    started_at: DateTime<Utc>,
    #[serde(default)]
    ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    span_count: u32,
    #[serde(default)]
    total_input_tokens: u64,
    #[serde(default)]
    total_output_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct Span {
    span_id: String,
    #[serde(default)]
    parent_span_id: Option<String>,
    name: String,
    started_at: DateTime<Utc>,
    #[serde(default)]
    duration_ms: Option<u64>,
    service_name: String,
    #[serde(default)]
    status_code: u8,
    #[serde(default)]
    attributes: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct TraceDetails {
    trace_id: String,
    #[serde(default)]
    spans: Vec<Span>,
    #[serde(default)]
    started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    duration_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct AgentFinOps {
    agent_id: String,
    total_input_tokens: u64,
    total_output_tokens: u64,
    estimated_cost_usd: f64,
    request_count: u64,
}

#[derive(Debug, Deserialize)]
struct FinOpsDashboard {
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
    #[serde(default)]
    agents: Vec<AgentFinOps>,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_estimated_cost_usd: f64,
}

// ─── Commands ────────────────────────────────────────────────────────────────

/// Show performance stats for a single agent.
///
/// Hits: GET /api/observability/agents/{agent}/stats
pub fn stats(agent: &str, since: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let mut path = format!("/observability/agents/{agent}/stats");
    if let Some(s) = since {
        path.push_str(&format!("?since={}", crate::api::urlencode(s)));
    }

    let s: AgentStatsResponse = client.get_json(&path)?;

    println!("Agent:        {}", s.agent_id);
    println!("Period start: {}", s.period_start.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("Source:       {}", s.source);
    println!();
    println!("Requests:     {}", s.total_requests);
    println!("Error rate:   {:.1}%", s.error_rate * 100.0);
    println!("Avg latency:  {:.0} ms", s.avg_latency_ms);
    if let Some(p50) = s.p50_latency_ms {
        println!("p50 latency:  {:.0} ms", p50);
    }
    if let Some(p95) = s.p95_latency_ms {
        println!("p95 latency:  {:.0} ms", p95);
    }
    println!();
    println!("Input tokens: {}", s.total_input_tokens);
    println!("Output tokens:{}", s.total_output_tokens);
    println!("Total tokens: {}", s.total_input_tokens + s.total_output_tokens);

    Ok(())
}

/// List recent distributed trace sessions.
///
/// Hits: GET /api/observability/traces
pub fn traces(agent_id: Option<&str>, session_id: Option<&str>, since: Option<&str>, limit: usize) -> Result<()> {
    let client = Client::from_active_cluster()?;

    let mut params: Vec<String> = vec![format!("limit={limit}")];
    if let Some(id) = agent_id {
        params.push(format!("agent_id={}", crate::api::urlencode(id)));
    }
    if let Some(sid) = session_id {
        params.push(format!("session_id={}", crate::api::urlencode(sid)));
    }
    if let Some(s) = since {
        params.push(format!("since={}", crate::api::urlencode(s)));
    }
    let path = format!("/observability/traces?{}", params.join("&"));

    let sessions: Vec<Session> = client.get_json(&path)?;

    if sessions.is_empty() {
        println!("No traces found.");
        return Ok(());
    }

    println!(
        "{:<34} {:<30} {:<10} {:<8} {:<12} TOKENS (in/out)",
        "TRACE ID", "AGENTS", "STARTED", "DUR(ms)", "SPANS"
    );
    println!("{}", "-".repeat(115));

    for s in &sessions {
        let agents = s.agent_ids.join(", ");
        let agents_col = if agents.len() > 28 {
            format!("{}…", &agents[..27])
        } else {
            agents
        };
        let started = s.started_at.format("%m-%d %H:%M:%S").to_string();
        let dur = s.duration_ms.map(|d| d.to_string()).unwrap_or_else(|| "-".into());
        println!(
            "{:<34} {:<30} {:<10} {:<8} {:<12} {}/{}",
            &s.trace_id[..s.trace_id.len().min(32)],
            agents_col,
            started,
            dur,
            s.span_count,
            s.total_input_tokens,
            s.total_output_tokens,
        );
    }
    println!("\n{} trace(s).", sessions.len());

    Ok(())
}

/// Show full span tree for a trace.
///
/// Hits: GET /api/observability/traces/{trace_id}
pub fn trace(trace_id: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let td: TraceDetails = client.get_json(&format!("/observability/traces/{trace_id}"))?;

    println!("Trace:    {}", td.trace_id);
    if let Some(s) = td.started_at {
        println!("Started:  {}", s.format("%Y-%m-%d %H:%M:%S UTC"));
    }
    if let Some(e) = td.ended_at {
        println!("Ended:    {}", e.format("%Y-%m-%d %H:%M:%S UTC"));
    }
    if let Some(d) = td.duration_ms {
        println!("Duration: {} ms", d);
    }
    println!("Spans:    {}", td.spans.len());
    println!();

    if td.spans.is_empty() {
        println!("No spans.");
        return Ok(());
    }

    // Build a map from span_id → children for tree rendering.
    let mut children: HashMap<Option<String>, Vec<&Span>> = HashMap::new();
    for span in &td.spans {
        children
            .entry(span.parent_span_id.clone())
            .or_default()
            .push(span);
    }

    // Print header.
    println!(
        "{:<5} {:<18} {:<8} {:<28} {}",
        "CODE", "SPAN ID", "DUR(ms)", "SERVICE", "NAME"
    );
    println!("{}", "-".repeat(90));

    // Recursive DFS printer.
    fn print_spans(
        parent: Option<&str>,
        children: &HashMap<Option<String>, Vec<&Span>>,
        depth: usize,
    ) {
        let key = parent.map(|s| s.to_string());
        let Some(kids) = children.get(&key) else { return };
        for span in kids {
            let indent = "  ".repeat(depth);
            let status = match span.status_code {
                2 => "ERR",
                1 => "OK ",
                _ => "   ",
            };
            let dur = span
                .duration_ms
                .map(|d| d.to_string())
                .unwrap_or_else(|| "-".into());
            let span_short = span.span_id.get(..16).unwrap_or(&span.span_id);
            println!(
                "{:<5} {:<18} {:<8} {:<28} {}{}",
                status,
                span_short,
                dur,
                &span.service_name[..span.service_name.len().min(27)],
                indent,
                span.name,
            );
            // Print gen_ai token attrs if present.
            let in_tok = span
                .attributes
                .get("gen_ai.usage.input_tokens")
                .and_then(|v| v.as_u64());
            let out_tok = span
                .attributes
                .get("gen_ai.usage.output_tokens")
                .and_then(|v| v.as_u64());
            if in_tok.is_some() || out_tok.is_some() {
                println!(
                    "{:<5} {:<18} {:<8} {:<28} {}  tokens: in={} out={}",
                    "",
                    "",
                    "",
                    "",
                    indent,
                    in_tok.unwrap_or(0),
                    out_tok.unwrap_or(0),
                );
            }
            print_spans(Some(&span.span_id), children, depth + 1);
        }
    }

    print_spans(None, &children, 0);

    Ok(())
}

/// Show FinOps cost dashboard across agents.
///
/// Hits: GET /api/observability/finops
pub fn finops(since: Option<&str>, agent_ids: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;

    let mut params: Vec<String> = Vec::new();
    if let Some(s) = since {
        params.push(format!("since={}", crate::api::urlencode(s)));
    }
    if let Some(ids) = agent_ids {
        params.push(format!("agent_ids={}", crate::api::urlencode(ids)));
    }
    let path = if params.is_empty() {
        "/observability/finops".to_string()
    } else {
        format!("/observability/finops?{}", params.join("&"))
    };

    let dash: FinOpsDashboard = client.get_json(&path)?;

    println!(
        "Period: {} → {}",
        dash.period_start.format("%Y-%m-%d %H:%M UTC"),
        dash.period_end.format("%Y-%m-%d %H:%M UTC"),
    );
    println!();

    if dash.agents.is_empty() {
        println!("No cost data available.");
        return Ok(());
    }

    println!(
        "{:<36} {:<10} {:<14} {:<14} {:<12}",
        "AGENT", "REQUESTS", "INPUT TOKENS", "OUTPUT TOKENS", "COST (USD)"
    );
    println!("{}", "-".repeat(90));

    for a in &dash.agents {
        println!(
            "{:<36} {:<10} {:<14} {:<14} ${:.4}",
            &a.agent_id[..a.agent_id.len().min(34)],
            a.request_count,
            a.total_input_tokens,
            a.total_output_tokens,
            a.estimated_cost_usd,
        );
    }

    println!("{}", "-".repeat(90));
    println!(
        "{:<36} {:<10} {:<14} {:<14} ${:.4}",
        "TOTAL",
        "",
        dash.total_input_tokens,
        dash.total_output_tokens,
        dash.total_estimated_cost_usd,
    );

    Ok(())
}

// ─── Response types for protected_router (ObservabilityService) ──────────────

#[derive(Deserialize)]
struct SessionSummary {
    session_id: String,
    agent_id: String,
    start_time: Option<String>,
    duration_ms: Option<u64>,
    #[serde(default)]
    token_usage: TokenUsageSummary,
    #[serde(default)]
    cost_summary: SimpleCostSummary,
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

#[derive(Deserialize)]
struct TraceEntry {
    trace_id: String,
    root_span: RootSpanEntry,
}

#[derive(Deserialize)]
struct RootSpanEntry {
    span_id: String,
    latency_ms: f64,
    start_time: Option<String>,
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

#[derive(Deserialize, Clone)]
struct SpanNode {
    span_id: String,
    name: String,
    span_kind: String,
    status_code: String,
    start_time: Option<String>,
    latency_ms: Option<f64>,
    token_count_total: u64,
    parent_id: Option<String>,
    #[serde(default)]
    children: Vec<SpanNode>,
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
struct SpanDetail {
    span_id: String,
    trace_id: String,
    name: String,
    span_kind: String,
    status_code: String,
    start_time: Option<String>,
    latency_ms: Option<f64>,
    token_count_total: u64,
    input: ContentField,
    output: ContentField,
    attributes: HashMap<String, serde_json::Value>,
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
    average_cost: f64,
    active_agents: usize,
    total_agents: usize,
}

#[derive(Deserialize)]
struct AgentFinopsRow {
    agent_id: String,
    agent_name: String,
    total_cost: f64,
    operations: usize,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    avg_latency_ms: Option<f64>,
}

#[derive(Deserialize)]
struct FinopsTokenUsage {
    total_tokens: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
}

#[derive(Deserialize)]
struct InsightsResponse {
    insights: Vec<String>,
}

// ─── Commands (protected_router / ObservabilityService) ───────────────────────

/// List sessions across all agents via ObservabilityService.
///
/// Hits: GET /api/observability/session/list
pub fn sessions(start_time: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let path = match start_time {
        Some(t) => format!("/observability/session/list?start_time={}", crate::api::urlencode(t)),
        None => "/observability/session/list".to_string(),
    };

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

    println!(
        "{:<34} {:<24} {:<22} {:<10} {:<10} COST",
        "SESSION ID", "AGENT", "STARTED", "DUR(ms)", "TOKENS"
    );
    println!("{}", "-".repeat(110));

    for s in &data.sessions {
        let started = s.start_time.as_deref().unwrap_or("-");
        let started_short = started.get(..19).unwrap_or(started);
        let dur = s.duration_ms.map(|d| d.to_string()).unwrap_or_else(|| "-".into());
        let tokens = s.token_usage.total.map(|t| t.to_string()).unwrap_or_else(|| "-".into());
        let cost = s.cost_summary.total.cost.map(|c| format!("${c:.4}")).unwrap_or_else(|| "-".into());
        println!(
            "{:<34} {:<24} {:<22} {:<10} {:<10} {}",
            &s.session_id[..s.session_id.len().min(32)],
            &s.agent_id[..s.agent_id.len().min(22)],
            started_short,
            dur,
            tokens,
            cost,
        );
    }
    println!("\n{} session(s).", data.sessions.len());
    Ok(())
}

/// Show full detail for a session including all traces.
///
/// Hits: GET /api/observability/session/{session_id}
pub fn session_detail(session_id: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: SessionDetailResponse =
        client.get_json(&format!("/observability/session/{session_id}"))?;
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

    println!("{:<34} {:<18} {:<22} {:<10} TOKENS", "TRACE ID", "ROOT SPAN", "STARTED", "LAT(ms)");
    println!("{}", "-".repeat(95));
    for t in &s.traces {
        let rs = &t.root_span;
        let started = rs.start_time.as_deref().unwrap_or("-");
        println!(
            "{:<34} {:<18} {:<22} {:<10} {}",
            &t.trace_id[..t.trace_id.len().min(32)],
            &rs.span_id[..rs.span_id.len().min(16)],
            started.get(..19).unwrap_or(started),
            format!("{:.0}", rs.latency_ms),
            rs.cumulative_token_count_total,
        );
    }
    Ok(())
}

/// Show full trace detail (span tree + costs) via ObservabilityService.
///
/// Hits: GET /api/observability/trace/{project_id}/{trace_id}
pub fn trace_detail(project_id: &str, trace_id: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: TraceDetailResponse =
        client.get_json(&format!("/observability/trace/{project_id}/{trace_id}"))?;
    let t = resp.data.trace;

    println!("Trace:    {}", t.id);
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

    println!("{:<5} {:<18} {:<10} {:<12} {:<10} NAME", "CODE", "SPAN ID", "KIND", "LAT(ms)", "TOKENS");
    println!("{}", "-".repeat(85));

    // The server returns a nested tree: t.spans contains only root nodes,
    // children are embedded in SpanNode.children (not repeated in the top-level slice).
    fn print_span_node(span: &SpanNode, depth: usize) {
        let indent = "  ".repeat(depth);
        let lat = span.latency_ms.map(|l| format!("{l:.0}")).unwrap_or_else(|| "-".into());
        println!(
            "{:<5} {:<18} {:<10} {:<12} {:<10} {}{}",
            &span.status_code[..span.status_code.len().min(4)],
            &span.span_id[..span.span_id.len().min(16)],
            &span.span_kind[..span.span_kind.len().min(8)],
            lat,
            span.token_count_total,
            indent,
            span.name,
        );
        for child in &span.children {
            print_span_node(child, depth + 1);
        }
    }
    for root in &t.spans {
        print_span_node(root, 0);
    }

    Ok(())
}

/// Show detail for a single span including prompt/completion content.
///
/// Hits: GET /api/observability/span/{trace_id}/{span_id}
pub fn span_detail(trace_id: &str, span_id: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: SpanDetailResponse =
        client.get_json(&format!("/observability/span/{trace_id}/{span_id}"))?;
    let s = resp.data.span;

    println!("Span:       {}", s.span_id);
    println!("Trace:      {}", s.trace_id);
    println!("Name:       {}", s.name);
    println!("Kind:       {}", s.span_kind);
    println!("Status:     {}", s.status_code);
    if let Some(t) = &s.start_time {
        println!("Started:    {}", t.get(..19).unwrap_or(t));
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

    let gen_ai_attrs: Vec<_> = s.attributes.iter()
        .filter(|(k, _)| k.starts_with("gen_ai.") || k.starts_with("llm."))
        .collect();
    if !gen_ai_attrs.is_empty() {
        println!("── Attributes ─────────────────────────────────────");
        for (k, v) in gen_ai_attrs {
            println!("  {k}: {v}");
        }
    }
    Ok(())
}

/// Show project-level stats for an agent via ObservabilityService.
///
/// Hits: GET /api/observability/agent/{agent_id}/stats?start_time=...
pub fn project_stats(agent_id: &str, start_time: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let t = start_time.unwrap_or_else(|| {
        // default handled server-side; pass empty if not set
        ""
    });
    let path = if t.is_empty() {
        // server requires start_time — default to 24h ago
        let default = (chrono::Utc::now() - chrono::Duration::hours(24))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        format!("/observability/agent/{agent_id}/stats?start_time={}", crate::api::urlencode(&default))
    } else {
        format!("/observability/agent/{agent_id}/stats?start_time={}", crate::api::urlencode(t))
    };

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
pub fn finops_dashboard(start_time: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let path = match start_time {
        Some(t) => format!("/observability/finops/dashboard?start_time={}", crate::api::urlencode(t)),
        None => "/observability/finops/dashboard".to_string(),
    };

    let resp: FinopsDashboardResponse = client.get_json(&path)?;
    let data = resp.data;
    let s = &data.summary;

    println!("Total cost:   ${:.4}", s.total_cost);
    println!("Operations:   {} total  (avg ${:.4}/op)", s.total_operations, s.average_cost);
    println!("Agents:       {} active / {} total", s.active_agents, s.total_agents);
    println!("Tokens:       {} total  ({} prompt / {} completion)",
        data.token_usage.total_tokens,
        data.token_usage.prompt_tokens,
        data.token_usage.completion_tokens,
    );
    println!();

    if data.agents.is_empty() {
        println!("No agent data.");
        return Ok(());
    }

    println!(
        "{:<28} {:<24} {:<8} {:<14} {:<14} {:<14} COST",
        "AGENT ID", "NAME", "OPS", "PROMPT TOK", "COMPL TOK", "TOTAL TOK"
    );
    println!("{}", "-".repeat(115));

    for a in &data.agents {
        println!(
            "{:<28} {:<24} {:<8} {:<14} {:<14} {:<14} ${:.4}",
            &a.agent_id[..a.agent_id.len().min(26)],
            &a.agent_name[..a.agent_name.len().min(22)],
            a.operations,
            a.prompt_tokens,
            a.completion_tokens,
            a.total_tokens,
            a.total_cost,
        );
        if let Some(lat) = a.avg_latency_ms {
            println!("{:<28} {:<24}   avg latency: {:.0} ms", "", "", lat);
        }
    }
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