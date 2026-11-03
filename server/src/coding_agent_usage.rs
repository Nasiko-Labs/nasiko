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
        // The read-back counts only `call_tool` spans; the exporter emits a
        // coding-agent tool as `execute_tool`, so it has always written 0 here.
        tool_call_count: 0,
        cost_estimated: cost.estimated,
        cost_usd: cost.total_usd,
        prompt_cost_usd: cost.prompt_usd,
        completion_cost_usd: cost.completion_usd,
        latency_ms: Some((ended_at - started_at).num_milliseconds().max(0)),
        started_at,
        error_count: u32::try_from(unrecovered_tool_errors(event)).unwrap_or(u32::MAX),
    })
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
