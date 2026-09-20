use nasiko_flow::{FlowContext, TRACEPARENT_HEADER};

// ── Constants ──────────────────────────────────────────────────────────────

const VALID_TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

// ── FlowContext creation ───────────────────────────────────────────────────

#[test]
fn new_root_flow_id_is_32_hex_chars() {
    let ctx = FlowContext::new_root();
    assert_eq!(
        ctx.flow_id.len(),
        32,
        "flow_id must be 32 hex chars (W3C trace-id)"
    );
    assert!(
        ctx.flow_id.chars().all(|c| c.is_ascii_hexdigit()),
        "flow_id must be all hex digits"
    );
}

#[test]
fn new_root_span_id_is_16_hex_chars() {
    let ctx = FlowContext::new_root();
    assert_eq!(
        ctx.parent_span_id.len(),
        16,
        "parent_span_id must be 16 hex chars (W3C span-id)"
    );
    assert!(
        ctx.parent_span_id.chars().all(|c| c.is_ascii_hexdigit()),
        "parent_span_id must be all hex digits"
    );
}

#[test]
fn new_root_produces_unique_flow_ids() {
    let a = FlowContext::new_root();
    let b = FlowContext::new_root();
    assert_ne!(
        a.flow_id, b.flow_id,
        "each new_root() must generate a unique flow_id"
    );
}

#[test]
fn new_root_produces_unique_span_ids() {
    let a = FlowContext::new_root();
    let b = FlowContext::new_root();
    // Span IDs being different is highly probable; the test documents the contract.
    assert_ne!(
        a.parent_span_id, b.parent_span_id,
        "each new_root() should generate a unique parent_span_id"
    );
}

// ── Traceparent parsing ────────────────────────────────────────────────────

#[test]
fn from_traceparent_valid_parses_trace_id() {
    let ctx = FlowContext::from_traceparent(VALID_TRACEPARENT).expect("should parse");
    assert_eq!(ctx.flow_id, "4bf92f3577b34da6a3ce929d0e0e4736");
}

#[test]
fn from_traceparent_valid_parses_parent_id() {
    let ctx = FlowContext::from_traceparent(VALID_TRACEPARENT).expect("should parse");
    assert_eq!(ctx.parent_span_id, "00f067aa0ba902b7");
}

#[test]
fn from_traceparent_valid_sampled_flag_is_ignored() {
    // The parser must accept both sampled (01) and not-sampled (00) flags.
    let unsampled = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00";
    assert!(FlowContext::from_traceparent(unsampled).is_some());
}

#[test]
fn from_traceparent_too_few_parts_returns_none() {
    assert!(FlowContext::from_traceparent("not-a-traceparent").is_none());
}

#[test]
fn from_traceparent_short_ids_returns_none() {
    // trace_id is only 5 chars, parent_id only 5 chars
    assert!(FlowContext::from_traceparent("00-short-short-01").is_none());
}

#[test]
fn from_traceparent_short_trace_id_returns_none() {
    // trace_id too short (31 chars instead of 32)
    let bad = "00-4bf92f3577b34da6a3ce929d0e0473-00f067aa0ba902b7-01";
    assert!(FlowContext::from_traceparent(bad).is_none());
}

#[test]
fn from_traceparent_short_span_id_returns_none() {
    // span_id too short (15 chars instead of 16)
    let bad = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b-01";
    assert!(FlowContext::from_traceparent(bad).is_none());
}

#[test]
fn from_traceparent_empty_string_returns_none() {
    assert!(FlowContext::from_traceparent("").is_none());
}

#[test]
fn traceparent_header_constant_value() {
    assert_eq!(TRACEPARENT_HEADER, "traceparent");
}

// ── Child context (to_traceparent) ────────────────────────────────────────

#[test]
fn to_traceparent_preserves_trace_id() {
    let parent = FlowContext::from_traceparent(VALID_TRACEPARENT).expect("should parse");
    let tp = parent.to_traceparent();
    let parts: Vec<&str> = tp.split('-').collect();
    assert_eq!(
        parts[1], "4bf92f3577b34da6a3ce929d0e0e4736",
        "trace_id must be preserved in child"
    );
}

#[test]
fn to_traceparent_generates_new_span_id() {
    let parent = FlowContext::from_traceparent(VALID_TRACEPARENT).expect("should parse");
    let tp = parent.to_traceparent();
    let parts: Vec<&str> = tp.split('-').collect();
    // The child span_id replaces the parent's span_id
    assert_ne!(
        parts[2], "00f067aa0ba902b7",
        "child must have a fresh span_id"
    );
}

#[test]
fn to_traceparent_child_span_id_is_16_hex_chars() {
    let parent = FlowContext::from_traceparent(VALID_TRACEPARENT).expect("should parse");
    let tp = parent.to_traceparent();
    let parts: Vec<&str> = tp.split('-').collect();
    assert_eq!(parts.len(), 4);
    assert_eq!(parts[2].len(), 16);
    assert!(parts[2].chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn to_traceparent_has_correct_version_prefix() {
    let ctx = FlowContext::from_traceparent(VALID_TRACEPARENT).expect("should parse");
    let tp = ctx.to_traceparent();
    assert!(
        tp.starts_with("00-"),
        "W3C traceparent version must be '00'"
    );
}

#[test]
fn to_traceparent_has_sampled_flag() {
    let ctx = FlowContext::from_traceparent(VALID_TRACEPARENT).expect("should parse");
    let tp = ctx.to_traceparent();
    assert!(
        tp.ends_with("-01"),
        "outbound traceparent must set sampled flag"
    );
}

#[test]
fn consecutive_to_traceparent_produce_unique_span_ids() {
    let ctx = FlowContext::from_traceparent(VALID_TRACEPARENT).expect("should parse");
    let tp1 = ctx.to_traceparent();
    let tp2 = ctx.to_traceparent();
    let span1 = tp1.split('-').nth(2).unwrap();
    let span2 = tp2.split('-').nth(2).unwrap();
    assert_ne!(
        span1, span2,
        "each to_traceparent() call must yield a unique child span_id"
    );
}

// ── Redis key format ───────────────────────────────────────────────────────

#[test]
fn redis_key_has_flow_prefix() {
    let ctx = FlowContext::from_traceparent(VALID_TRACEPARENT).expect("should parse");
    assert!(
        ctx.redis_key().starts_with("flow:"),
        "redis key must start with 'flow:'"
    );
}

#[test]
fn redis_key_contains_flow_id() {
    let ctx = FlowContext::from_traceparent(VALID_TRACEPARENT).expect("should parse");
    assert_eq!(ctx.redis_key(), "flow:4bf92f3577b34da6a3ce929d0e0e4736");
}

#[test]
fn redis_key_differs_across_roots() {
    let a = FlowContext::new_root();
    let b = FlowContext::new_root();
    assert_ne!(a.redis_key(), b.redis_key());
}

// ── Serde round-trip ───────────────────────────────────────────────────────

#[test]
fn flow_context_serialises_and_deserialises() {
    let ctx = FlowContext::from_traceparent(VALID_TRACEPARENT).expect("should parse");
    let json = serde_json::to_string(&ctx).expect("serialise");
    let back: FlowContext = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(back.flow_id, ctx.flow_id);
    assert_eq!(back.parent_span_id, ctx.parent_span_id);
}
