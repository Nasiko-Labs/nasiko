use nasiko_agent_proxy::{AgentEndpoint, ResolvedAgent};

// ── AgentEndpoint construction and field access ────────────────────────────

#[test]
fn agent_endpoint_host_field() {
    let ep = AgentEndpoint {
        host: "10.0.0.5".to_string(),
        port: 8080,
    };
    assert_eq!(ep.host, "10.0.0.5");
}

#[test]
fn agent_endpoint_port_field() {
    let ep = AgentEndpoint {
        host: "10.0.0.5".to_string(),
        port: 8080,
    };
    assert_eq!(ep.port, 8080);
}

#[test]
fn agent_endpoint_port_zero_is_valid() {
    let ep = AgentEndpoint {
        host: "localhost".to_string(),
        port: 0,
    };
    assert_eq!(ep.port, 0);
}

#[test]
fn agent_endpoint_port_max_is_valid() {
    let ep = AgentEndpoint {
        host: "localhost".to_string(),
        port: u16::MAX,
    };
    assert_eq!(ep.port, u16::MAX);
}

#[test]
fn agent_endpoint_empty_host() {
    let ep = AgentEndpoint {
        host: String::new(),
        port: 8000,
    };
    assert_eq!(ep.host, "");
}

// ── ResolvedAgent field access ─────────────────────────────────────────────

#[test]
fn resolved_agent_name_field() {
    let agent = ResolvedAgent {
        name: "my-agent".to_string(),
        endpoint: None,
    };
    assert_eq!(agent.name, "my-agent");
}

#[test]
fn resolved_agent_endpoint_may_be_absent() {
    // An empty `agents.url` resolves to a running agent with no stored
    // endpoint snapshot — callers fall back to a live runtime lookup.
    let agent = ResolvedAgent {
        name: "my-agent".to_string(),
        endpoint: None,
    };
    assert!(agent.endpoint.is_none());
}

#[test]
fn resolved_agent_endpoint_when_present() {
    let agent = ResolvedAgent {
        name: "my-agent".to_string(),
        endpoint: Some(AgentEndpoint {
            host: "10.0.0.5".to_string(),
            port: 8080,
        }),
    };
    let ep = agent.endpoint.expect("endpoint must be present");
    assert_eq!(ep.host, "10.0.0.5");
    assert_eq!(ep.port, 8080);
}

// ── Debug formatting ──────────────────────────────────────────────────────

#[test]
fn agent_endpoint_debug_contains_host() {
    let ep = AgentEndpoint {
        host: "myhost".to_string(),
        port: 9090,
    };
    let debug = format!("{:?}", ep);
    assert!(
        debug.contains("myhost"),
        "Debug output must contain the host: {debug}"
    );
}

#[test]
fn agent_endpoint_debug_contains_port() {
    let ep = AgentEndpoint {
        host: "myhost".to_string(),
        port: 9090,
    };
    let debug = format!("{:?}", ep);
    assert!(
        debug.contains("9090"),
        "Debug output must contain the port: {debug}"
    );
}

#[test]
fn resolved_agent_debug_contains_name() {
    let agent = ResolvedAgent {
        name: "agent-x".to_string(),
        endpoint: None,
    };
    let debug = format!("{:?}", agent);
    assert!(
        debug.contains("agent-x"),
        "Debug output must contain the name: {debug}"
    );
}

// ── Clone behaviour ────────────────────────────────────────────────────────

#[test]
fn agent_endpoint_clone_is_independent() {
    let original = AgentEndpoint {
        host: "original-host".to_string(),
        port: 1234,
    };
    let mut cloned = original.clone();
    cloned.host = "different-host".to_string();
    // Original must be unaffected
    assert_eq!(original.host, "original-host");
}

#[test]
fn resolved_agent_clone_copies_all_fields() {
    let agent = ResolvedAgent {
        name: "n".to_string(),
        endpoint: Some(AgentEndpoint {
            host: "h".to_string(),
            port: 42,
        }),
    };
    let c = agent.clone();
    assert_eq!(c.name, agent.name);
    let (ep, cep) = (agent.endpoint.unwrap(), c.endpoint.unwrap());
    assert_eq!(cep.host, ep.host);
    assert_eq!(cep.port, ep.port);
}
