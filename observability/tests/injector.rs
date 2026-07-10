//! Tests for OtelInjector — no external services required.

use std::collections::HashMap;

use nasiko_observability::injector::{AgentContext, InstrumentationInjector, OtelInjector};

const TEST_ENDPOINT: &str = "http://otel-collector.nasiko-infra:4318";

fn make_ctx(agent_id: &str) -> AgentContext {
    AgentContext {
        agent_id: agent_id.to_owned(),
        tenant_id: None,
        version: None,
        capture_content: false,
        otel_collector_endpoint: TEST_ENDPOINT.to_owned(),
        otel_protocol: "grpc".to_owned(),
    }
}

fn inject(ctx: AgentContext) -> HashMap<String, String> {
    let mut env_vars = HashMap::new();
    OtelInjector.inject(&mut env_vars, &ctx);
    env_vars
}

// ─── All 7 keys are present ───────────────────────────────────────────────────

#[test]
fn inject_adds_all_seven_otel_env_vars() {
    let env = inject(make_ctx("my-agent"));

    let expected_keys = [
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_PROTOCOL",
        "OTEL_SERVICE_NAME",
        "OTEL_RESOURCE_ATTRIBUTES",
        "OTEL_TRACES_EXPORTER",
        "OTEL_LOGS_EXPORTER",
        "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT",
    ];

    for key in &expected_keys {
        assert!(
            env.contains_key(*key),
            "missing key: {key}"
        );
    }
    assert_eq!(env.len(), 7, "expected exactly 7 OTEL_ env vars");
}

// ─── Individual key values ────────────────────────────────────────────────────

#[test]
fn inject_sets_correct_otlp_endpoint() {
    let env = inject(make_ctx("my-agent"));
    assert_eq!(
        env["OTEL_EXPORTER_OTLP_ENDPOINT"],
        TEST_ENDPOINT
    );
}

#[test]
fn inject_sets_otlp_protocol_from_context() {
    let env = inject(make_ctx("my-agent"));
    assert_eq!(env["OTEL_EXPORTER_OTLP_PROTOCOL"], "grpc");
}

#[test]
fn inject_sets_service_name_to_agent_id() {
    let env = inject(make_ctx("coding-agent"));
    assert_eq!(env["OTEL_SERVICE_NAME"], "coding-agent");
}

#[test]
fn inject_sets_traces_exporter_to_otlp() {
    let env = inject(make_ctx("my-agent"));
    assert_eq!(env["OTEL_TRACES_EXPORTER"], "otlp");
}

#[test]
fn inject_sets_logs_exporter_to_otlp() {
    let env = inject(make_ctx("my-agent"));
    assert_eq!(env["OTEL_LOGS_EXPORTER"], "otlp");
}

#[test]
fn inject_sets_capture_content_false_by_default() {
    let env = inject(make_ctx("my-agent"));
    assert_eq!(
        env["OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT"],
        "false"
    );
}

#[test]
fn inject_sets_capture_content_true_when_requested() {
    let ctx = AgentContext {
        capture_content: true,
        ..make_ctx("my-agent")
    };
    let env = inject(ctx);
    assert_eq!(
        env["OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT"],
        "true"
    );
}

// ─── OTEL_RESOURCE_ATTRIBUTES format ─────────────────────────────────────────

#[test]
fn inject_resource_attrs_contains_agent_id() {
    let env = inject(make_ctx("my-agent"));
    let attrs = &env["OTEL_RESOURCE_ATTRIBUTES"];
    assert!(
        attrs.contains("agent.id=my-agent"),
        "OTEL_RESOURCE_ATTRIBUTES should contain agent.id; got: {attrs}"
    );
}

#[test]
fn inject_resource_attrs_with_all_optional_fields() {
    let ctx = AgentContext {
        agent_id: "full-agent".to_owned(),
        tenant_id: Some("tenant-42".to_owned()),
        version: Some("v2.1.0".to_owned()),
        capture_content: false,
        otel_collector_endpoint: TEST_ENDPOINT.to_owned(),
        otel_protocol: "grpc".to_owned(),
    };
    let env = inject(ctx);
    let attrs = &env["OTEL_RESOURCE_ATTRIBUTES"];

    assert!(
        attrs.contains("agent.id=full-agent"),
        "should contain agent.id; got: {attrs}"
    );
    assert!(
        attrs.contains("tenant.id=tenant-42"),
        "should contain tenant.id; got: {attrs}"
    );
    assert!(
        attrs.contains("service.version=v2.1.0"),
        "should contain service.version; got: {attrs}"
    );
}

#[test]
fn inject_resource_attrs_tenant_id_only() {
    let ctx = AgentContext {
        agent_id: "agent-x".to_owned(),
        tenant_id: Some("acme-corp".to_owned()),
        version: None,
        capture_content: false,
        otel_collector_endpoint: TEST_ENDPOINT.to_owned(),
        otel_protocol: "grpc".to_owned(),
    };
    let env = inject(ctx);
    let attrs = &env["OTEL_RESOURCE_ATTRIBUTES"];

    assert!(attrs.contains("tenant.id=acme-corp"), "got: {attrs}");
    assert!(!attrs.contains("service.version"), "version should be absent; got: {attrs}");
}

#[test]
fn inject_resource_attrs_version_only() {
    let ctx = AgentContext {
        agent_id: "versioned-agent".to_owned(),
        tenant_id: None,
        version: Some("1.0.0".to_owned()),
        capture_content: false,
        otel_collector_endpoint: TEST_ENDPOINT.to_owned(),
        otel_protocol: "grpc".to_owned(),
    };
    let env = inject(ctx);
    let attrs = &env["OTEL_RESOURCE_ATTRIBUTES"];

    assert!(attrs.contains("service.version=1.0.0"), "got: {attrs}");
    assert!(!attrs.contains("tenant.id"), "tenant.id should be absent; got: {attrs}");
}

#[test]
fn inject_resource_attrs_minimal_has_only_agent_id() {
    let env = inject(make_ctx("bare-agent"));
    let attrs = &env["OTEL_RESOURCE_ATTRIBUTES"];

    // Should be exactly "agent.id=bare-agent" with no trailing comma
    assert_eq!(attrs, "agent.id=bare-agent");
}

// ─── inject is additive (does not clear existing env vars) ───────────────────

#[test]
fn inject_preserves_existing_env_vars() {
    let mut env_vars = HashMap::new();
    env_vars.insert("MY_APP_KEY".to_owned(), "my-value".to_owned());
    env_vars.insert("DATABASE_URL".to_owned(), "postgres://localhost/db".to_owned());

    OtelInjector.inject(&mut env_vars, &make_ctx("my-agent"));

    // Existing vars still present
    assert_eq!(env_vars.get("MY_APP_KEY").map(|s| s.as_str()), Some("my-value"));
    assert_eq!(
        env_vars.get("DATABASE_URL").map(|s| s.as_str()),
        Some("postgres://localhost/db")
    );
    // Plus all 7 OTEL vars
    assert!(env_vars.contains_key("OTEL_SERVICE_NAME"));
}

// ─── inject respects custom collector endpoint ────────────────────────────────

#[test]
fn inject_uses_custom_otel_collector_endpoint() {
    let ctx = AgentContext {
        agent_id: "agent".to_owned(),
        tenant_id: None,
        version: None,
        capture_content: false,
        otel_collector_endpoint: "http://custom-collector:4318".to_owned(),
        otel_protocol: "grpc".to_owned(),
    };
    let env = inject(ctx);
    assert_eq!(
        env["OTEL_EXPORTER_OTLP_ENDPOINT"],
        "http://custom-collector:4318"
    );
}

// ─── patch_dockerfile_for_otel ────────────────────────────────────────────────

mod dockerfile_patch {
    use nasiko_observability::injector::patch_dockerfile_for_otel;

    #[test]
    fn idempotent_if_already_patched() {
        let dockerfile = "FROM python:3.11\nRUN pip install myapp\nCMD python main.py";
        let patched = patch_dockerfile_for_otel(dockerfile);
        assert!(patched.contains(".nasiko_otel_patch.py"), "first pass should inject patch");
        let double_patched = patch_dockerfile_for_otel(&patched);
        assert_eq!(patched, double_patched, "second pass should be a no-op");
    }

    #[test]
    fn non_python_dockerfile_unchanged() {
        let dockerfile = "FROM node:20\nCMD [\"node\", \"server.js\"]";
        let result = patch_dockerfile_for_otel(dockerfile);
        // No python detected — should be returned unchanged
        assert_eq!(result, dockerfile);
    }

    #[test]
    fn python_dockerfile_gets_otel_install_line() {
        let dockerfile = "FROM python:3.11\nRUN pip install myapp\nCMD python main.py";
        let result = patch_dockerfile_for_otel(dockerfile);
        assert!(
            result.contains("opentelemetry-distro"),
            "patched Dockerfile should include opentelemetry-distro install"
        );
    }

    #[test]
    fn python_dockerfile_installs_sitecustomize() {
        let dockerfile = "FROM python:3.11\nRUN pip install myapp\nCMD python main.py";
        let result = patch_dockerfile_for_otel(dockerfile);
        assert!(
            result.contains(".nasiko_otel_patch.py"),
            "patched Dockerfile should copy .nasiko_otel_patch.py"
        );
        assert!(
            result.contains("sitecustomize.py"),
            "patched Dockerfile should install sitecustomize.py"
        );
    }

    #[test]
    fn python_dockerfile_cmd_not_wrapped_with_instrument() {
        // We intentionally do NOT wrap CMD with opentelemetry-instrument because
        // it prepends its own sitecustomize.py to PYTHONPATH, which would shadow
        // ours and prevent the AgentExecutor session.id patch from running.
        // Our sitecustomize.py calls initialize() directly instead.
        let dockerfile = "FROM python:3.11\nRUN pip install myapp\nCMD python main.py";
        let result = patch_dockerfile_for_otel(dockerfile);
        assert!(
            result.contains("CMD python main.py"),
            "CMD should be left unchanged (no opentelemetry-instrument wrapper)"
        );
    }

    #[test]
    fn python_dockerfile_exec_form_cmd_unchanged() {
        let dockerfile = "FROM python:3.11\nRUN pip install myapp\nCMD [\"python\", \"main.py\"]";
        let result = patch_dockerfile_for_otel(dockerfile);
        assert!(
            result.contains("CMD [\"python\", \"main.py\"]"),
            "exec-form CMD should be left unchanged"
        );
    }
}