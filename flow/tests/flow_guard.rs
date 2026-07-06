use nasiko_flow::{FlowConfig, FlowContext, FlowGuard, FlowRejection};

// ── FlowConfig defaults ────────────────────────────────────────────────────

#[test]
fn flow_config_default_max_depth() {
    let cfg = FlowConfig::default();
    assert_eq!(cfg.max_depth, 5);
}

#[test]
fn flow_config_default_max_fan_out() {
    let cfg = FlowConfig::default();
    assert_eq!(cfg.max_fan_out, 20);
}

#[test]
fn flow_config_default_max_flow_tokens() {
    let cfg = FlowConfig::default();
    assert_eq!(cfg.max_flow_tokens, 100_000);
}

#[test]
fn flow_config_default_flow_timeout_secs() {
    let cfg = FlowConfig::default();
    assert_eq!(cfg.flow_timeout_secs, 120);
}

#[test]
fn flow_config_default_state_ttl_secs() {
    let cfg = FlowConfig::default();
    assert_eq!(cfg.flow_state_ttl_secs, 300);
}

// ── FlowRejection Display ──────────────────────────────────────────────────

#[test]
fn flow_rejection_max_depth_display() {
    let r = FlowRejection::MaxDepthExceeded { depth: 5, max: 5 };
    let s = r.to_string();
    assert!(s.contains("depth"), "display must mention 'depth'");
    assert!(s.contains('5'), "display must include the values");
}

#[test]
fn flow_rejection_cycle_display() {
    let r = FlowRejection::CycleDetected {
        agent_id: "agent-a".to_string(),
        chain: vec!["agent-a".to_string(), "agent-b".to_string()],
    };
    let s = r.to_string();
    assert!(s.contains("cycle"), "display must mention 'cycle'");
    assert!(s.contains("agent-a"));
}

#[test]
fn flow_rejection_fan_out_display() {
    let r = FlowRejection::MaxFanOutExceeded { invocations: 21, max: 20 };
    let s = r.to_string();
    assert!(s.contains("fan"), "display must mention fan-out");
    assert!(s.contains("21") && s.contains("20"));
}

#[test]
fn flow_rejection_token_budget_display() {
    let r = FlowRejection::TokenBudgetExhausted { used: 100_001, max: 100_000 };
    let s = r.to_string();
    assert!(s.contains("token"), "display must mention tokens");
}

#[test]
fn flow_rejection_timeout_display() {
    let r = FlowRejection::FlowTimeout { elapsed_secs: 130, max: 120 };
    let s = r.to_string();
    assert!(s.contains("timeout") || s.contains("time"), "display must mention timeout");
    assert!(s.contains("130"));
}

// ── FlowGuard without Redis (graceful degradation) ─────────────────────────
//
// When Redis is unreachable the guard methods fall back to Ok(()) rather than
// panicking. These tests use a deliberately bad Redis URL to exercise that path.

fn unreachable_guard() -> FlowGuard {
    let client = redis::Client::open("redis://127.0.0.1:1/").expect("client creation always succeeds");
    FlowGuard::new(client, FlowConfig::default())
}

#[tokio::test]
async fn guard_check_without_redis_allows() {
    let guard = unreachable_guard();
    let ctx = FlowContext::new_root();
    let result = guard.check(&ctx, "agent-x").await;
    assert!(result.is_ok(), "when Redis is unreachable, check() must allow (fail-open)");
}

#[tokio::test]
async fn guard_record_invocation_without_redis_allows() {
    let guard = unreachable_guard();
    let ctx = FlowContext::new_root();
    let result = guard.record_invocation(&ctx, "agent-x").await;
    assert!(result.is_ok(), "when Redis is unreachable, record_invocation() must allow");
}

#[tokio::test]
async fn guard_record_tokens_without_redis_returns_max() {
    let guard = unreachable_guard();
    let ctx = FlowContext::new_root();
    let result = guard.record_tokens(&ctx, 500).await;
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        FlowConfig::default().max_flow_tokens,
        "when Redis unreachable, remaining budget equals max"
    );
}

#[tokio::test]
async fn guard_record_return_without_redis_is_noop() {
    let guard = unreachable_guard();
    let ctx = FlowContext::new_root();
    // Must not panic
    guard.record_return(&ctx).await;
}

#[tokio::test]
async fn guard_init_flow_without_redis_is_noop() {
    let guard = unreachable_guard();
    let ctx = FlowContext::new_root();
    // Must not panic
    guard.init_flow(&ctx, "root-agent").await;
}

// ── FlowGuard config accessor ──────────────────────────────────────────────

#[test]
fn guard_config_accessor() {
    let cfg = FlowConfig { max_depth: 3, ..FlowConfig::default() };
    let client = redis::Client::open("redis://127.0.0.1:1/").unwrap();
    let guard = FlowGuard::new(client, cfg);
    assert_eq!(guard.config().max_depth, 3);
}

// ── FlowGuard with real Redis (integration) ────────────────────────────────
//
// These tests require a running Redis at REDIS_URL (default redis://127.0.0.1:6379).
// Run them with:  cargo test --test flow_guard -- --include-ignored

#[tokio::test]
#[ignore = "requires Redis"]
async fn guard_enforces_depth_limit() {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".into());
    let client = redis::Client::open(url).unwrap();
    let config = FlowConfig { max_depth: 2, ..FlowConfig::default() };
    let guard = FlowGuard::new(client, config);

    let ctx = FlowContext::new_root();
    guard.init_flow(&ctx, "agent-root").await;

    // First invocation: depth becomes 1 → ok
    guard.record_invocation(&ctx, "agent-a").await.unwrap();
    // Second invocation: depth becomes 2 → ok (still within limit)
    guard.record_invocation(&ctx, "agent-b").await.unwrap();
    // Third invocation: depth becomes 3 → exceeds max_depth=2
    let result = guard.record_invocation(&ctx, "agent-c").await;
    assert!(
        matches!(result, Err(FlowRejection::MaxDepthExceeded { .. })),
        "expected MaxDepthExceeded, got {:?}",
        result
    );
}

#[tokio::test]
#[ignore = "requires Redis"]
async fn guard_enforces_fan_out_limit() {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".into());
    let client = redis::Client::open(url).unwrap();
    let config = FlowConfig {
        max_fan_out: 2,
        max_depth: 100,
        ..FlowConfig::default()
    };
    let guard = FlowGuard::new(client, config);

    let ctx = FlowContext::new_root();
    guard.init_flow(&ctx, "root").await;

    guard.record_invocation(&ctx, "a1").await.unwrap();
    guard.record_invocation(&ctx, "a2").await.unwrap();
    let result = guard.record_invocation(&ctx, "a3").await;
    assert!(
        matches!(result, Err(FlowRejection::MaxFanOutExceeded { .. })),
        "expected MaxFanOutExceeded, got {:?}",
        result
    );
}

#[tokio::test]
#[ignore = "requires Redis"]
async fn guard_allows_within_limits() {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".into());
    let client = redis::Client::open(url).unwrap();
    let config = FlowConfig { max_depth: 5, max_fan_out: 10, ..FlowConfig::default() };
    let guard = FlowGuard::new(client, config);

    let ctx = FlowContext::new_root();
    guard.init_flow(&ctx, "root").await;

    guard.check(&ctx, "target").await.unwrap();
    guard.record_invocation(&ctx, "target").await.unwrap();
    guard.check(&ctx, "target2").await.unwrap();
}

#[tokio::test]
#[ignore = "requires Redis"]
async fn guard_cycle_detected_via_check() {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".into());
    let client = redis::Client::open(url).unwrap();
    let guard = FlowGuard::new(client, FlowConfig::default());

    let ctx = FlowContext::new_root();
    guard.init_flow(&ctx, "agent-a").await;
    guard.record_invocation(&ctx, "agent-b").await.unwrap();

    // agent-a is already in the call chain → cycle
    let result = guard.check(&ctx, "agent-a").await;
    // The call_chain is built by init_flow (root_agent) + record_invocations.
    // init_flow sets call_chain="agent-a", record_invocation appends "agent-b".
    // Checking against "agent-a" should detect the cycle.
    assert!(
        matches!(result, Err(FlowRejection::CycleDetected { .. })),
        "expected CycleDetected, got {:?}",
        result
    );
}

#[tokio::test]
#[ignore = "requires Redis"]
async fn guard_token_budget_exhausted() {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".into());
    let client = redis::Client::open(url).unwrap();
    let config = FlowConfig { max_flow_tokens: 100, ..FlowConfig::default() };
    let guard = FlowGuard::new(client, config);

    let ctx = FlowContext::new_root();
    guard.init_flow(&ctx, "root").await;

    // First batch: 80 tokens → ok
    guard.record_tokens(&ctx, 80).await.unwrap();
    // Second batch: 30 more → total 110 > 100 → exhausted
    let result = guard.record_tokens(&ctx, 30).await;
    assert!(
        matches!(result, Err(FlowRejection::TokenBudgetExhausted { .. })),
        "expected TokenBudgetExhausted, got {:?}",
        result
    );
}

#[tokio::test]
#[ignore = "requires Redis"]
async fn guard_record_return_decrements_depth() {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".into());
    let client = redis::Client::open(url).unwrap();
    let config = FlowConfig { max_depth: 2, ..FlowConfig::default() };
    let guard = FlowGuard::new(client, config);

    let ctx = FlowContext::new_root();
    guard.init_flow(&ctx, "root").await;

    guard.record_invocation(&ctx, "child").await.unwrap();
    // Depth is now 1. Return to decrement it back to 0.
    guard.record_return(&ctx).await;
    // After return, depth is 0 and we can invoke again without hitting limit.
    guard.record_invocation(&ctx, "child2").await.unwrap();
}