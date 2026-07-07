use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

use crate::api::Client;

// ─── Response types (ObservabilityService) ───────────────────────────────────

#[derive(Deserialize)]
struct SessionSummary {
    session_id: String,
    agent_id: String,
    #[serde(default)]
    num_traces: Option<u32>,
    start_time: Option<String>,
    duration_ms: Option<u64>,
    #[serde(default)]
    trace_latency_ms_p50: Option<f64>,
    #[serde(default)]
    trace_latency_ms_p99: Option<f64>,
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
    #[serde(default)]
    operations_last_24h: usize,
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
    #[serde(default)]
    avg_cost_per_operation: f64,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    avg_latency_ms: Option<f64>,
    #[serde(default)]
    version: Option<String>,
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
        "{:<34} {:<22} {:<20} {:<8} {:<6} {:<10} {:<8} {:<8} COST",
        "SESSION ID", "AGENT", "STARTED", "DUR(ms)", "SPANS", "TOKENS", "p50(ms)", "p99(ms)"
    );
    println!("{}", "-".repeat(125));

    for s in &data.sessions {
        let started = s.start_time.as_deref().unwrap_or("-");
        let started_short = started.get(..19).unwrap_or(started);
        let dur = s.duration_ms.map(|d| d.to_string()).unwrap_or_else(|| "-".into());
        let spans = s.num_traces.map(|n| n.to_string()).unwrap_or_else(|| "-".into());
        let tokens = s.token_usage.total.map(|t| t.to_string()).unwrap_or_else(|| "-".into());
        let p50 = s.trace_latency_ms_p50.map(|p| format!("{p:.0}")).unwrap_or_else(|| "-".into());
        let p99 = s.trace_latency_ms_p99.map(|p| format!("{p:.0}")).unwrap_or_else(|| "-".into());
        let cost = s.cost_summary.total.cost.map(|c| format!("${c:.4}")).unwrap_or_else(|| "-".into());
        println!(
            "{:<34} {:<22} {:<20} {:<8} {:<6} {:<10} {:<8} {:<8} {}",
            &s.session_id[..s.session_id.len().min(32)],
            &s.agent_id[..s.agent_id.len().min(20)],
            started_short,
            dur,
            spans,
            tokens,
            p50,
            p99,
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
/// Hits: GET /api/observability/trace/{trace_id}
pub fn trace_detail(trace_id: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: TraceDetailResponse =
        client.get_json(&format!("/observability/trace/{trace_id}"))?;
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

    println!("{:<5} {:<18} {:<10} {:<12} {:<10} {:<20} NAME", "CODE", "SPAN ID", "KIND", "LAT(ms)", "TOKENS", "STARTED");
    println!("{}", "-".repeat(100));

    // The server returns a nested tree: t.spans contains only root nodes,
    // children are embedded in SpanNode.children (not repeated in the top-level slice).
    fn print_span_node(span: &SpanNode, depth: usize) {
        let indent = "  ".repeat(depth);
        let lat = span.latency_ms.map(|l| format!("{l:.0}")).unwrap_or_else(|| "-".into());
        let started = span.start_time.as_deref()
            .and_then(|t| t.get(..19))
            .unwrap_or("-");
        println!(
            "{:<5} {:<18} {:<10} {:<12} {:<10} {:<20} {}{}",
            &span.status_code[..span.status_code.len().min(4)],
            &span.span_id[..span.span_id.len().min(16)],
            &span.span_kind[..span.span_kind.len().min(8)],
            lat,
            span.token_count_total,
            started,
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

    println!(
        "{:<26} {:<22} {:<6} {:<12} {:<12} {:<12} {:<10} {:<10} COST/OP",
        "AGENT ID", "NAME", "OPS", "PROMPT TOK", "COMPL TOK", "TOTAL TOK", "AVG LAT", "TOTAL $"
    );
    println!("{}", "-".repeat(128));

    for a in &data.agents {
        let lat = a.avg_latency_ms.map(|l| format!("{l:.0}ms")).unwrap_or_else(|| "-".into());
        let ver = a.version.as_deref().unwrap_or("");
        let name_ver = if ver.is_empty() {
            a.agent_name.clone()
        } else {
            format!("{} ({})", a.agent_name, ver)
        };
        println!(
            "{:<26} {:<22} {:<6} {:<12} {:<12} {:<12} {:<10} {:<10} ${:.4}",
            &a.agent_id[..a.agent_id.len().min(24)],
            &name_ver[..name_ver.len().min(20)],
            a.operations,
            a.prompt_tokens,
            a.completion_tokens,
            a.total_tokens,
            lat,
            format!("${:.4}", a.total_cost),
            a.avg_cost_per_operation,
        );
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