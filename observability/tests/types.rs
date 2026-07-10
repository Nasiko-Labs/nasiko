//! Tests for observability types — pure Rust, no external services.

use chrono::{TimeZone, Utc};
use std::collections::HashMap;

use nasiko_observability::pricing::CostBreakdown;
use nasiko_observability::types::{
    Session, Span, TokenUsage, TraceDetails, extract_token_attrs, latency_percentiles,
};
use nasiko_observability::find_root_span;

// ─── TokenUsage ───────────────────────────────────────────────────────────────

#[test]
fn token_usage_default_is_zero() {
    let usage = TokenUsage::default();
    assert_eq!(usage.input_tokens, 0);
    assert_eq!(usage.output_tokens, 0);
    assert_eq!(usage.total_tokens, 0);
}

#[test]
fn token_usage_serialization_roundtrip() {
    let usage = TokenUsage {
        input_tokens: 1024,
        output_tokens: 512,
        total_tokens: 1536,
    };
    let json = serde_json::to_string(&usage).unwrap();
    let back: TokenUsage = serde_json::from_str(&json).unwrap();
    assert_eq!(back.input_tokens, usage.input_tokens);
    assert_eq!(back.output_tokens, usage.output_tokens);
    assert_eq!(back.total_tokens, usage.total_tokens);
}

// ─── Span ─────────────────────────────────────────────────────────────────────

fn make_span(span_id: &str, service_name: &str) -> Span {
    Span {
        span_id: span_id.to_owned(),
        parent_span_id: None,
        name: "chat gpt-4o".to_owned(),
        started_at: Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap(),
        ended_at: Some(Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 1).unwrap()),
        duration_ms: Some(1000),
        service_name: service_name.to_owned(),
        kind: 2, // SPAN_KIND_SERVER
        status_code: 0,
        status_message: String::new(),
        attributes: HashMap::new(),
    }
}

fn gen_ai_span(span_id: &str, model: &str, input: u64, output: u64) -> Span {
    let mut span = make_span(span_id, "agent");
    span.attributes.insert(
        "gen_ai.usage.input_tokens".into(),
        serde_json::json!(input),
    );
    span.attributes.insert(
        "gen_ai.usage.output_tokens".into(),
        serde_json::json!(output),
    );
    span.attributes
        .insert("gen_ai.request.model".into(), serde_json::json!(model));
    span
}

#[test]
fn span_construction() {
    let span = make_span("abc123", "coding-agent");
    assert_eq!(span.span_id, "abc123");
    assert_eq!(span.service_name, "coding-agent");
    assert_eq!(span.kind, 2);
    assert_eq!(span.duration_ms, Some(1000));
    assert!(span.parent_span_id.is_none());
}

#[test]
fn span_serialization_roundtrip() {
    let span = make_span("ser-span", "my-agent");
    let json = serde_json::to_string(&span).unwrap();
    let back: Span = serde_json::from_str(&json).unwrap();
    assert_eq!(back.span_id, span.span_id);
    assert_eq!(back.service_name, span.service_name);
    assert_eq!(back.duration_ms, span.duration_ms);
}

// ─── extract_token_attrs ──────────────────────────────────────────────────────

#[test]
fn extract_token_attrs_semconv_names() {
    let span = gen_ai_span("s1", "gpt-4o", 312, 89);
    let (input, output, model) = extract_token_attrs(&span.attributes);
    assert_eq!(input, 312);
    assert_eq!(output, 89);
    assert_eq!(model.as_deref(), Some("gpt-4o"));
}

#[test]
fn extract_token_attrs_legacy_names_and_string_values() {
    let mut attrs: HashMap<String, serde_json::Value> = HashMap::new();
    attrs.insert("llm.usage.prompt_tokens".into(), serde_json::json!("42"));
    attrs.insert("llm.usage.completion_tokens".into(), serde_json::json!(7));
    attrs.insert("llm.request.model".into(), serde_json::json!("claude-3-5-haiku"));
    let (input, output, model) = extract_token_attrs(&attrs);
    assert_eq!(input, 42);
    assert_eq!(output, 7);
    assert_eq!(model.as_deref(), Some("claude-3-5-haiku"));
}

#[test]
fn extract_token_attrs_empty() {
    let attrs = HashMap::new();
    let (input, output, model) = extract_token_attrs(&attrs);
    assert_eq!((input, output), (0, 0));
    assert!(model.is_none());
}

// ─── TraceDetails ─────────────────────────────────────────────────────────────

fn make_trace(spans: Vec<Span>) -> TraceDetails {
    TraceDetails {
        trace_id: "trace-abc".to_owned(),
        started_at: spans.iter().map(|s| s.started_at).min(),
        ended_at: spans.iter().filter_map(|s| s.ended_at).max(),
        duration_ms: Some(5000),
        spans,
    }
}

#[test]
fn trace_token_totals_aggregates_across_spans() {
    let trace = make_trace(vec![
        gen_ai_span("s1", "gpt-4o", 200, 100),
        gen_ai_span("s2", "gpt-4o", 100, 25),
        make_span("s3", "agent"), // no gen_ai attrs — ignored
    ]);
    let (input, output, model) = trace.token_totals();
    assert_eq!(input, 300);
    assert_eq!(output, 125);
    assert_eq!(model.as_deref(), Some("gpt-4o"));
}

#[test]
fn trace_token_totals_zero_when_no_attributes() {
    let trace = make_trace(vec![make_span("s1", "agent")]);
    let (input, output, model) = trace.token_totals();
    assert_eq!((input, output), (0, 0));
    assert!(model.is_none());
}

#[test]
fn trace_token_totals_by_model_splits_mixed_traces() {
    let trace = make_trace(vec![
        gen_ai_span("s1", "gpt-4o", 200, 100),
        gen_ai_span("s2", "claude-3-5-haiku", 50, 20),
        gen_ai_span("s3", "gpt-4o", 100, 50),
    ]);
    let by_model = trace.token_totals_by_model();
    assert_eq!(by_model.len(), 2);
    let gpt = by_model
        .iter()
        .find(|(m, _, _)| m.as_deref() == Some("gpt-4o"))
        .unwrap();
    assert_eq!((gpt.1, gpt.2), (300, 150));
}

// ─── find_root_span ───────────────────────────────────────────────────────────

#[test]
fn find_root_span_picks_orphan_parent() {
    let mut child = make_span("child", "agent");
    child.parent_span_id = Some("root".into());
    let root = make_span("root", "agent");
    let spans = vec![child, root];
    assert_eq!(find_root_span(&spans).unwrap().span_id, "root");
}

#[test]
fn find_root_span_parent_missing_from_trace() {
    let mut span = make_span("only", "agent");
    span.parent_span_id = Some("not-in-trace".into());
    let spans = vec![span];
    assert_eq!(find_root_span(&spans).unwrap().span_id, "only");
}

#[test]
fn find_root_span_empty() {
    assert!(find_root_span(&[]).is_none());
}

// ─── latency_percentiles ──────────────────────────────────────────────────────

#[test]
fn latency_percentiles_empty() {
    let (p50, p99) = latency_percentiles(vec![]);
    assert!(p50.is_none());
    assert!(p99.is_none());
}

#[test]
fn latency_percentiles_sorted() {
    let (p50, p99) = latency_percentiles(vec![300, 100, 200, 400, 500]);
    assert_eq!(p50, Some(300.0));
    assert_eq!(p99, Some(400.0));
}

// ─── Session ──────────────────────────────────────────────────────────────────

#[test]
fn session_groups_multiple_traces() {
    let session = Session {
        session_id: "ses_14cda".to_owned(),
        agent_id: "agent-a".to_owned(),
        trace_ids: vec!["t1".to_owned(), "t2".to_owned(), "t3".to_owned()],
        started_at: Some(Utc.with_ymd_and_hms(2024, 6, 1, 10, 0, 0).unwrap()),
        ended_at: Some(Utc.with_ymd_and_hms(2024, 6, 1, 10, 5, 0).unwrap()),
        duration_ms: Some(300_000),
        input_tokens: 800,
        output_tokens: 400,
        model_used: Some("gpt-4o".to_owned()),
        latency_ms_p50: Some(1200.0),
        latency_ms_p99: Some(4000.0),
        cost: CostBreakdown::default(),
    };
    // One session == many traces (one per user query)
    assert_eq!(session.trace_ids.len(), 3);
    assert_ne!(session.session_id, session.trace_ids[0]);
}

#[test]
fn session_serialization_roundtrip() {
    let session = Session {
        session_id: "ses_ser".to_owned(),
        agent_id: "my-agent".to_owned(),
        trace_ids: vec!["t1".to_owned()],
        started_at: Some(Utc::now()),
        ended_at: None,
        duration_ms: None,
        input_tokens: 200,
        output_tokens: 100,
        model_used: None,
        latency_ms_p50: None,
        latency_ms_p99: None,
        cost: CostBreakdown::default(),
    };
    let json = serde_json::to_string(&session).unwrap();
    let back: Session = serde_json::from_str(&json).unwrap();
    assert_eq!(back.session_id, session.session_id);
    assert_eq!(back.trace_ids.len(), 1);
    assert_eq!(back.input_tokens, 200);
}
