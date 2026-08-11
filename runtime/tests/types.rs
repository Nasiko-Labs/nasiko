//! Tests for nasiko-runtime pure types — no Docker daemon required.

use std::collections::HashMap;

use nasiko_runtime::{ContainerId, DeploymentSpec, DeploymentStatus, ResourceLimits, RuntimeState};

// ─── ContainerId ─────────────────────────────────────────────────────────────

#[test]
fn container_id_new_wraps_string() {
    let id = ContainerId::new("my-agent");
    assert_eq!(id.as_str(), "my-agent");
}

#[test]
fn container_id_display_matches_inner() {
    let id = ContainerId::new("agent-42");
    assert_eq!(id.to_string(), "agent-42");
}

#[test]
fn container_id_from_string_conversion() {
    let id: ContainerId = "my-agent".into();
    assert_eq!(id.as_str(), "my-agent");
}

#[test]
fn container_id_from_str_conversion() {
    let id = ContainerId::from("my-agent");
    assert_eq!(id.as_str(), "my-agent");
}

#[test]
fn container_id_equality() {
    let a = ContainerId::new("agent-abc");
    let b = ContainerId::new("agent-abc");
    assert_eq!(a, b);
}

#[test]
fn container_id_try_new_valid() {
    let valid_ids = [
        "agent",
        "agent-123",
        "agent_name",
        "A1",
        "abc123",
        // exactly 63 chars
        "a123456789012345678901234567890123456789012345678901234567890b",
    ];
    for id in &valid_ids {
        assert!(
            ContainerId::try_new(*id).is_ok(),
            "expected '{id}' to be valid"
        );
    }
}

#[test]
fn container_id_try_new_rejects_empty() {
    assert!(ContainerId::try_new("").is_err());
}

#[test]
fn container_id_try_new_rejects_too_long() {
    let long = "a".repeat(64);
    assert!(ContainerId::try_new(long).is_err());
}

#[test]
fn container_id_try_new_rejects_invalid_chars() {
    let bad_ids = ["agent@host", "my agent", "agent/name", "agent=1"];
    for id in &bad_ids {
        assert!(
            ContainerId::try_new(*id).is_err(),
            "expected '{id}' to be rejected"
        );
    }
}

#[test]
fn container_id_try_new_rejects_leading_hyphen() {
    assert!(ContainerId::try_new("-agent").is_err());
}

#[test]
fn container_id_try_new_rejects_trailing_hyphen() {
    assert!(ContainerId::try_new("agent-").is_err());
}

#[test]
fn container_id_validate_roundtrip() {
    let id = ContainerId::new("valid-agent-1");
    assert!(id.validate().is_ok());
}

#[test]
fn container_id_validate_rejects_invalid() {
    let id = ContainerId::new("bad id with spaces");
    assert!(id.validate().is_err());
}

#[test]
fn container_id_from_uuid_is_valid() {
    let uuid = uuid::Uuid::new_v4();
    let id = ContainerId::from_uuid(uuid);
    // UUID v4 string should always pass validation
    assert!(id.validate().is_ok());
    assert_eq!(id.as_str(), uuid.to_string());
}

#[test]
fn container_id_clone_equals_original() {
    let id = ContainerId::new("clone-test");
    let cloned = id.clone();
    assert_eq!(id, cloned);
}

// ─── ResourceLimits ──────────────────────────────────────────────────────────

#[test]
fn resource_limits_default_values() {
    let limits = ResourceLimits::default();
    assert_eq!(limits.memory, "512Mi");
    assert_eq!(limits.cpu_milli, 500);
}

#[test]
fn resource_limits_custom_construction() {
    let limits = ResourceLimits {
        memory: "1Gi".to_owned(),
        cpu_milli: 1000,
    };
    assert_eq!(limits.memory, "1Gi");
    assert_eq!(limits.cpu_milli, 1000);
}

#[test]
fn resource_limits_serialization_roundtrip() {
    let limits = ResourceLimits {
        memory: "256Mi".to_owned(),
        cpu_milli: 250,
    };
    let json = serde_json::to_string(&limits).unwrap();
    let back: ResourceLimits = serde_json::from_str(&json).unwrap();
    assert_eq!(back.memory, limits.memory);
    assert_eq!(back.cpu_milli, limits.cpu_milli);
}

// ─── RuntimeState ─────────────────────────────────────────────────────────────

#[test]
fn runtime_state_all_variants_accessible() {
    let states = [
        RuntimeState::Pending,
        RuntimeState::Running,
        RuntimeState::Crashed,
        RuntimeState::Failed,
        RuntimeState::Stopped,
        RuntimeState::Unknown,
    ];
    // Verify distinct values
    assert_eq!(states.len(), 6);
}

#[test]
fn runtime_state_equality() {
    assert_eq!(RuntimeState::Running, RuntimeState::Running);
    assert_ne!(RuntimeState::Running, RuntimeState::Stopped);
}

#[test]
fn runtime_state_display() {
    assert_eq!(RuntimeState::Pending.to_string(), "pending");
    assert_eq!(RuntimeState::Running.to_string(), "running");
    assert_eq!(RuntimeState::Crashed.to_string(), "crashed");
    assert_eq!(RuntimeState::Failed.to_string(), "failed");
    assert_eq!(RuntimeState::Stopped.to_string(), "stopped");
    assert_eq!(RuntimeState::Unknown.to_string(), "unknown");
}

#[test]
fn runtime_state_serialization_roundtrip() {
    let state = RuntimeState::Running;
    let json = serde_json::to_string(&state).unwrap();
    assert_eq!(json, "\"running\"");
    let back: RuntimeState = serde_json::from_str(&json).unwrap();
    assert_eq!(back, RuntimeState::Running);
}

#[test]
fn runtime_state_copy() {
    let a = RuntimeState::Failed;
    let b = a; // Copy
    assert_eq!(a, b);
}

// ─── DeploymentSpec ──────────────────────────────────────────────────────────

fn minimal_spec() -> DeploymentSpec {
    DeploymentSpec {
        container_id: ContainerId::new("test-agent"),
        name: "test-agent".to_owned(),
        image: "my-registry/agent:latest".to_owned(),
        min_replicas: 1,
        max_replicas: 3,
        env_vars: HashMap::new(),
        ports: vec![8080],
        resources: None,
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: false,
        network_override: None,
        workload_kind: Default::default(),
        writable: false,
        owner_id: uuid::Uuid::nil(),
    }
}

#[test]
fn deployment_spec_minimal_construction() {
    let spec = minimal_spec();
    assert_eq!(spec.container_id.as_str(), "test-agent");
    assert_eq!(spec.name, "test-agent");
    assert_eq!(spec.image, "my-registry/agent:latest");
    assert_eq!(spec.min_replicas, 1);
    assert_eq!(spec.max_replicas, 3);
    assert!(spec.env_vars.is_empty());
    assert_eq!(spec.ports, vec![8080]);
    assert!(spec.resources.is_none());
}

#[test]
fn deployment_spec_with_all_fields() {
    let mut env_vars = HashMap::new();
    env_vars.insert("FOO".to_owned(), "bar".to_owned());
    env_vars.insert("PORT".to_owned(), "8080".to_owned());

    let spec = DeploymentSpec {
        container_id: ContainerId::new("full-agent"),
        name: "full-agent".to_owned(),
        image: "harbor.example.io/agents/full-agent:v1.2.3".to_owned(),
        min_replicas: 2,
        max_replicas: 5,
        env_vars,
        ports: vec![8080, 9090],
        resources: Some(ResourceLimits {
            memory: "1Gi".to_owned(),
            cpu_milli: 1000,
        }),
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: false,
        network_override: None,
        workload_kind: Default::default(),
        writable: true,
        owner_id: uuid::Uuid::nil(),
    };

    assert_eq!(spec.min_replicas, 2);
    assert_eq!(spec.max_replicas, 5);
    assert_eq!(spec.ports.len(), 2);
    assert_eq!(spec.env_vars.get("FOO").map(|s| s.as_str()), Some("bar"));
    assert!(spec.resources.is_some());
    assert!(spec.writable);
}

#[test]
fn deployment_spec_validate_minimal_passes() {
    assert!(minimal_spec().validate().is_ok());
}

#[test]
fn deployment_spec_validate_empty_image_fails() {
    let mut spec = minimal_spec();
    spec.image = String::new();
    assert!(spec.validate().is_err());
}

#[test]
fn deployment_spec_validate_empty_ports_fails() {
    let mut spec = minimal_spec();
    spec.ports = vec![];
    assert!(spec.validate().is_err());
}

#[test]
fn deployment_spec_validate_port_zero_fails() {
    let mut spec = minimal_spec();
    spec.ports = vec![0];
    assert!(spec.validate().is_err());
}

#[test]
fn deployment_spec_validate_min_replicas_zero_fails() {
    let mut spec = minimal_spec();
    spec.min_replicas = 0;
    assert!(spec.validate().is_err());
}

#[test]
fn deployment_spec_validate_min_exceeds_max_fails() {
    let mut spec = minimal_spec();
    spec.min_replicas = 5;
    spec.max_replicas = 3;
    assert!(spec.validate().is_err());
}

#[test]
fn deployment_spec_validate_empty_name_fails() {
    let mut spec = minimal_spec();
    spec.name = String::new();
    assert!(spec.validate().is_err());
}

#[test]
fn deployment_spec_validate_name_too_long_fails() {
    let mut spec = minimal_spec();
    spec.name = "a".repeat(64);
    assert!(spec.validate().is_err());
}

#[test]
fn deployment_spec_validate_name_invalid_chars_fails() {
    let mut spec = minimal_spec();
    spec.name = "-bad-name".to_owned();
    assert!(spec.validate().is_err());
}

#[test]
fn deployment_spec_validate_invalid_env_key_fails() {
    let mut spec = minimal_spec();
    spec.env_vars
        .insert("KEY=WITH=EQUALS".to_owned(), "val".to_owned());
    assert!(spec.validate().is_err());
}

#[test]
fn deployment_spec_validate_env_value_with_control_char_fails() {
    let mut spec = minimal_spec();
    spec.env_vars
        .insert("KEY".to_owned(), "val\x01ue".to_owned());
    assert!(spec.validate().is_err());
}

#[test]
fn deployment_spec_serialization_roundtrip() {
    let spec = minimal_spec();
    let json = serde_json::to_string(&spec).unwrap();
    let back: DeploymentSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(back.container_id, spec.container_id);
    assert_eq!(back.image, spec.image);
    assert_eq!(back.ports, spec.ports);
}

// ─── DeploymentStatus ────────────────────────────────────────────────────────

#[test]
fn deployment_status_construction() {
    let status = DeploymentStatus {
        container_id: ContainerId::new("agent-1"),
        state: RuntimeState::Running,
        replicas_live: 1,
        endpoint: Some("http://localhost:8080".to_owned()),
        message: None,
        restart_count: 0,
    };
    assert_eq!(status.state, RuntimeState::Running);
    assert_eq!(status.replicas_live, 1);
    assert_eq!(status.endpoint.as_deref(), Some("http://localhost:8080"));
    assert!(status.message.is_none());
}

#[test]
fn deployment_status_stopped_construction() {
    let status = DeploymentStatus {
        container_id: ContainerId::new("agent-2"),
        state: RuntimeState::Stopped,
        replicas_live: 0,
        endpoint: None,
        message: Some("intentionally stopped".to_owned()),
        restart_count: 0,
    };
    assert_eq!(status.state, RuntimeState::Stopped);
    assert_eq!(status.replicas_live, 0);
    assert!(status.endpoint.is_none());
    assert_eq!(status.message.as_deref(), Some("intentionally stopped"));
}

#[test]
fn deployment_status_serialization_roundtrip() {
    let status = DeploymentStatus {
        container_id: ContainerId::new("ser-agent"),
        state: RuntimeState::Crashed,
        replicas_live: 0,
        endpoint: None,
        message: Some("OOM killed".to_owned()),
        restart_count: 3,
    };
    let json = serde_json::to_string(&status).unwrap();
    let back: DeploymentStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(back.state, RuntimeState::Crashed);
    assert_eq!(back.restart_count, 3);
    assert_eq!(back.message.as_deref(), Some("OOM killed"));
}

// ─── validate_build_inputs ────────────────────────────────────────────────────

#[test]
fn validate_build_inputs_valid_tar_and_tag() {
    use nasiko_runtime::validate_build_inputs;
    assert!(validate_build_inputs(b"notempty", "registry/agent:v1.0.0").is_ok());
}

#[test]
fn validate_build_inputs_empty_tar_fails() {
    use nasiko_runtime::validate_build_inputs;
    assert!(validate_build_inputs(b"", "registry/agent:v1").is_err());
}

#[test]
fn validate_build_inputs_empty_tag_fails() {
    use nasiko_runtime::validate_build_inputs;
    assert!(validate_build_inputs(b"data", "").is_err());
}

#[test]
fn validate_build_inputs_invalid_tag_chars_fail() {
    use nasiko_runtime::validate_build_inputs;
    // Space and special chars not in [A-Za-z0-9._-/:@] are rejected
    assert!(validate_build_inputs(b"data", "agent with spaces").is_err());
    assert!(validate_build_inputs(b"data", "agent;rm -rf").is_err());
}

#[test]
fn validate_build_inputs_valid_digest_tag() {
    use nasiko_runtime::validate_build_inputs;
    // sha256 digest reference is valid
    assert!(validate_build_inputs(b"data", "registry/agent@sha256:abc123def456").is_ok());
}

// ─── WorkloadKind ──────────────────────────────────────────────────────────

#[test]
fn workload_kind_default_is_agent() {
    use nasiko_runtime::WorkloadKind;
    assert_eq!(
        WorkloadKind::default(),
        WorkloadKind::Agent,
        "default must be Agent for backward compatibility with existing callers"
    );
}
