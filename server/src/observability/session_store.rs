//! Session detail from the lakehouse: `flows` (one row per turn == one trace) plus
//! `trace_usage` (tokens/cost per trace and agent), each read hot + cold.
//!
//! Pure: no IO. Tempo is optional enrichment on top (span attributes, prompt and
//! completion text) and only while it still retains the trace; ids, counts, tokens,
//! cost and latency all come from here so old and tiered sessions keep their traces.

use std::collections::HashMap;

use nasiko_event_store::flows::FlowRow;
use nasiko_event_store::tool_calls::ToolCallRow;
use nasiko_event_store::trace_usage::TraceUsageRow;
use nasiko_observability::pricing::CostBreakdown;
use nasiko_observability::types::{SessionDetails, Span, TraceDetails, TraceSummary};

/// A trace Tempo no longer has, rebuilt from its flow and `trace_usage` rows: one
/// root span for the turn plus one child span per agent that used a model.
/// Returns the trace and its cost (summed from `trace_usage`, else the flow total).
pub fn trace_from_rows(f: &FlowRow, usage: &[TraceUsageRow]) -> (TraceDetails, CostBreakdown) {
    let rows: Vec<&TraceUsageRow> = usage.iter().filter(|u| u.trace_id == f.flow_id).collect();
    let duration = f
        .duration_ms
        .or_else(|| {
            f.completed_at
                .map(|e| (e - f.created_at).num_milliseconds())
        })
        .map(|d| d.max(0) as u64);
    let mut root = root_span(f, duration);
    root.span_id = "0000000000000001".into();
    let mut spans = vec![root];
    let mut cost = CostBreakdown::default();
    for (i, u) in rows.iter().enumerate() {
        cost.prompt_usd += u.prompt_cost_usd;
        cost.completion_usd += u.completion_cost_usd;
        cost.total_usd += u.cost_usd;
        cost.estimated |= u.cost_estimated == Some(true);
        let mut attributes = HashMap::new();
        attributes.insert("agent.name".into(), u.agent_name.clone().into());
        attributes.insert("gen_ai.usage.input_tokens".into(), u.input_tokens.into());
        attributes.insert("gen_ai.usage.output_tokens".into(), u.output_tokens.into());
        attributes.insert(
            "gen_ai.usage.cache_read_tokens".into(),
            u.cache_read_tokens.into(),
        );
        attributes.insert("gen_ai.usage.cost_usd".into(), u.cost_usd.into());
        if let Some(m) = &u.model {
            attributes.insert("gen_ai.request.model".into(), m.clone().into());
        }
        let lat = u.latency_ms.map(|l| l.max(0) as u64);
        spans.push(Span {
            span_id: format!("{:016x}", i + 2),
            parent_span_id: Some("0000000000000001".into()),
            name: format!("{} llm", u.agent_name),
            started_at: u.started_at,
            ended_at: lat.map(|l| u.started_at + chrono::Duration::milliseconds(l as i64)),
            duration_ms: lat,
            service_name: u.agent_name.clone(),
            kind: 3,
            status_code: if u.error_count.unwrap_or(0) > 0 { 2 } else { 1 },
            status_message: String::new(),
            attributes,
            events: vec![],
        });
    }
    if rows.is_empty() {
        cost.total_usd = f.total_cost_usd.unwrap_or(0.0);
    }
    let trace = TraceDetails {
        trace_id: f.flow_id.clone(),
        spans,
        started_at: Some(f.created_at),
        ended_at: f.completed_at,
        duration_ms: duration,
    };
    (trace, cost)
}

/// The turn a trace the server wrote no `flows` row for, described from its `trace_usage`
/// rows. Coding-agent turns, a subagent's among them, never cross the proxy, so this is
/// the only record of them outside Tempo. `None` when no row names a session, since the
/// trace could not then be authorized.
pub fn flow_from_usage(trace_id: &str, usage: &[TraceUsageRow]) -> Option<FlowRow> {
    let rows: Vec<&TraceUsageRow> = usage.iter().filter(|u| u.trace_id == trace_id).collect();
    let first = rows.iter().min_by_key(|u| (u.started_at, &u.agent_name))?;
    let session_id = rows.iter().find_map(|u| u.session_id.clone())?;
    let latency_ms = rows.iter().filter_map(|u| u.latency_ms).max();
    let failed = rows.iter().any(|u| u.error_count.unwrap_or(0) > 0);
    Some(FlowRow {
        id: uuid::Uuid::nil(),
        flow_id: trace_id.to_string(),
        user_id: rows.iter().find_map(|u| u.user_id).unwrap_or_default(),
        session_id,
        parent_flow_id: None,
        root_agent_id: first.agent_id,
        root_agent_name: Some(first.agent_name.clone()),
        title: None,
        status: if failed { "failed" } else { "completed" }.into(),
        max_depth_reached: 0,
        total_invocations: i32::try_from(rows.len()).unwrap_or(i32::MAX),
        total_tokens_used: rows
            .iter()
            .map(|u| {
                u.input_tokens
                    .saturating_add(u.output_tokens)
                    .saturating_add(u.cache_read_tokens)
                    .saturating_add(u.cache_creation_tokens)
            })
            .fold(0i64, i64::saturating_add),
        total_cost_usd: Some(rows.iter().map(|u| u.cost_usd).sum()),
        duration_ms: latency_ms,
        error_message: None,
        metadata: "{}".into(),
        created_at: first.started_at,
        completed_at: latency_ms.map(|l| first.started_at + chrono::Duration::milliseconds(l)),
    })
}

/// `flows` plus a turn per trace of `usage` they do not cover (see [`flow_from_usage`]),
/// so a session whose turns never crossed the proxy still lists them.
pub fn flows_with_usage_turns(flows: &[FlowRow], usage: &[TraceUsageRow]) -> Vec<FlowRow> {
    let mut all = flows.to_vec();
    let mut seen: std::collections::HashSet<&str> =
        flows.iter().map(|f| f.flow_id.as_str()).collect();
    for u in usage {
        if seen.insert(u.trace_id.as_str()) {
            all.extend(flow_from_usage(&u.trace_id, usage));
        }
    }
    all
}

/// Adds one child span of the root per stored tool call of this trace, so a trace rebuilt
/// without Tempo still lists its tool calls. Calls of other traces are ignored.
pub fn add_tool_call_spans(trace: &mut TraceDetails, calls: &[ToolCallRow]) {
    let mine: Vec<&ToolCallRow> = calls
        .iter()
        .filter(|c| c.trace_id == trace.trace_id)
        .collect();
    for (i, c) in mine.iter().enumerate() {
        let mut attributes: HashMap<String, serde_json::Value> = HashMap::new();
        attributes.insert("tool.name".into(), c.tool_name.clone().into());
        attributes.insert("tool.status".into(), c.status.clone().into());
        if let Some(e) = &c.error {
            attributes.insert("tool.error".into(), e.clone().into());
        }
        let dur: Option<u64> = c.duration_ms.map(|d| d.max(0) as u64);
        trace.spans.push(Span {
            span_id: format!("{:016x}", 0x1000 + i),
            parent_span_id: Some("0000000000000001".into()),
            name: format!("execute_tool {}", c.tool_name),
            started_at: c.started_at,
            ended_at: dur.map(|d| c.started_at + chrono::Duration::milliseconds(d as i64)),
            duration_ms: dur,
            service_name: c.agent_name.clone(),
            kind: 1,
            status_code: if c.status == "error" || c.error.is_some() {
                2
            } else {
                1
            },
            status_message: c.error.clone().unwrap_or_default(),
            attributes,
            events: vec![],
        });
    }
}

/// Builds the session's trace list, newest turn first (the order Tempo returned).
pub fn session_from_rows(
    session_id: &str,
    flows: &[FlowRow],
    usage: &[TraceUsageRow],
) -> SessionDetails {
    let mut by_trace: HashMap<&str, Vec<&TraceUsageRow>> = HashMap::new();
    for u in usage {
        by_trace.entry(u.trace_id.as_str()).or_default().push(u);
    }

    let mut flows: Vec<&FlowRow> = flows.iter().collect();
    flows.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then(a.flow_id.cmp(&b.flow_id))
    });

    let mut traces = Vec::with_capacity(flows.len());
    let mut total = CostBreakdown::default();
    let (mut tin, mut tout, mut cr, mut cc) = (0u64, 0u64, 0u64, 0u64);
    let mut durations: Vec<f64> = Vec::new();
    let mut model_used: Option<String> = None;

    for f in flows {
        let rows = by_trace
            .get(f.flow_id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let n = |g: fn(&TraceUsageRow) -> i64| rows.iter().map(|r| g(r).max(0) as u64).sum::<u64>();
        let (i, o, r, c) = (
            n(|r| r.input_tokens),
            n(|r| r.output_tokens),
            n(|r| r.cache_read_tokens),
            n(|r| r.cache_creation_tokens),
        );
        let mut cost = CostBreakdown {
            prompt_usd: rows.iter().map(|r| r.prompt_cost_usd).sum(),
            completion_usd: rows.iter().map(|r| r.completion_cost_usd).sum(),
            total_usd: rows.iter().map(|r| r.cost_usd).sum(),
            estimated: rows.iter().any(|r| r.cost_estimated == Some(true)),
            ..Default::default()
        };
        // A turn with no usage rows yet (unpriced, or never called a model) still
        // has the flow's own running total.
        if rows.is_empty() {
            cost.total_usd = f.total_cost_usd.unwrap_or(0.0);
        }
        // Not `add_assign`: it rebuilds `total_usd` from the parts, which drops the
        // flow-total fallback and any cost `trace_usage` never split per component.
        total.prompt_usd += cost.prompt_usd;
        total.completion_usd += cost.completion_usd;
        total.total_usd += cost.total_usd;
        total.estimated |= cost.estimated;
        tin += i;
        tout += o;
        cr += r;
        cc += c;

        let model = rows.iter().find_map(|r| r.model.clone());
        if model_used.is_none() {
            model_used = model.clone();
        }
        let duration = f
            .duration_ms
            .or_else(|| {
                f.completed_at
                    .map(|e| (e - f.created_at).num_milliseconds())
            })
            .or_else(|| rows.iter().filter_map(|r| r.latency_ms).max())
            .map(|d| d.max(0) as u64);
        if let Some(d) = duration {
            durations.push(d as f64);
        }

        traces.push(TraceSummary {
            trace_id: f.flow_id.clone(),
            root_span: root_span(f, duration),
            input_tokens: i,
            output_tokens: o,
            cache_read_tokens: r,
            cache_creation_tokens: c,
            model_used: model,
            duration_ms: duration,
            cost,
            input_content: f.title.clone(),
            output_content: None,
        });
    }

    let (p50, p99, avg) = latency_stats(&mut durations);
    SessionDetails {
        session_id: session_id.to_string(),
        trace_count: traces.len(),
        traces,
        input_tokens: tin,
        output_tokens: tout,
        cache_read_tokens: cr,
        cache_creation_tokens: cc,
        model_used,
        latency_ms_p50: p50,
        latency_ms_p99: p99,
        latency_ms_avg: avg,
        has_more_traces: false,
        metrics_complete: true,
        cost: total,
    }
}

/// Overlays what only Tempo knows (root span attributes and events, prompt and
/// completion text) onto traces it still retains. Counts and cost stay from SQL.
pub fn enrich_from_tempo(details: &mut SessionDetails, tempo: &SessionDetails) {
    let by_id: HashMap<&str, &TraceSummary> = tempo
        .traces
        .iter()
        .map(|t| (t.trace_id.as_str(), t))
        .collect();
    for t in &mut details.traces {
        let Some(live) = by_id.get(t.trace_id.as_str()) else {
            continue;
        };
        t.root_span = live.root_span.clone();
        if live.input_content.is_some() {
            t.input_content = live.input_content.clone();
        }
        if live.output_content.is_some() {
            t.output_content = live.output_content.clone();
        }
        if t.model_used.is_none() {
            t.model_used = live.model_used.clone();
        }
    }
}

/// Span detail for a span of a trace rebuilt by [`trace_from_rows`]: tokens and cost
/// come from the attributes that function wrote; no prompt/completion text exists.
pub fn span_details_from_sql(span: Span) -> nasiko_observability::types::SpanDetails {
    let get = |k: &str| {
        span.attributes
            .get(k)
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .max(0) as u64
    };
    let (i, o, r) = (
        get("gen_ai.usage.input_tokens"),
        get("gen_ai.usage.output_tokens"),
        get("gen_ai.usage.cache_read_tokens"),
    );
    let total_usd = span
        .attributes
        .get("gen_ai.usage.cost_usd")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    nasiko_observability::types::SpanDetails {
        span,
        input_content: None,
        output_content: None,
        token_usage: nasiko_observability::types::TokenUsage {
            input_tokens: i,
            output_tokens: o,
            cache_read_tokens: r,
            cache_creation_tokens: 0,
            total_tokens: i + o + r,
        },
        cost: CostBreakdown {
            total_usd,
            ..Default::default()
        },
    }
}

fn root_span(f: &FlowRow, duration: Option<u64>) -> Span {
    let mut attributes = HashMap::new();
    attributes.insert("session.id".into(), f.session_id.clone().into());
    attributes.insert("flow.status".into(), f.status.clone().into());
    if let Some(a) = &f.root_agent_name {
        attributes.insert("agent.name".into(), a.clone().into());
    }
    Span {
        span_id: String::new(),
        parent_span_id: None,
        name: f.title.clone().unwrap_or_else(|| "turn".into()),
        started_at: f.created_at,
        ended_at: f.completed_at,
        duration_ms: duration,
        service_name: f.root_agent_name.clone().unwrap_or_default(),
        kind: 2,
        status_code: if f.status == "failed" { 2 } else { 1 },
        status_message: f.error_message.clone().unwrap_or_default(),
        attributes,
        events: vec![],
    }
}

fn latency_stats(d: &mut [f64]) -> (Option<f64>, Option<f64>, Option<f64>) {
    if d.is_empty() {
        return (None, None, None);
    }
    d.sort_by(|a, b| a.total_cmp(b));
    let pick = |q: f64| d[((d.len() - 1) as f64 * q).round() as usize];
    let avg = d.iter().sum::<f64>() / d.len() as f64;
    (Some(pick(0.5)), Some(pick(0.99)), Some(avg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    fn flow(id: &str, secs: i64, status: &str) -> FlowRow {
        FlowRow {
            id: Uuid::new_v4(),
            flow_id: id.into(),
            user_id: Uuid::nil(),
            session_id: "s".into(),
            parent_flow_id: None,
            root_agent_id: None,
            root_agent_name: Some("echo".into()),
            title: Some(format!("q {id}")),
            status: status.into(),
            max_depth_reached: 0,
            total_invocations: 1,
            total_tokens_used: 0,
            total_cost_usd: Some(0.25),
            duration_ms: Some(secs * 100),
            error_message: None,
            metadata: "{}".into(),
            created_at: Utc.timestamp_opt(secs, 0).unwrap(),
            completed_at: None,
        }
    }

    fn usage(trace: &str, agent: &str, tin: i64, cost: f64) -> TraceUsageRow {
        TraceUsageRow {
            trace_id: trace.into(),
            agent_name: agent.into(),
            session_id: Some("s".into()),
            agent_id: None,
            user_id: None,
            model: Some("m".into()),
            provider: None,
            input_tokens: tin,
            output_tokens: 1,
            cache_read_tokens: 2,
            cache_creation_tokens: 0,
            cost_usd: cost,
            prompt_cost_usd: cost / 2.0,
            completion_cost_usd: cost / 2.0,
            latency_ms: None,
            tool_call_count: 0,
            error_count: None,
            cost_estimated: None,
            started_at: Utc.timestamp_opt(0, 0).unwrap(),
            materialized_at: Utc.timestamp_opt(0, 0).unwrap(),
            source: "agent".into(),
            metadata: "{}".into(),
        }
    }

    #[test]
    fn one_trace_per_flow_newest_first_with_usage_summed_per_trace() {
        let flows = [flow("a", 1, "completed"), flow("b", 2, "failed")];
        let u = [
            usage("a", "x", 10, 1.0),
            usage("a", "y", 5, 0.5),
            usage("b", "x", 3, 0.1),
        ];
        let d = session_from_rows("s", &flows, &u);
        let ids: Vec<_> = d.traces.iter().map(|t| t.trace_id.as_str()).collect();
        assert_eq!(ids, ["b", "a"]);
        assert_eq!(d.trace_count, 2);
        assert_eq!(d.traces[1].input_tokens, 15);
        assert!((d.traces[1].cost.total_usd - 1.5).abs() < 1e-9);
        assert_eq!(d.input_tokens, 18);
        assert!((d.cost.total_usd - 1.6).abs() < 1e-9);
        assert_eq!(d.traces[0].root_span.status_code, 2);
        assert_eq!(d.traces[1].input_content.as_deref(), Some("q a"));
    }

    #[test]
    fn trace_rebuilt_from_rows_has_root_and_agent_spans() {
        let u = [
            usage("a", "x", 10, 1.0),
            usage("a", "y", 5, 0.5),
            usage("b", "x", 3, 0.1),
        ];
        let (t, cost) = trace_from_rows(&flow("a", 1, "completed"), &u);
        assert_eq!(t.spans.len(), 3);
        assert!(
            t.spans[1..]
                .iter()
                .all(|s| s.parent_span_id.as_deref() == Some("0000000000000001"))
        );
        assert!((cost.total_usd - 1.5).abs() < 1e-9);
        let (t, cost) = trace_from_rows(&flow("c", 1, "completed"), &u);
        assert_eq!(t.spans.len(), 1);
        assert!((cost.total_usd - 0.25).abs() < 1e-9);
    }

    #[test]
    fn a_trace_without_a_flow_is_a_turn_of_its_usage_rows() {
        let mut slow = usage("t", "x", 10, 1.0);
        slow.latency_ms = Some(400);
        slow.error_count = Some(1);
        let mut later = usage("t", "y", 5, 0.5);
        later.started_at = Utc.timestamp_opt(5, 0).unwrap();
        let f = flow_from_usage("t", &[later, slow, usage("other", "x", 1, 9.0)]).unwrap();
        assert_eq!((f.flow_id.as_str(), f.session_id.as_str()), ("t", "s"));
        assert_eq!(f.root_agent_name.as_deref(), Some("x"), "the earliest row");
        assert_eq!(f.created_at, Utc.timestamp_opt(0, 0).unwrap());
        assert_eq!(f.duration_ms, Some(400));
        assert_eq!(f.status, "failed");
        assert!((f.total_cost_usd.unwrap() - 1.5).abs() < 1e-9);
        assert_eq!(f.total_tokens_used, 13 + 8);
    }

    #[test]
    fn usage_without_a_session_is_no_turn() {
        let mut orphan = usage("t", "x", 1, 0.1);
        orphan.session_id = None;
        assert_eq!(flow_from_usage("t", &[orphan]), None);
        assert_eq!(flow_from_usage("missing", &[usage("t", "x", 1, 0.1)]), None);
    }

    #[test]
    fn a_session_with_no_flows_lists_one_turn_per_usage_trace() {
        let flows = [flow("a", 1, "completed")];
        let u = [
            usage("a", "x", 10, 1.0),
            usage("b", "x", 3, 0.1),
            usage("b", "y", 2, 0.2),
        ];
        let turns = flows_with_usage_turns(&flows, &u);
        let ids: Vec<_> = turns.iter().map(|f| f.flow_id.as_str()).collect();
        assert_eq!(ids, ["a", "b"], "a flow is never duplicated");
        assert_eq!(turns[0], flows[0]);
        let d = session_from_rows("s", &flows_with_usage_turns(&[], &u), &u);
        assert_eq!(d.trace_count, 2);
        assert_eq!(d.input_tokens, 15);
    }

    #[test]
    fn unpriced_turn_falls_back_to_flow_total() {
        let d = session_from_rows("s", &[flow("a", 1, "completed")], &[]);
        assert_eq!(d.traces.len(), 1);
        assert!((d.cost.total_usd - 0.25).abs() < 1e-9);
        assert_eq!(d.latency_ms_avg, Some(100.0));
    }

    #[test]
    fn tempo_enriches_only_traces_it_retains() {
        let mut d = session_from_rows(
            "s",
            &[flow("a", 1, "completed"), flow("b", 2, "completed")],
            &[],
        );
        let mut live = d.clone();
        live.traces.retain(|t| t.trace_id == "a");
        live.traces[0].output_content = Some("answer".into());
        live.traces[0].input_tokens = 999;
        enrich_from_tempo(&mut d, &live);
        let a = d.traces.iter().find(|t| t.trace_id == "a").unwrap();
        assert_eq!(a.output_content.as_deref(), Some("answer"));
        assert_eq!(a.input_tokens, 0, "counts stay from SQL");
        assert!(
            d.traces
                .iter()
                .find(|t| t.trace_id == "b")
                .unwrap()
                .output_content
                .is_none()
        );
    }
}
