//! Tests for observability types — pure Rust, no external services.

use chrono::{TimeZone, Utc};
use std::collections::HashMap;

use nasiko_observability::types::{
    AgentFinOps, AgentStats, FinOpsDashboard, Session, Span, SpanDetails, TokenUsage, TraceDetails,
};

// ─── TokenUsage ───────────────────────────────────────────────────────────────

#[test]
fn token_usage_default_is_zero() {
    let usage = TokenUsage::default();
    assert_eq!(usage.input_tokens, 0);
    assert_eq!(usage.output_tokens, 0);
    assert_eq!(usage.total_tokens, 0);
}

#[test]
fn token_usage_construction() {
    let usage = TokenUsage {
        input_tokens: 100,
        output_tokens: 50,
        total_tokens: 150,
        estimated_cost_usd: 0.00,
    };
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 50);
    assert_eq!(usage.total_tokens, 150);
    assert_eq!(usage.estimated_cost_usd, 0.00);
}

#[test]
fn token_usage_serialization_roundtrip() {
    let usage = TokenUsage {
        input_tokens: 1024,
        output_tokens: 512,
        total_tokens: 1536,
        estimated_cost_usd: 0.00,
    };
    let json = serde_json::to_string(&usage).unwrap();
    let back: TokenUsage = serde_json::from_str(&json).unwrap();
    assert_eq!(back.input_tokens, usage.input_tokens);
    assert_eq!(back.output_tokens, usage.output_tokens);
    assert_eq!(back.total_tokens, usage.total_tokens);
    assert_eq!(back.estimated_cost_usd, usage.estimated_cost_usd);
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
fn span_with_parent() {
    let mut span = make_span("child-span", "agent-a");
    span.parent_span_id = Some("parent-span".to_owned());
    assert_eq!(span.parent_span_id.as_deref(), Some("parent-span"));
}

#[test]
fn span_with_attributes() {
    let mut span = make_span("span-1", "agent-b");
    span.attributes.insert(
        "gen_ai.usage.input_tokens".to_owned(),
        serde_json::json!(312u64),
    );
    span.attributes.insert(
        "gen_ai.usage.output_tokens".to_owned(),
        serde_json::json!(89u64),
    );
    assert_eq!(
        span.attributes["gen_ai.usage.input_tokens"].as_u64(),
        Some(312)
    );
    assert_eq!(
        span.attributes["gen_ai.usage.output_tokens"].as_u64(),
        Some(89)
    );
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

// ─── TraceDetails ─────────────────────────────────────────────────────────────

#[test]
fn trace_details_construction() {
    let details = TraceDetails {
        trace_id: "trace-abc".to_owned(),
        spans: vec![make_span("s1", "agent-a"), make_span("s2", "agent-b")],
        started_at: Some(Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap()),
        ended_at: Some(Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 5).unwrap()),
        duration_ms: Some(5000),
    };
    assert_eq!(details.trace_id, "trace-abc");
    assert_eq!(details.spans.len(), 2);
    assert_eq!(details.duration_ms, Some(5000));
}

#[test]
fn trace_details_empty_spans() {
    let details = TraceDetails {
        trace_id: "empty-trace".to_owned(),
        spans: vec![],
        started_at: None,
        ended_at: None,
        duration_ms: None,
    };
    assert!(details.spans.is_empty());
    assert!(details.started_at.is_none());
    assert!(details.duration_ms.is_none());
}

#[test]
fn trace_details_token_usage_aggregates_across_spans() {
    let mut span1 = make_span("s1", "agent-a");
    span1.attributes.insert(
        "gen_ai.usage.input_tokens".to_owned(),
        serde_json::json!(200u64),
    );
    span1.attributes.insert(
        "gen_ai.usage.output_tokens".to_owned(),
        serde_json::json!(50u64),
    );

    let mut span2 = make_span("s2", "agent-b");
    span2.attributes.insert(
        "gen_ai.usage.input_tokens".to_owned(),
        serde_json::json!(100u64),
    );
    span2.attributes.insert(
        "gen_ai.usage.output_tokens".to_owned(),
        serde_json::json!(75u64),
    );

    let details = TraceDetails {
        trace_id: "tok-trace".to_owned(),
        spans: vec![span1, span2],
        started_at: None,
        ended_at: None,
        duration_ms: None,
    };

    let usage = details.token_usage();
    assert_eq!(usage.input_tokens, 300);
    assert_eq!(usage.output_tokens, 125);
    assert_eq!(usage.total_tokens, 425);
}

#[test]
fn trace_details_token_usage_zero_when_no_attributes() {
    let details = TraceDetails {
        trace_id: "zero-tok".to_owned(),
        spans: vec![make_span("s1", "agent-a")],
        started_at: None,
        ended_at: None,
        duration_ms: None,
    };
    let usage = details.token_usage();
    assert_eq!(usage.input_tokens, 0);
    assert_eq!(usage.output_tokens, 0);
    assert_eq!(usage.total_tokens, 0);
}

#[test]
fn trace_details_serialization_roundtrip() {
    let details = TraceDetails {
        trace_id: "ser-trace".to_owned(),
        spans: vec![make_span("s1", "svc-a")],
        started_at: Some(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()),
        ended_at: None,
        duration_ms: Some(500),
    };
    let json = serde_json::to_string(&details).unwrap();
    let back: TraceDetails = serde_json::from_str(&json).unwrap();
    assert_eq!(back.trace_id, details.trace_id);
    assert_eq!(back.spans.len(), 1);
    assert_eq!(back.duration_ms, Some(500));
}

// ─── SpanDetails ──────────────────────────────────────────────────────────────

#[test]
fn span_details_construction_no_logs() {
    let details = SpanDetails {
        span: make_span("sd-span", "agent-c"),
        prompt_content: None,
        completion_content: None,
    };
    assert_eq!(details.span.span_id, "sd-span");
    assert!(details.prompt_content.is_none());
    assert!(details.completion_content.is_none());
}

#[test]
fn span_details_construction_with_logs() {
    let details = SpanDetails {
        span: make_span("sd-span-2", "agent-d"),
        prompt_content: Some(r#"{"prompt": "Hello, world!"}"#.to_owned()),
        completion_content: Some(r#"{"completion": "Hi there!"}"#.to_owned()),
    };
    assert!(details.prompt_content.is_some());
    assert!(details.completion_content.is_some());
}

#[test]
fn span_details_serialization_roundtrip() {
    let details = SpanDetails {
        span: make_span("sd-ser", "svc"),
        prompt_content: Some("prompt text".to_owned()),
        completion_content: None,
    };
    let json = serde_json::to_string(&details).unwrap();
    let back: SpanDetails = serde_json::from_str(&json).unwrap();
    assert_eq!(back.span.span_id, "sd-ser");
    assert_eq!(back.prompt_content.as_deref(), Some("prompt text"));
    assert!(back.completion_content.is_none());
}

// ─── Session ──────────────────────────────────────────────────────────────────

#[test]
fn session_construction() {
    let session = Session {
        trace_id: "trace-xyz".to_owned(),
        agent_ids: vec!["agent-a".to_owned(), "agent-b".to_owned()],
        started_at: Utc.with_ymd_and_hms(2024, 6, 1, 10, 0, 0).unwrap(),
        ended_at: Some(Utc.with_ymd_and_hms(2024, 6, 1, 10, 0, 30).unwrap()),
        duration_ms: Some(30_000),
        span_count: 12,
        total_input_tokens: 800,
        total_output_tokens: 400,
    };
    assert_eq!(session.trace_id, "trace-xyz");
    assert_eq!(session.agent_ids.len(), 2);
    assert_eq!(session.span_count, 12);
    assert_eq!(session.total_input_tokens, 800);
    assert_eq!(session.total_output_tokens, 400);
}

#[test]
fn session_serialization_roundtrip() {
    let session = Session {
        trace_id: "ser-sess".to_owned(),
        agent_ids: vec!["my-agent".to_owned()],
        started_at: Utc::now(),
        ended_at: None,
        duration_ms: None,
        span_count: 3,
        total_input_tokens: 200,
        total_output_tokens: 100,
    };
    let json = serde_json::to_string(&session).unwrap();
    let back: Session = serde_json::from_str(&json).unwrap();
    assert_eq!(back.trace_id, session.trace_id);
    assert_eq!(back.span_count, 3);
    assert_eq!(back.total_input_tokens, 200);
}

// ─── AgentStats ───────────────────────────────────────────────────────────────

#[test]
fn agent_stats_construction() {
    let stats = AgentStats {
        agent_id: "coding-agent".to_owned(),
        total_requests: 42,
        total_tokens: TokenUsage {
            input_tokens: 5000,
            output_tokens: 2500,
            total_tokens: 7500,
            estimated_cost_usd: 0.00,
        },
        avg_latency_ms: 350.5,
        error_rate: 0.02,
        period_start: Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap(),
    };
    assert_eq!(stats.agent_id, "coding-agent");
    assert_eq!(stats.total_requests, 42);
    assert_eq!(stats.total_tokens.total_tokens, 7500);
    assert!((stats.avg_latency_ms - 350.5).abs() < f64::EPSILON);
    assert!((stats.error_rate - 0.02).abs() < f64::EPSILON);
}

#[test]
fn agent_stats_zero_requests() {
    let stats = AgentStats {
        agent_id: "idle-agent".to_owned(),
        total_requests: 0,
        total_tokens: TokenUsage::default(),
        avg_latency_ms: 0.0,
        error_rate: 0.0,
        period_start: Utc::now(),
    };
    assert_eq!(stats.total_requests, 0);
    assert_eq!(stats.total_tokens.total_tokens, 0);
}

#[test]
fn agent_stats_serialization_roundtrip() {
    let stats = AgentStats {
        agent_id: "ser-agent".to_owned(),
        total_requests: 10,
        total_tokens: TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            estimated_cost_usd: 0.00,
        },
        avg_latency_ms: 200.0,
        error_rate: 0.0,
        period_start: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
    };
    let json = serde_json::to_string(&stats).unwrap();
    let back: AgentStats = serde_json::from_str(&json).unwrap();
    assert_eq!(back.agent_id, stats.agent_id);
    assert_eq!(back.total_requests, stats.total_requests);
    assert_eq!(back.total_tokens.input_tokens, stats.total_tokens.input_tokens);
}

// ─── AgentFinOps ──────────────────────────────────────────────────────────────

#[test]
fn agent_finops_construction() {
    let finops = AgentFinOps {
        agent_id: "expensive-agent".to_owned(),
        total_input_tokens: 100_000,
        total_output_tokens: 50_000,
        estimated_cost_usd: 0.50,
        request_count: 200,
    };
    assert_eq!(finops.agent_id, "expensive-agent");
    assert_eq!(finops.total_input_tokens, 100_000);
    assert_eq!(finops.total_output_tokens, 50_000);
    assert!((finops.estimated_cost_usd - 0.50).abs() < 1e-9);
    assert_eq!(finops.request_count, 200);
}

#[test]
fn agent_finops_serialization_roundtrip() {
    let finops = AgentFinOps {
        agent_id: "test-agent".to_owned(),
        total_input_tokens: 1000,
        total_output_tokens: 500,
        estimated_cost_usd: 0.005,
        request_count: 5,
    };
    let json = serde_json::to_string(&finops).unwrap();
    let back: AgentFinOps = serde_json::from_str(&json).unwrap();
    assert_eq!(back.agent_id, finops.agent_id);
    assert_eq!(back.total_input_tokens, finops.total_input_tokens);
    assert_eq!(back.request_count, finops.request_count);
}

// ─── FinOpsDashboard ──────────────────────────────────────────────────────────

#[test]
fn finops_dashboard_construction() {
    let dashboard = FinOpsDashboard {
        period_start: Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap(),
        period_end: Utc.with_ymd_and_hms(2024, 6, 30, 23, 59, 59).unwrap(),
        agents: vec![
            AgentFinOps {
                agent_id: "agent-a".to_owned(),
                total_input_tokens: 50_000,
                total_output_tokens: 25_000,
                estimated_cost_usd: 0.25,
                request_count: 100,
            },
            AgentFinOps {
                agent_id: "agent-b".to_owned(),
                total_input_tokens: 30_000,
                total_output_tokens: 15_000,
                estimated_cost_usd: 0.15,
                request_count: 60,
            },
        ],
        total_input_tokens: 80_000,
        total_output_tokens: 40_000,
        total_estimated_cost_usd: 0.40,
    };
    assert_eq!(dashboard.agents.len(), 2);
    assert_eq!(dashboard.total_input_tokens, 80_000);
    assert_eq!(dashboard.total_output_tokens, 40_000);
    assert!((dashboard.total_estimated_cost_usd - 0.40).abs() < 1e-9);
}

#[test]
fn finops_dashboard_empty_agents() {
    let dashboard = FinOpsDashboard {
        period_start: Utc::now(),
        period_end: Utc::now(),
        agents: vec![],
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_estimated_cost_usd: 0.0,
    };
    assert!(dashboard.agents.is_empty());
    assert_eq!(dashboard.total_input_tokens, 0);
}

#[test]
fn finops_dashboard_serialization_roundtrip() {
    let dashboard = FinOpsDashboard {
        period_start: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        period_end: Utc.with_ymd_and_hms(2024, 1, 31, 0, 0, 0).unwrap(),
        agents: vec![AgentFinOps {
            agent_id: "test".to_owned(),
            total_input_tokens: 100,
            total_output_tokens: 50,
            estimated_cost_usd: 0.001,
            request_count: 1,
        }],
        total_input_tokens: 100,
        total_output_tokens: 50,
        total_estimated_cost_usd: 0.001,
    };
    let json = serde_json::to_string(&dashboard).unwrap();
    let back: FinOpsDashboard = serde_json::from_str(&json).unwrap();
    assert_eq!(back.agents.len(), 1);
    assert_eq!(back.total_input_tokens, 100);
    assert_eq!(back.agents[0].agent_id, "test");
}