use nasiko_flow::{
    CircuitBreakerTrip, FlowGuardConfig, GracefulDegradation, StatefulFlowGuard,
};

#[test]
fn test_normal_agent_flow_succeeds() {
    let mut guard = StatefulFlowGuard::new(FlowGuardConfig::default());
    let trace_id = "trace-normal-user-session";

    assert!(guard
        .inspect_and_record(trace_id, "agent_router", "sql_agent", "list_tables", 100, 0.01)
        .is_ok());
    assert!(guard
        .inspect_and_record(trace_id, "sql_agent", "code_agent", "read_schema", 150, 0.01)
        .is_ok());
    assert!(guard
        .inspect_and_record(trace_id, "code_agent", "qa_agent", "run_tests", 200, 0.02)
        .is_ok());
}

#[test]
fn test_max_depth_circuit_breaker_trips() {
    let config = FlowGuardConfig {
        max_depth: 3,
        ..Default::default()
    };
    let mut guard = StatefulFlowGuard::new(config);
    let trace_id = "trace-deep-recursion";

    assert!(guard.inspect_and_record(trace_id, "A", "B", "tool_1", 50, 0.01).is_ok());
    assert!(guard.inspect_and_record(trace_id, "B", "C", "tool_2", 50, 0.01).is_ok());
    assert!(guard.inspect_and_record(trace_id, "C", "D", "tool_3", 50, 0.01).is_ok());

    let trip = guard.inspect_and_record(trace_id, "D", "E", "tool_4", 50, 0.01);
    assert_eq!(
        trip,
        Err(CircuitBreakerTrip::MaxDepthExceeded {
            depth: 4,
            max: 3,
        })
    );
}

#[test]
fn test_anomalous_cycle_trips_circuit_breaker() {
    let config = FlowGuardConfig {
        max_depth: 10,
        cycle_repetition_threshold: 3,
        ..Default::default()
    };
    let mut guard = StatefulFlowGuard::new(config);
    let trace_id = "trace-ping-pong-loop";

    // Cycle pattern: [run_migration, request_reauth] repeated 3 times
    assert!(guard.inspect_and_record(trace_id, "A", "B", "run_migration", 100, 0.02).is_ok());
    assert!(guard.inspect_and_record(trace_id, "B", "A", "request_reauth", 100, 0.02).is_ok());

    assert!(guard.inspect_and_record(trace_id, "A", "B", "run_migration", 100, 0.02).is_ok());
    assert!(guard.inspect_and_record(trace_id, "B", "A", "request_reauth", 100, 0.02).is_ok());

    assert!(guard.inspect_and_record(trace_id, "A", "B", "run_migration", 100, 0.02).is_ok());
    let trip = guard.inspect_and_record(trace_id, "B", "A", "request_reauth", 100, 0.02);

    assert!(matches!(
        trip,
        Err(CircuitBreakerTrip::AnomalousCycleDetected { .. })
    ));
}

#[test]
fn test_budget_cap_trips_circuit_breaker() {
    let config = FlowGuardConfig {
        max_budget_usd: 0.50,
        ..Default::default()
    };
    let mut guard = StatefulFlowGuard::new(config);
    let trace_id = "trace-budget-spike";

    assert!(guard.inspect_and_record(trace_id, "A", "B", "analyze", 1000, 0.30).is_ok());

    let trip = guard.inspect_and_record(trace_id, "B", "C", "deep_search", 2000, 0.25);
    assert!(matches!(
        trip,
        Err(CircuitBreakerTrip::BudgetExceeded { .. })
    ));
}

#[test]
fn test_graceful_degradation_synthesizes_valid_openai_payload() {
    let trip = CircuitBreakerTrip::AnomalousCycleDetected {
        pattern: vec!["run_migration".to_string(), "request_reauth".to_string()],
        repetitions: 3,
    };

    let payload = GracefulDegradation::synthesize_hitl_response(
        &trip,
        "tr-a2a-8f92",
        "gpt-4o",
    );

    assert_eq!(payload["object"], "chat.completion");
    assert_eq!(payload["choices"][0]["finish_reason"], "stop");

    let message_content = payload["choices"][0]["message"]["content"]
        .as_str()
        .expect("content should be string");

    assert!(message_content.contains("FLOW-GUARD INTERVENTION"));
    assert!(message_content.contains("Operator intervention required"));
}
