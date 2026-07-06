use nasiko_agent_proxy::AgentEndpoint;

// ── AgentEndpoint construction and field access ────────────────────────────

#[test]
fn agent_endpoint_host_field() {
    let ep = AgentEndpoint {
        host: "10.0.0.5".to_string(),
        port: 8080,
        name: "my-agent".to_string(),
    };
    assert_eq!(ep.host, "10.0.0.5");
}

#[test]
fn agent_endpoint_port_field() {
    let ep = AgentEndpoint {
        host: "10.0.0.5".to_string(),
        port: 8080,
        name: "my-agent".to_string(),
    };
    assert_eq!(ep.port, 8080);
}

#[test]
fn agent_endpoint_name_field() {
    let ep = AgentEndpoint {
        host: "10.0.0.5".to_string(),
        port: 8080,
        name: "my-agent".to_string(),
    };
    assert_eq!(ep.name, "my-agent");
}

#[test]
fn agent_endpoint_port_zero_is_valid() {
    let ep = AgentEndpoint {
        host: "localhost".to_string(),
        port: 0,
        name: "test".to_string(),
    };
    assert_eq!(ep.port, 0);
}

#[test]
fn agent_endpoint_port_max_is_valid() {
    let ep = AgentEndpoint {
        host: "localhost".to_string(),
        port: u16::MAX,
        name: "test".to_string(),
    };
    assert_eq!(ep.port, u16::MAX);
}

#[test]
fn agent_endpoint_empty_host() {
    let ep = AgentEndpoint {
        host: String::new(),
        port: 8000,
        name: "test".to_string(),
    };
    assert_eq!(ep.host, "");
}

// ── Debug formatting ──────────────────────────────────────────────────────

#[test]
fn agent_endpoint_debug_contains_host() {
    let ep = AgentEndpoint {
        host: "myhost".to_string(),
        port: 9090,
        name: "agent-x".to_string(),
    };
    let debug = format!("{:?}", ep);
    assert!(debug.contains("myhost"), "Debug output must contain the host: {debug}");
}

#[test]
fn agent_endpoint_debug_contains_port() {
    let ep = AgentEndpoint {
        host: "myhost".to_string(),
        port: 9090,
        name: "agent-x".to_string(),
    };
    let debug = format!("{:?}", ep);
    assert!(debug.contains("9090"), "Debug output must contain the port: {debug}");
}

#[test]
fn agent_endpoint_debug_contains_name() {
    let ep = AgentEndpoint {
        host: "myhost".to_string(),
        port: 9090,
        name: "agent-x".to_string(),
    };
    let debug = format!("{:?}", ep);
    assert!(debug.contains("agent-x"), "Debug output must contain the name: {debug}");
}

// ── Clone behaviour ────────────────────────────────────────────────────────

#[test]
fn agent_endpoint_clone_is_independent() {
    let original = AgentEndpoint {
        host: "original-host".to_string(),
        port: 1234,
        name: "original-name".to_string(),
    };
    let mut cloned = original.clone();
    cloned.host = "different-host".to_string();
    // Original must be unaffected
    assert_eq!(original.host, "original-host");
}

#[test]
fn agent_endpoint_clone_copies_all_fields() {
    let ep = AgentEndpoint {
        host: "h".to_string(),
        port: 42,
        name: "n".to_string(),
    };
    let c = ep.clone();
    assert_eq!(c.host, ep.host);
    assert_eq!(c.port, ep.port);
    assert_eq!(c.name, ep.name);
}