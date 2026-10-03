//! Tests for observability types — pure Rust, no external services.

use chrono::{TimeZone, Utc};
use std::collections::HashMap;

use nasiko_observability::find_root_span;
use nasiko_observability::pricing::CostBreakdown;
use nasiko_observability::types::{
    Session, Span, TokenUsage, TraceDetails, extract_token_attrs, latency_percentiles,
    link_router_spans,
};

// ─── TokenUsage ───────────────────────────────────────────────────────────────

#[test]
fn token_usage_default_is_zero() {
    let usage = TokenUsage::default();
    assert_eq!(usage.input_tokens, 0);
    assert_eq!(usage.output_tokens, 0);
    assert_eq!(usage.cache_read_tokens, 0);
    assert_eq!(usage.cache_creation_tokens, 0);
    assert_eq!(usage.total_tokens, 0);
}

#[test]
fn token_usage_serialization_roundtrip() {
    let usage = TokenUsage {
        input_tokens: 1024,
        output_tokens: 512,
        cache_read_tokens: 128,
        cache_creation_tokens: 64,
        total_tokens: 1728,
    };
    let json = serde_json::to_string(&usage).unwrap();
    let back: TokenUsage = serde_json::from_str(&json).unwrap();
    assert_eq!(back.input_tokens, usage.input_tokens);
    assert_eq!(back.output_tokens, usage.output_tokens);
    assert_eq!(back.cache_read_tokens, usage.cache_read_tokens);
    assert_eq!(back.cache_creation_tokens, usage.cache_creation_tokens);
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
        events: vec![],
    }
}

fn gen_ai_span(span_id: &str, model: &str, input: u64, output: u64) -> Span {
    let mut span = make_span(span_id, "agent");
    span.attributes
        .insert("gen_ai.usage.input_tokens".into(), serde_json::json!(input));
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
    attrs.insert(
        "llm.request.model".into(),
        serde_json::json!("claude-3-5-haiku"),
    );
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
fn trace_token_totals_ignore_replayed_span_ids() {
    let span = gen_ai_span("replayed", "gpt-4o", 200, 100);
    let trace = make_trace(vec![span.clone(), span]);

    assert_eq!(trace.token_totals(), (200, 100, Some("gpt-4o".into())));
}

// ─── LLM-router span pairing ──────────────────────────────────────────────────

/// The router's own record of a call it served: the same tokens the calling
/// agent reports, but the model that actually ran. `gen_ai.agent.id` is what
/// marks it as the router's.
fn router_span(span_id: &str, parent: &str, model: &str, input: u64, output: u64) -> Span {
    let mut span = gen_ai_span(span_id, model, input, output);
    span.service_name = "nasiko-cp".to_owned();
    span.parent_span_id = Some(parent.to_owned());
    span.attributes
        .insert("gen_ai.agent.id".into(), serde_json::json!("agent-uuid"));
    span
}

#[test]
fn router_span_supersedes_the_agents_copy_of_the_same_call() {
    // One LLM call, two spans: the agent asked for gpt-4o-mini, the router
    // resolved that to gpt-6-astra and served it. Counting both doubles the
    // tokens; believing the agent prices an expensive call as a cheap one.
    let mut agent = gen_ai_span("llm", "gpt-4o-mini", 440, 67);
    agent.service_name = "devops-agent".to_owned();
    let mut http = make_span("req", "nasiko-cp");
    http.parent_span_id = Some("llm".to_owned());
    let trace = make_trace(vec![
        agent,
        http,
        router_span("chat", "req", "openai.gpt-6-astra", 440, 67),
    ]);

    let (input, output, model) = trace.token_totals();
    assert_eq!(input, 440, "the call is counted once, not twice");
    assert_eq!(output, 67);
    assert_eq!(
        model.as_deref(),
        Some("openai.gpt-6-astra"),
        "the model that ran, not the one the agent asked for"
    );
}

#[test]
fn an_unattributable_router_span_is_dropped_rather_than_double_counted() {
    // Nothing above it from another service, because the agent propagated the
    // context it was *called* with rather than its own LLM span's. Its copy of
    // the call is then a sibling instead of an ancestor — still a duplicate, but
    // one this pairing cannot see. Counting the router's span as well would
    // double the tokens, so it is dropped and the agent's figure stands.
    let mut agent = gen_ai_span("llm", "gpt-4o-mini", 440, 67);
    agent.service_name = "devops-agent".to_owned();
    let trace = make_trace(vec![
        agent,
        router_span("chat", "sibling-parent", "openai.gpt-6-astra", 440, 67),
    ]);

    let (input, output, _) = trace.token_totals();
    assert_eq!((input, output), (440, 67), "counted once, not twice");
}

#[test]
fn both_usage_paths_exclude_the_same_spans() {
    // The session view and the FinOps materializer aggregate separately over the
    // same spans and must agree — `cost_path_differential` is the live gate, and
    // this is the unit-level one. A rule applied to only one of them shows up as
    // a dashboard that disagrees with the trace it was derived from.
    let mut agent = gen_ai_span("llm", "gpt-4o-mini", 440, 67);
    agent.service_name = "devops-agent".to_owned();
    let mut http = make_span("req", "nasiko-cp");
    http.parent_span_id = Some("llm".to_owned());
    let spans = vec![
        agent,
        http,
        router_span("chat", "req", "openai.gpt-6-astra", 440, 67),
    ];

    let links = link_router_spans(&spans);
    assert!(
        links.excluded.contains("llm"),
        "the agent's copy is the one superseded"
    );
    assert_eq!(links.attributed.get("chat"), Some(&"devops-agent"));
    assert_eq!(make_trace(spans).token_totals().0, 440);
}

#[test]
fn trace_token_totals_by_model_splits_mixed_traces() {
    let mut cached = gen_ai_span("s1", "gpt-4o", 200, 100);
    cached.attributes.insert(
        "gen_ai.usage.cache_read_input_tokens".into(),
        serde_json::json!(25),
    );
    cached.attributes.insert(
        "gen_ai.usage.cache_creation_input_tokens".into(),
        serde_json::json!(10),
    );
    // This fixture uses the disjoint convention — `input_tokens` (200) excludes the 25+10
    // cached. Say so with `total_tokens`, as real instrumentation does: without it the counts
    // are equally consistent with the inclusive (OpenAI) reading and the extractor has to
    // guess, which is exactly the ambiguity that made the session view double-count.
    cached.attributes.insert(
        "gen_ai.usage.total_tokens".into(),
        serde_json::json!(200 + 25 + 10 + 100),
    );
    let trace = make_trace(vec![
        cached,
        gen_ai_span("s2", "claude-3-5-haiku", 50, 20),
        gen_ai_span("s3", "gpt-4o", 100, 50),
    ]);
    let by_model = trace.token_totals_by_model();
    assert_eq!(by_model.len(), 2);
    let gpt = by_model
        .iter()
        .find(|(m, _, _, _, _)| m.as_deref() == Some("gpt-4o"))
        .unwrap();
    assert_eq!((gpt.1, gpt.2), (300, 150));
    assert_eq!((gpt.3, gpt.4), (25, 10));
    let (usage, _) = trace.usage_totals();
    assert_eq!(usage.total_tokens, 555);
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
    assert_eq!(p99, Some(500.0));
}

#[test]
fn latency_p99_is_max_for_four_values() {
    let (_, p99) = latency_percentiles(vec![400, 100, 300, 200]);
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
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
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
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
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

// ─── extract_usage_attrs: the cached-token split ─────────────────────────────
//
// Costing charges `input` at the full rate and `cache_read` at the cache rate and sums
// them, so `input` must never still contain the cached tokens. These pin both directions:
// inclusive instrumentation must be reduced, disjoint instrumentation must be left alone.

use nasiko_observability::extract_usage_attrs;

fn attrs(pairs: &[(&str, u64)]) -> std::collections::HashMap<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
        .collect()
}

#[test]
fn openai_dotted_cache_attribute_is_read_at_all() {
    // The exact shape opentelemetry-instrumentation-openai emits. Before the dotted key was
    // recognised this returned cache_read = 0 and every cached token was billed in full.
    let u = extract_usage_attrs(&attrs(&[
        ("gen_ai.usage.input_tokens", 4732),
        ("gen_ai.usage.output_tokens", 110),
        ("gen_ai.usage.cache_read.input_tokens", 3968),
        ("gen_ai.usage.total_tokens", 4842),
    ]));
    assert_eq!(u.cache_read, 3968, "dotted cache attribute not recognised");
    assert_eq!(
        u.input, 764,
        "cached tokens left inside input — they bill twice"
    );
    assert_eq!(u.output, 110);
    assert_eq!(u.total_prompt(), 4732, "the prompt total must be preserved");
}

#[test]
fn inclusive_instrumentation_has_its_cached_subset_removed() {
    let u = extract_usage_attrs(&attrs(&[
        ("gen_ai.usage.input_tokens", 1000), // total prompt
        ("gen_ai.usage.output_tokens", 50),
        ("gen_ai.usage.cached_input_tokens", 800),
        ("gen_ai.usage.total_tokens", 1050), // == input + output ⇒ inclusive
    ]));
    assert_eq!(u.input, 200);
    assert_eq!(u.total_prompt(), 1000);
}

#[test]
fn disjoint_instrumentation_is_left_alone() {
    // Anthropic: input_tokens excludes cache reads, and total counts them separately.
    let u = extract_usage_attrs(&attrs(&[
        ("gen_ai.usage.input_tokens", 200),
        ("gen_ai.usage.output_tokens", 50),
        ("gen_ai.usage.cache_read_input_tokens", 800),
        ("gen_ai.usage.total_tokens", 1050), // == input + cached + output ⇒ disjoint
    ]));
    assert_eq!(u.input, 200, "a disjoint count was reduced — undercharges");
    assert_eq!(u.total_prompt(), 1000);
}

#[test]
fn input_smaller_than_the_cached_subset_cannot_be_inclusive() {
    // No total to arbitrate. 200 cannot contain 800, so the counts must be disjoint.
    let u = extract_usage_attrs(&attrs(&[
        ("gen_ai.usage.input_tokens", 200),
        ("gen_ai.usage.output_tokens", 50),
        ("gen_ai.usage.cache_read.input_tokens", 800),
    ]));
    assert_eq!(u.input, 200);
    assert_eq!(u.total_prompt(), 1000);
}

#[test]
fn ambiguous_counts_err_toward_not_double_charging() {
    // No total, and 1000 could plausibly contain 800. Both readings are possible; the
    // inclusive one is the semconv default and the only one that cannot bill twice.
    let u = extract_usage_attrs(&attrs(&[
        ("gen_ai.usage.input_tokens", 1000),
        ("gen_ai.usage.cache_read.input_tokens", 800),
    ]));
    assert_eq!(u.input, 200);
}

#[test]
fn declared_prompt_convention_overrides_ambiguous_or_inconsistent_totals() {
    for total in [None, Some(1050), Some(1850), Some(1)] {
        let mut attributes = attrs(&[
            ("gen_ai.usage.input_tokens", 1000),
            ("gen_ai.usage.output_tokens", 50),
            ("gen_ai.usage.cache_read_input_tokens", 800),
        ]);
        if let Some(total) = total {
            attributes.insert("gen_ai.usage.total_tokens".into(), total.into());
        }
        attributes.insert("nasiko.usage.prompt_convention".into(), "exclusive".into());
        let usage = extract_usage_attrs(&attributes);
        assert_eq!(usage.input, 1000);
        assert_eq!(usage.total_prompt(), 1800);
        attributes.insert("nasiko.usage.prompt_convention".into(), "inclusive".into());
        assert_eq!(extract_usage_attrs(&attributes).input, 200);
    }
}

#[test]
fn a_span_with_no_cache_is_untouched() {
    let u = extract_usage_attrs(&attrs(&[
        ("gen_ai.usage.input_tokens", 1000),
        ("gen_ai.usage.output_tokens", 50),
    ]));
    assert_eq!(u.input, 1000);
    assert_eq!(u.cache_read, 0);
    assert_eq!(u.total_prompt(), 1000);
}

#[test]
fn cache_creation_is_also_excluded_from_input() {
    let u = extract_usage_attrs(&attrs(&[
        ("gen_ai.usage.input_tokens", 1000),
        ("gen_ai.usage.output_tokens", 50),
        ("gen_ai.usage.cache_read.input_tokens", 600),
        ("gen_ai.usage.cache_creation.input_tokens", 300),
        ("gen_ai.usage.total_tokens", 1050),
    ]));
    assert_eq!(u.input, 100);
    assert_eq!(u.cache_creation, 300);
    assert_eq!(u.total_prompt(), 1000);
}

/// The invariant that matters: whatever the convention, the parts never exceed the prompt.
#[test]
fn the_split_never_exceeds_the_reported_prompt() {
    for input in [0u64, 1, 200, 999, 1000, 5000] {
        for cache in [0u64, 1, 200, 800, 5000] {
            let u = extract_usage_attrs(&attrs(&[
                ("gen_ai.usage.input_tokens", input),
                ("gen_ai.usage.cache_read.input_tokens", cache),
            ]));
            // Exactly two readings are legitimate: inclusive (prompt == input) and
            // disjoint (prompt == input + cache). Anything else invented tokens.
            assert!(
                u.total_prompt() == input || u.total_prompt() == input + cache,
                "input={input} cache={cache} produced prompt {} — neither reading",
                u.total_prompt()
            );
            // The billable portion can only ever shrink, never grow: growing it is the
            // double-charge this whole function exists to prevent.
            assert!(
                u.input <= input,
                "input={input} cache={cache} grew the billable input"
            );
        }
    }
}

// ─── TraceDetails aggregates: the cached tokens must be counted once ─────────

fn usage_span(id: &str, input: u64, output: u64, cache: u64, total: u64) -> Span {
    let mut attributes = std::collections::HashMap::new();
    attributes.insert("gen_ai.usage.input_tokens".into(), serde_json::json!(input));
    attributes.insert(
        "gen_ai.usage.output_tokens".into(),
        serde_json::json!(output),
    );
    attributes.insert(
        "gen_ai.usage.cache_read.input_tokens".into(),
        serde_json::json!(cache),
    );
    attributes.insert("gen_ai.usage.total_tokens".into(), serde_json::json!(total));
    attributes.insert(
        "gen_ai.request.model".into(),
        serde_json::json!("gpt-4o-mini"),
    );
    Span {
        span_id: id.into(),
        parent_span_id: None,
        name: "openai.chat".into(),
        started_at: chrono::Utc::now(),
        ended_at: None,
        duration_ms: Some(1),
        service_name: "translator".into(),
        kind: 3,
        status_code: 0,
        status_message: String::new(),
        attributes,
        events: vec![],
    }
}

fn trace(spans: Vec<Span>) -> TraceDetails {
    TraceDetails {
        trace_id: "t".into(),
        spans,
        started_at: None,
        ended_at: None,
        duration_ms: Some(1),
    }
}

/// The reported bug: the session view showed 7,992 tokens for a trace that was 6,456,
/// over by exactly the 1,536 cached tokens — `token_totals` left them inside `input`
/// and `usage_totals` then added `cache_read` on top.
#[test]
fn usage_totals_counts_the_cached_prompt_exactly_once() {
    // One real trace: 2,319 fresh + 1,536 cached prompt, 2,601 output.
    let t = trace(vec![
        usage_span("a", 1536 + 783, 2601, 1536, 1536 + 783 + 2601),
        usage_span("b", 1536, 0, 1536, 1536),
    ]);
    let (usage, model) = t.usage_totals();

    assert_eq!(usage.input_tokens, 783, "cached tokens left inside input");
    assert_eq!(usage.cache_read_tokens, 3072);
    assert_eq!(usage.output_tokens, 2601);
    assert_eq!(
        usage.total_tokens,
        783 + 3072 + 2601,
        "total double-counted the cached prompt"
    );
    assert_eq!(model.as_deref(), Some("gpt-4o-mini"));
}

#[test]
fn a_span_served_entirely_from_cache_is_not_skipped() {
    // No fresh input and no output — but 4,096 real cached tokens that must survive.
    let t = trace(vec![usage_span("only", 4096, 0, 4096, 4096)]);
    let (usage, _) = t.usage_totals();
    assert_eq!(usage.input_tokens, 0);
    assert_eq!(usage.cache_read_tokens, 4096);
    assert_eq!(usage.total_tokens, 4096);
}

#[test]
fn per_model_totals_also_count_the_cache_once() {
    let t = trace(vec![usage_span("a", 1000, 50, 800, 1050)]);
    let rows = t.token_totals_by_model();
    assert_eq!(rows.len(), 1);
    let (model, input, output, cache_read, _cache_creation) = &rows[0];
    assert_eq!(model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(*input, 200, "cached tokens left inside per-model input");
    assert_eq!(*cache_read, 800);
    assert_eq!(*output, 50);
}
