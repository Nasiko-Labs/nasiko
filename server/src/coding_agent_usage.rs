//! A coding-agent turn's `trace_usage` row, computed at ingest.
//!
//! Monitor, Overview and the coding-harness pages read spend from `trace_usage`.
//! It used to reach that table only by a round trip: export the turn to Tempo,
//! then read it back (`receipt_materializer`), eight turns every two minutes —
//! hours of lag after a history import, and never for a turn older than Tempo's
//! ingestion window. The receipt already carries everything the row holds, so
//! ingest writes it in the receipt's own transaction instead.
//!
//! The row is the one the read-back would have produced from the exported trace
//! (`TempoLokiProvider::extract_trace_usage` over `coding_agent_otlp::trace_payload`):
//! same key, same pricing calls, same model/provider collapsing, same error
//! rule. That keeps the two paths interchangeable — a Tempo window scan that
//! later reaches the same trace rewrites the row with the values it already has
//! instead of adding a second one.

use chrono::{DateTime, Utc};
use nasiko_observability::types::{Span, SpanUsage, unrecovered_error_spans};
use nasiko_observability::{CostBreakdown, CostRequest, TraceUsageRow, compute_cost};
use nasiko_types::{CodingAgentEventV1, CodingAgentLlmCall};

use crate::coding_agent_otlp::{tool_span_status_code, tool_span_window, trace_id_for_event};

/// The usage row for a turn, or `None` when no LLM call carried a token — the
/// read-back writes nothing for such a turn either.
///
/// `event` is the turn as it is exported: scoped to the server's session id and
/// relabeled with the router's provider/model (`coding_agent_otlp::relabel_from_router`).
pub(crate) async fn turn_usage_row(
    pricing: &nasiko_pricing::PricingEngine,
    event: &CodingAgentEventV1,
) -> Option<TraceUsageRow> {
    // The read-back skips a span with no tokens before it looks at its model,
    // provider or cost, so a zero-token call must not influence any of them.
    let calls: Vec<&CodingAgentLlmCall> = event
        .turn
        .llm_calls
        .iter()
        .filter(|call| has_tokens(call))
        .collect();
    let first = calls.first()?;

    let mut cost = CostBreakdown::default();
    for call in &calls {
        let usage = span_usage(call);
        cost.add_assign(
            compute_cost(
                pricing,
                CostRequest::from_usage(
                    provider_label(call),
                    Some(&call.model),
                    call.started_at,
                    &usage,
                ),
            )
            .await,
        );
    }
    let sum = |tokens: fn(&CodingAgentLlmCall) -> u64| {
        calls
            .iter()
            .fold(0_u64, |total, call| total.saturating_add(tokens(call)))
    };
    let (started_at, ended_at) = trace_window(event);
    Some(TraceUsageRow {
        trace_id: trace_id_for_event(event),
        agent_name: event.source.agent_name.clone(),
        // A local coding agent's row is named per user; the name alone resolves it.
        agent_uuid: None,
        session_id: Some(event.session.id.clone()),
        model: calls
            .iter()
            .all(|call| call.model == first.model)
            .then(|| first.model.clone()),
        provider: calls
            .iter()
            .all(|call| provider_label(call) == provider_label(first))
            .then(|| provider_label(first).map(str::to_owned))
            .flatten(),
        input_tokens: sum(|call| call.input_tokens),
        output_tokens: sum(|call| call.output_tokens),
        cache_read_tokens: sum(|call| call.cache_read_tokens),
        cache_creation_tokens: sum(|call| call.cache_creation_tokens),
        // The exporter emits one `execute_tool` span per tool call, and the
        // read-back counts each of them.
        tool_call_count: u32::try_from(event.turn.tool_calls.len()).unwrap_or(u32::MAX),
        cost_estimated: cost.estimated,
        cost_usd: cost.total_usd,
        prompt_cost_usd: cost.prompt_usd,
        completion_cost_usd: cost.completion_usd,
        latency_ms: Some((ended_at - started_at).num_milliseconds().max(0)),
        started_at,
        error_count: u32::try_from(unrecovered_tool_errors(event)).unwrap_or(u32::MAX),
        // Coding-agent tool calls are written by ingest (`write_tool_calls`), with full bodies.
        tool_calls: Vec::new(),
    })
}

/// What the router has already booked, live, for a coding agent's calls inside
/// one turn's window: the `router_*` share (0080) of its other rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, sqlx::FromRow)]
pub(crate) struct RoutedShare {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cost_usd: f64,
    pub prompt_cost_usd: f64,
    pub completion_cost_usd: f64,
}

/// The router's share of `agent_id`'s spend booked on rows other than `row`
/// within `row`'s window, or `None` when the agent is not a coding agent.
///
/// A local coding agent that both routes through us and reports turns has each
/// call booked twice: live by the router (`direct:<id>` rows, or its session
/// flow's row) and again in the turn's receipt estimate. The router's figure is
/// the billed one, so the turn row keeps only what the router did not see.
pub(crate) async fn routed_share_within(
    conn: &mut sqlx::PgConnection,
    agent_id: uuid::Uuid,
    row: &TraceUsageRow,
) -> Result<Option<RoutedShare>, sqlx::Error> {
    let tolerance = crate::coding_agent_otlp::ROUTER_MATCH_TOLERANCE;
    let ended_at = row.started_at + chrono::Duration::milliseconds(row.latency_ms.unwrap_or(0));
    sqlx::query_as(
        r#"SELECT COALESCE(SUM(o.router_input_tokens), 0)::bigint          AS input_tokens,
                  COALESCE(SUM(o.router_output_tokens), 0)::bigint         AS output_tokens,
                  COALESCE(SUM(o.router_cache_read_tokens), 0)::bigint     AS cache_read_tokens,
                  COALESCE(SUM(o.router_cache_creation_tokens), 0)::bigint AS cache_creation_tokens,
                  COALESCE(SUM(o.router_cost_usd), 0)::float8              AS cost_usd,
                  COALESCE(SUM(o.router_prompt_cost_usd), 0)::float8       AS prompt_cost_usd,
                  COALESCE(SUM(o.router_completion_cost_usd), 0)::float8   AS completion_cost_usd
             FROM agents a
             LEFT JOIN trace_usage o
                    ON o.agent_id = a.id
                   AND o.trace_id <> $2
                   AND o.priced_by = 'router'
                   AND o.started_at BETWEEN $3 AND $4
            WHERE a.id = $1 AND a.coding_agent_integration_id IS NOT NULL
            GROUP BY a.id"#,
    )
    .bind(agent_id)
    .bind(&row.trace_id)
    .bind(row.started_at - tolerance)
    .bind(ended_at + tolerance)
    .fetch_optional(&mut *conn)
    .await
}

/// `row` less what the router already booked, never below zero. With nothing
/// routed (a reporting-only install) the receipt's estimate stands as is.
pub(crate) fn net_of_routed(row: &TraceUsageRow, routed: &RoutedShare) -> TraceUsageRow {
    let less =
        |reported: u64, booked: i64| reported.saturating_sub(u64::try_from(booked).unwrap_or(0));
    let less_usd = |reported: f64, booked: f64| (reported - booked).max(0.0);
    TraceUsageRow {
        input_tokens: less(row.input_tokens, routed.input_tokens),
        output_tokens: less(row.output_tokens, routed.output_tokens),
        cache_read_tokens: less(row.cache_read_tokens, routed.cache_read_tokens),
        cache_creation_tokens: less(row.cache_creation_tokens, routed.cache_creation_tokens),
        cost_usd: less_usd(row.cost_usd, routed.cost_usd),
        prompt_cost_usd: less_usd(row.prompt_cost_usd, routed.prompt_cost_usd),
        completion_cost_usd: less_usd(row.completion_cost_usd, routed.completion_cost_usd),
        ..row.clone()
    }
}

fn has_tokens(call: &CodingAgentLlmCall) -> bool {
    call.input_tokens != 0
        || call.output_tokens != 0
        || call.cache_read_tokens != 0
        || call.cache_creation_tokens != 0
}

/// `span_provider`'s reading of the exported `gen_ai.provider.name`.
fn provider_label(call: &CodingAgentLlmCall) -> Option<&str> {
    Some(call.provider.as_str()).filter(|label| !label.is_empty())
}

/// What `extract_usage_attrs` reads back off the exported call span. The span
/// declares the exclusive prompt convention, so the counts pass through as-is.
fn span_usage(call: &CodingAgentLlmCall) -> SpanUsage {
    let accounting = call.accounting.as_ref();
    SpanUsage {
        input: call.input_tokens,
        output: call.output_tokens,
        cache_read: call.cache_read_tokens,
        cache_creation: call.cache_creation_tokens,
        cache_creation_5m: accounting.and_then(|a| a.cache_creation_5m_tokens),
        cache_creation_1h: accounting.and_then(|a| a.cache_creation_1h_tokens),
        speed: accounting.and_then(|a| a.speed.clone()),
        service_tier: accounting.and_then(|a| a.service_tier.clone()),
        inference_geo: accounting.and_then(|a| a.inference_geo.clone()),
        conflicting_observations: accounting.is_some_and(|a| a.conflicting_observations),
        model: Some(call.model.clone()),
    }
}

/// Earliest start and latest end over every exported span — the trace's own
/// bounds, which the read-back stores as `started_at` and `latency_ms`.
fn trace_window(event: &CodingAgentEventV1) -> (DateTime<Utc>, DateTime<Utc>) {
    let turn = &event.turn;
    let windows = std::iter::once((turn.started_at, turn.ended_at))
        .chain(turn.llm_calls.iter().map(|c| (c.started_at, c.ended_at)))
        .chain(turn.tool_calls.iter().map(|t| tool_span_window(event, t)));
    windows.fold((turn.started_at, turn.ended_at), |(start, end), (s, e)| {
        (start.min(s), end.max(e))
    })
}

/// Tool spans that failed with no later successful retry of the same tool —
/// the read-back's SE-3 rule (`unrecovered_error_spans`), over the same spans.
fn unrecovered_tool_errors(event: &CodingAgentEventV1) -> usize {
    let spans: Vec<Span> = event
        .turn
        .tool_calls
        .iter()
        .enumerate()
        .map(|(index, tool)| {
            let (started_at, ended_at) = tool_span_window(event, tool);
            Span {
                span_id: index.to_string(),
                // Every tool span is a child of the turn's root span.
                parent_span_id: Some("root".into()),
                name: format!("execute_tool {}", tool.name),
                started_at,
                ended_at: Some(ended_at),
                duration_ms: Some((ended_at - started_at).num_milliseconds().max(0) as u64),
                service_name: event.source.agent_name.clone(),
                kind: 1,
                status_code: tool_span_status_code(tool.status),
                status_message: String::new(),
                attributes: Default::default(),
                events: Vec::new(),
            }
        })
        .collect();
    let refs: Vec<&Span> = spans.iter().collect();
    unrecovered_error_spans(&refs).len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use nasiko_types::{
        CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentSession, CodingAgentSource,
        CodingAgentTimestampQuality, CodingAgentToolAssociation, CodingAgentToolCall,
        CodingAgentToolCallStatus, CodingAgentTurn,
    };

    fn usage_row(tokens: u64, cost: f64) -> TraceUsageRow {
        TraceUsageRow {
            trace_id: "t".into(),
            agent_name: "claude".into(),
            agent_uuid: None,
            session_id: None,
            model: None,
            provider: None,
            input_tokens: tokens,
            output_tokens: tokens,
            cache_read_tokens: tokens,
            cache_creation_tokens: tokens,
            tool_call_count: 0,
            cost_estimated: false,
            cost_usd: cost,
            prompt_cost_usd: cost / 2.0,
            completion_cost_usd: cost / 2.0,
            latency_ms: Some(1_000),
            started_at: at(0),
            error_count: 0,
            tool_calls: Vec::new(),
        }
    }

    #[test]
    fn reporting_only_turn_keeps_its_estimate() {
        let net = net_of_routed(&usage_row(100, 2.0), &RoutedShare::default());
        assert_eq!(
            (net.input_tokens, net.output_tokens, net.cache_read_tokens),
            (100, 100, 100)
        );
        assert_eq!(
            (net.cost_usd, net.prompt_cost_usd, net.completion_cost_usd),
            (2.0, 1.0, 1.0)
        );
    }

    #[test]
    fn routed_turn_drops_what_the_router_booked() {
        let routed = RoutedShare {
            input_tokens: 100,
            output_tokens: 100,
            cache_read_tokens: 100,
            cache_creation_tokens: 100,
            cost_usd: 2.0,
            prompt_cost_usd: 1.0,
            completion_cost_usd: 1.0,
        };
        let net = net_of_routed(&usage_row(100, 2.0), &routed);
        assert_eq!((net.input_tokens, net.output_tokens), (0, 0));
        assert_eq!((net.cache_read_tokens, net.cache_creation_tokens), (0, 0));
        assert_eq!(net.cost_usd, 0.0);
    }

    #[test]
    fn partly_routed_turn_keeps_the_rest_and_never_goes_negative() {
        let routed = RoutedShare {
            input_tokens: 40,
            output_tokens: 500,
            cost_usd: 0.5,
            prompt_cost_usd: 3.0,
            ..RoutedShare::default()
        };
        let net = net_of_routed(&usage_row(100, 2.0), &routed);
        assert_eq!(
            (net.input_tokens, net.output_tokens, net.cache_read_tokens),
            (60, 0, 100)
        );
        assert_eq!(
            (net.cost_usd, net.prompt_cost_usd, net.completion_cost_usd),
            (1.5, 0.0, 1.0)
        );
    }

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn call(id: &str, model: &str, tokens: u64) -> CodingAgentLlmCall {
        CodingAgentLlmCall {
            id: id.into(),
            model: model.into(),
            provider: "anthropic".into(),
            input_tokens: tokens,
            output_tokens: tokens,
            cache_read_tokens: tokens,
            cache_creation_tokens: tokens,
            accounting: None,
            started_at: at(1),
            ended_at: at(2),
        }
    }

    fn tool(name: &str, status: CodingAgentToolCallStatus, start: i64) -> CodingAgentToolCall {
        CodingAgentToolCall {
            id: format!("{name}-{start}"),
            name: name.into(),
            kind: "shell".into(),
            model_call_id: None,
            status,
            arguments: None,
            output: None,
            raw: None,
            error: None,
            started_at: Some(at(start)),
            ended_at: Some(at(start + 1)),
            duration_ms: None,
            association: CodingAgentToolAssociation::Turn,
            timestamp_quality: CodingAgentTimestampQuality::Exact,
        }
    }

    fn event(
        llm_calls: Vec<CodingAgentLlmCall>,
        tool_calls: Vec<CodingAgentToolCall>,
    ) -> CodingAgentEventV1 {
        CodingAgentEventV1 {
            version: CODING_AGENT_EVENT_VERSION,
            event_id: "event".into(),
            captured_at: at(10),
            source: CodingAgentSource {
                agent_id: "claude".into(),
                agent_name: "coding-agent".into(),
            },
            session: CodingAgentSession {
                id: "server-session".into(),
                source_id: "source".into(),
                title: None,
                parent: None,
            },
            turn: CodingAgentTurn {
                id: "turn".into(),
                prompt: None,
                response: None,
                started_at: at(0),
                ended_at: at(5),
                llm_calls,
                tool_calls,
            },
            capture_policy: CapturePolicy::MetadataOnly,
        }
    }

    #[tokio::test]
    async fn sums_token_calls_and_keeps_the_trace_key_and_window() {
        let pricing = nasiko_pricing::PricingEngine::offline();
        let source = event(
            vec![call("a", "claude-x", 10), call("b", "claude-x", 5)],
            vec![tool("bash", CodingAgentToolCallStatus::Succeeded, 8)],
        );
        let row = turn_usage_row(&pricing, &source).await.unwrap();
        assert_eq!(row.trace_id, trace_id_for_event(&source));
        assert_eq!(row.agent_name, "coding-agent");
        assert_eq!(row.session_id.as_deref(), Some("server-session"));
        assert_eq!(
            (
                row.input_tokens,
                row.output_tokens,
                row.cache_read_tokens,
                row.cache_creation_tokens
            ),
            (15, 15, 15, 15)
        );
        assert_eq!(row.model.as_deref(), Some("claude-x"));
        assert_eq!(row.provider.as_deref(), Some("anthropic"));
        assert_eq!(row.started_at, at(0));
        // The tool span ends at 9s, after the turn's own end.
        assert_eq!(row.latency_ms, Some(9_000));
        assert_eq!(row.error_count, 0);
    }

    #[tokio::test]
    async fn mixed_models_collapse_and_zero_token_calls_are_ignored() {
        let pricing = nasiko_pricing::PricingEngine::offline();
        let mixed = event(
            vec![call("a", "claude-x", 1), call("b", "claude-y", 1)],
            vec![],
        );
        assert_eq!(turn_usage_row(&pricing, &mixed).await.unwrap().model, None);
        let zero_ignored = event(
            vec![call("a", "claude-x", 1), call("b", "claude-y", 0)],
            vec![],
        );
        assert_eq!(
            turn_usage_row(&pricing, &zero_ignored)
                .await
                .unwrap()
                .model
                .as_deref(),
            Some("claude-x")
        );
        let tokenless = event(vec![call("a", "claude-x", 0)], vec![]);
        assert!(turn_usage_row(&pricing, &tokenless).await.is_none());
    }

    #[tokio::test]
    async fn counts_only_tool_failures_no_later_retry_recovered() {
        use CodingAgentToolCallStatus::*;
        let pricing = nasiko_pricing::PricingEngine::offline();
        let source = event(
            vec![call("a", "claude-x", 1)],
            vec![
                tool("bash", Failed, 0),
                tool("bash", Succeeded, 2),
                tool("read", TimedOut, 0),
                tool("edit", Denied, 3),
            ],
        );
        assert_eq!(
            turn_usage_row(&pricing, &source).await.unwrap().error_count,
            2
        );
    }
}
