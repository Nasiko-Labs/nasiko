use nasiko_flow::FlowContext;

// ── Parse valid traceparent header ────────────────────────────────────────

#[test]
fn parse_valid_traceparent_version() {
    let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let ctx = FlowContext::from_traceparent(tp).expect("must parse");
    // Version is not stored, but parse must succeed for version "00"
    assert_eq!(ctx.flow_id, "4bf92f3577b34da6a3ce929d0e0e4736");
}

#[test]
fn parse_valid_traceparent_trace_id_32_chars() {
    let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let ctx = FlowContext::from_traceparent(tp).expect("must parse");
    assert_eq!(ctx.flow_id.len(), 32);
}

#[test]
fn parse_valid_traceparent_span_id_16_chars() {
    let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let ctx = FlowContext::from_traceparent(tp).expect("must parse");
    assert_eq!(ctx.parent_span_id.len(), 16);
}

#[test]
fn parse_valid_traceparent_all_zeros_trace_id() {
    // All-zeros trace_id is technically valid per the W3C spec (just considered invalid
    // for sampling decisions, but the parser should still parse it).
    let tp = "00-00000000000000000000000000000000-00f067aa0ba902b7-01";
    let ctx = FlowContext::from_traceparent(tp).expect("must parse");
    assert_eq!(ctx.flow_id, "00000000000000000000000000000000");
}

#[test]
fn parse_valid_traceparent_all_zeros_span_id() {
    let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01";
    let ctx = FlowContext::from_traceparent(tp).expect("must parse");
    assert_eq!(ctx.parent_span_id, "0000000000000000");
}

// ── Parse invalid traceparent → None ──────────────────────────────────────

#[test]
fn parse_empty_string_returns_none() {
    assert!(FlowContext::from_traceparent("").is_none());
}

#[test]
fn parse_too_few_segments_returns_none() {
    // Only 3 segments separated by '-' (one less than required)
    assert!(FlowContext::from_traceparent("00-traceid-spanid").is_none());
}

#[test]
fn parse_short_trace_id_returns_none() {
    // trace_id is 31 chars instead of 32
    let tp = "00-4bf92f3577b34da6a3ce929d0e0473-00f067aa0ba902b7-01";
    assert!(FlowContext::from_traceparent(tp).is_none());
}

#[test]
fn parse_long_trace_id_returns_none() {
    // trace_id is 33 chars — the field lengths are strict
    let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736f-00f067aa0ba902b7-01";
    assert!(FlowContext::from_traceparent(tp).is_none());
}

#[test]
fn parse_short_span_id_returns_none() {
    // span_id is 15 chars instead of 16
    let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b-01";
    assert!(FlowContext::from_traceparent(tp).is_none());
}

#[test]
fn parse_long_span_id_returns_none() {
    // span_id is 17 chars
    let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7a-01";
    assert!(FlowContext::from_traceparent(tp).is_none());
}

#[test]
fn parse_garbage_string_returns_none() {
    assert!(FlowContext::from_traceparent("this-is-not-valid-at-all-surely").is_none());
}

#[test]
fn parse_random_short_string_returns_none() {
    assert!(FlowContext::from_traceparent("abc").is_none());
}

// ── Generate new traceparent ───────────────────────────────────────────────

#[test]
fn generate_traceparent_from_new_root() {
    let ctx = FlowContext::new_root();
    let tp = ctx.to_traceparent();
    let parts: Vec<&str> = tp.split('-').collect();
    assert_eq!(
        parts.len(),
        4,
        "W3C traceparent has exactly 4 dash-separated segments"
    );
    assert_eq!(parts[0], "00", "version must be 00");
    assert_eq!(parts[1].len(), 32, "trace_id must be 32 chars");
    assert_eq!(parts[2].len(), 16, "span_id must be 16 chars");
    assert_eq!(parts[3], "01", "flags must be 01 (sampled)");
}

#[test]
fn generate_traceparent_contains_only_hex_and_dashes() {
    let ctx = FlowContext::new_root();
    let tp = ctx.to_traceparent();
    assert!(
        tp.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "traceparent must contain only hex digits and dashes, got: {tp}"
    );
}

// ── Round-trip encode / decode ─────────────────────────────────────────────

#[test]
fn round_trip_trace_id_preserved() {
    let original_tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let ctx = FlowContext::from_traceparent(original_tp).expect("parse");
    let generated = ctx.to_traceparent();
    // Re-parse the generated header
    let ctx2 = FlowContext::from_traceparent(&generated).expect("re-parse");
    assert_eq!(
        ctx2.flow_id, ctx.flow_id,
        "trace_id survives encode-decode round-trip"
    );
}

#[test]
fn round_trip_generates_parseable_header() {
    let ctx = FlowContext::new_root();
    let tp = ctx.to_traceparent();
    let reparsed = FlowContext::from_traceparent(&tp);
    assert!(
        reparsed.is_some(),
        "to_traceparent() output must be parseable by from_traceparent()"
    );
}

#[test]
fn round_trip_reparsed_trace_id_matches_original() {
    let ctx = FlowContext::new_root();
    let tp = ctx.to_traceparent();
    let reparsed = FlowContext::from_traceparent(&tp).unwrap();
    assert_eq!(reparsed.flow_id, ctx.flow_id);
}

#[test]
fn round_trip_span_id_is_fresh() {
    // to_traceparent() generates a new child span_id, so the reparsed context
    // will have a different parent_span_id than the original.
    let ctx =
        FlowContext::from_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .unwrap();
    let tp = ctx.to_traceparent();
    let child_ctx = FlowContext::from_traceparent(&tp).unwrap();
    assert_ne!(
        child_ctx.parent_span_id, ctx.parent_span_id,
        "child context must carry a new span_id"
    );
}

#[test]
fn multiple_round_trips_keep_same_trace_id() {
    let root = FlowContext::new_root();
    let tp1 = root.to_traceparent();
    let ctx1 = FlowContext::from_traceparent(&tp1).unwrap();
    let tp2 = ctx1.to_traceparent();
    let ctx2 = FlowContext::from_traceparent(&tp2).unwrap();

    assert_eq!(
        ctx2.flow_id, root.flow_id,
        "trace_id must remain stable across multiple hops"
    );
}
