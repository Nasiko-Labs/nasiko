use nasiko_orchestrator::RouterError;

// ── Variant construction ──────────────────────────────────────────────────────

#[test]
fn no_agents_available_constructs() {
    let e = RouterError::NoAgentsAvailable;
    let msg = e.to_string();
    assert!(msg.contains("no agents available"));
}

#[test]
fn agent_not_found_constructs_with_name() {
    let e = RouterError::AgentNotFound("my-agent".to_string());
    let msg = e.to_string();
    assert!(msg.contains("my-agent"));
    assert!(msg.contains("agent not found"));
}

#[test]
fn embedding_error_constructs_with_message() {
    let e = RouterError::Embedding("OpenAI returned 429".to_string());
    let msg = e.to_string();
    assert!(msg.contains("embedding error"));
    assert!(msg.contains("OpenAI returned 429"));
}

#[test]
fn selection_error_constructs_with_message() {
    let e = RouterError::Selection("LLM timeout".to_string());
    let msg = e.to_string();
    assert!(msg.contains("selection failed"));
    assert!(msg.contains("LLM timeout"));
}

#[test]
fn internal_error_constructs_with_message() {
    let e = RouterError::Internal("unexpected state".to_string());
    let msg = e.to_string();
    assert!(msg.contains("internal error"));
    assert!(msg.contains("unexpected state"));
}

// ── Debug formatting ──────────────────────────────────────────────────────────

#[test]
fn all_variants_implement_debug() {
    let variants: Vec<RouterError> = vec![
        RouterError::NoAgentsAvailable,
        RouterError::AgentNotFound("x".to_string()),
        RouterError::Embedding("e".to_string()),
        RouterError::Selection("s".to_string()),
        RouterError::Internal("i".to_string()),
    ];
    for v in &variants {
        let s = format!("{v:?}");
        assert!(!s.is_empty());
    }
}

// ── Display formatting ────────────────────────────────────────────────────────

#[test]
fn all_variants_implement_display() {
    let variants: Vec<RouterError> = vec![
        RouterError::NoAgentsAvailable,
        RouterError::AgentNotFound("agent-42".to_string()),
        RouterError::Embedding("bad embedding".to_string()),
        RouterError::Selection("selector failed".to_string()),
        RouterError::Internal("crash".to_string()),
    ];
    for v in &variants {
        let s = v.to_string();
        assert!(
            !s.is_empty(),
            "Display should produce non-empty output for {v:?}"
        );
    }
}
