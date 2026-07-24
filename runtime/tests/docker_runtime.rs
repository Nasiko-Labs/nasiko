//! Tests for DockerRuntime.
//!
//! Tests that require a live Docker daemon are tagged `#[ignore]`.
//! Run them explicitly with:
//!   cargo test --test docker_runtime -- --ignored

use std::collections::HashMap;
use std::time::Duration;

use nasiko_runtime::{
    ContainerId, DeploymentSpec, DockerRuntime, DockerRuntimeConfig, ResourceLimits,
};

// ─── DockerRuntimeConfig pure construction ────────────────────────────────────

#[test]
fn docker_runtime_config_default_values() {
    let cfg = DockerRuntimeConfig::default();
    assert_eq!(cfg.bind_host, "127.0.0.1");
    assert!(cfg.network.is_none());
    assert_eq!(cfg.operation_timeout, Duration::from_secs(30));
    assert_eq!(cfg.build_timeout, Duration::from_secs(30 * 60));
    assert!(cfg.registry_host.is_none());
}

#[test]
fn docker_runtime_config_custom_construction() {
    let cfg = DockerRuntimeConfig {
        bind_host: "0.0.0.0".to_owned(),
        network: Some("my-net".to_owned()),
        operation_timeout: Duration::from_secs(10),
        build_timeout: Duration::from_secs(600),
        registry_host: Some("localhost:5000".to_owned()),
    };
    assert_eq!(cfg.bind_host, "0.0.0.0");
    assert_eq!(cfg.network.as_deref(), Some("my-net"));
    assert_eq!(cfg.operation_timeout, Duration::from_secs(10));
    assert_eq!(cfg.registry_host.as_deref(), Some("localhost:5000"));
}

#[test]
fn docker_runtime_config_clone() {
    let cfg = DockerRuntimeConfig::default();
    let cloned = cfg.clone();
    assert_eq!(cfg.bind_host, cloned.bind_host);
    assert_eq!(cfg.operation_timeout, cloned.operation_timeout);
}

// ─── Spec validation (no daemon needed) ──────────────────────────────────────

/// Builds a valid minimal DeploymentSpec for use in validation-only tests.
fn test_spec() -> DeploymentSpec {
    DeploymentSpec {
        container_id: ContainerId::new("test-agent"),
        name: "test-agent".to_owned(),
        image: "alpine:latest".to_owned(),
        min_replicas: 1,
        max_replicas: 1,
        env_vars: HashMap::new(),
        ports: vec![8080],
        resources: Some(ResourceLimits {
            memory: "256Mi".to_owned(),
            cpu_milli: 250,
        }),
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: false,
        network_override: None,
        workload_kind: Default::default(),
    }
}

#[test]
fn spec_with_resource_limits_validates() {
    let spec = test_spec();
    assert!(spec.validate().is_ok());
}

#[test]
fn spec_resource_limits_zero_cpu_fails() {
    let mut spec = test_spec();
    spec.resources = Some(ResourceLimits {
        memory: "256Mi".to_owned(),
        cpu_milli: 0,
    });
    // validate() on DeploymentSpec calls ResourceLimits::validate() internally
    assert!(spec.validate().is_err());
}

#[test]
fn spec_resource_limits_unrecognized_memory_suffix_fails() {
    let mut spec = test_spec();
    spec.resources = Some(ResourceLimits {
        memory: "256KB".to_owned(),
        cpu_milli: 250,
    });
    assert!(spec.validate().is_err());
}

#[test]
fn spec_resource_limits_bare_bytes_is_valid() {
    let mut spec = test_spec();
    spec.resources = Some(ResourceLimits {
        memory: "536870912".to_owned(), // 512 MiB in bytes
        cpu_milli: 500,
    });
    assert!(spec.validate().is_ok());
}

#[test]
fn spec_resource_limits_gi_suffix_is_valid() {
    let mut spec = test_spec();
    spec.resources = Some(ResourceLimits {
        memory: "2Gi".to_owned(),
        cpu_milli: 2000,
    });
    assert!(spec.validate().is_ok());
}

#[test]
fn spec_multi_port_validates() {
    let mut spec = test_spec();
    spec.ports = vec![8080, 9090, 3000];
    assert!(spec.validate().is_ok());
}

#[test]
fn spec_env_vars_valid() {
    let mut spec = test_spec();
    spec.env_vars
        .insert("API_KEY".to_owned(), "secret123".to_owned());
    spec.env_vars.insert("PORT".to_owned(), "8080".to_owned());
    assert!(spec.validate().is_ok());
}

// ─── Tests requiring a live Docker daemon (marked #[ignore]) ─────────────────

/// DockerRuntime::new() connects to the Docker daemon via the Unix socket.
/// This test pings the daemon — it will fail if Docker is not running.
#[tokio::test]
#[ignore]
async fn docker_runtime_new_connects_to_daemon() {
    let cfg = DockerRuntimeConfig::default();
    let runtime = DockerRuntime::new(cfg).await;
    assert!(
        runtime.is_ok(),
        "DockerRuntime::new should succeed when Docker daemon is running"
    );
}

#[tokio::test]
#[ignore]
async fn docker_runtime_list_returns_empty_or_agents() {
    use nasiko_runtime::ContainerRuntime;

    let cfg = DockerRuntimeConfig::default();
    let runtime = DockerRuntime::new(cfg)
        .await
        .expect("Docker must be running");
    let result = runtime.list().await;
    assert!(result.is_ok(), "list() should not fail with a live daemon");
}

#[tokio::test]
#[ignore]
async fn docker_runtime_status_unknown_for_missing_agent() {
    use nasiko_runtime::{ContainerRuntime, RuntimeState};

    let cfg = DockerRuntimeConfig::default();
    let runtime = DockerRuntime::new(cfg)
        .await
        .expect("Docker must be running");
    let id = ContainerId::new("nonexistent-agent-xyz-999");
    let status = runtime
        .status(&id)
        .await
        .expect("status() should not error for missing agent");
    // Per the contract: missing container → Unknown state, not an error
    assert_eq!(status.state, RuntimeState::Unknown);
    assert_eq!(status.replicas_live, 0);
    assert!(status.endpoint.is_none());
}

#[tokio::test]
#[ignore]
async fn docker_runtime_destroy_nonexistent_is_idempotent() {
    use nasiko_runtime::ContainerRuntime;

    let cfg = DockerRuntimeConfig::default();
    let runtime = DockerRuntime::new(cfg)
        .await
        .expect("Docker must be running");
    let id = ContainerId::new("nonexistent-agent-destroy-test");
    // destroy() on a missing container must succeed (idempotent)
    assert!(runtime.destroy(&id).await.is_ok());
}

#[tokio::test]
#[ignore]
async fn docker_runtime_deploy_and_destroy_alpine() {
    use nasiko_runtime::{ContainerRuntime, RuntimeState};

    let cfg = DockerRuntimeConfig::default();
    let runtime = DockerRuntime::new(cfg)
        .await
        .expect("Docker must be running");

    let spec = DeploymentSpec {
        container_id: ContainerId::new("test-integration-alpine"),
        name: "test-integration-alpine".to_owned(),
        // alpine with a long-running command so the container stays up
        image: "alpine:latest".to_owned(),
        min_replicas: 1,
        max_replicas: 1,
        env_vars: {
            let mut m = HashMap::new();
            m.insert("TEST".to_owned(), "1".to_owned());
            m
        },
        ports: vec![9999],
        resources: None,
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: false,
        network_override: None,
        workload_kind: Default::default(),
    };

    // Deploy
    let status = runtime.deploy(&spec).await.expect("deploy should succeed");
    assert!(
        matches!(status.state, RuntimeState::Running | RuntimeState::Pending),
        "state should be Running or Pending after deploy, got: {:?}",
        status.state
    );

    // Cleanup — destroy must not fail even if the container is running
    runtime
        .destroy(&spec.container_id)
        .await
        .expect("destroy should succeed");
}

// ─── RUN-10a: deploy() must recreate the container when env vars change ──────

/// Shells out to `docker inspect` rather than adding a `bollard` dev-dependency
/// just for test assertions — mirrors what an operator would check by hand.
fn docker_container_id(name: &str) -> String {
    let out = std::process::Command::new("docker")
        .args(["inspect", "--format", "{{.Id}}", name])
        .output()
        .expect("docker inspect should run");
    String::from_utf8(out.stdout)
        .expect("utf8")
        .trim()
        .to_owned()
}

fn docker_container_env(name: &str) -> Vec<String> {
    let out = std::process::Command::new("docker")
        .args(["inspect", "--format", "{{json .Config.Env}}", name])
        .output()
        .expect("docker inspect should run");
    serde_json::from_slice(&out.stdout).unwrap_or_default()
}

#[tokio::test]
#[ignore]
async fn docker_runtime_deploy_recreates_container_when_env_changes() {
    use nasiko_runtime::ContainerRuntime;

    let cfg = DockerRuntimeConfig::default();
    let runtime = DockerRuntime::new(cfg)
        .await
        .expect("Docker must be running");
    let id = ContainerId::new("test-run10a-env-change");
    let _ = runtime.destroy(&id).await;

    let mut spec = DeploymentSpec {
        container_id: id.clone(),
        name: "test-run10a-env-change".to_owned(),
        image: "alpine:latest".to_owned(),
        min_replicas: 1,
        max_replicas: 1,
        env_vars: HashMap::from([("SECRET".to_owned(), "v1".to_owned())]),
        ports: vec![9998],
        resources: None,
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: false,
        network_override: None,
        workload_kind: Default::default(),
    };

    runtime.deploy(&spec).await.expect("initial deploy");
    let container_name = "nasiko-agent-test-run10a-env-change";
    let id_before = docker_container_id(container_name);

    // Same image tag, changed env — this used to be a silent no-op (RUN-10a).
    spec.env_vars.insert("SECRET".to_owned(), "v2".to_owned());
    runtime
        .deploy(&spec)
        .await
        .expect("redeploy with changed env");

    let id_after = docker_container_id(container_name);
    assert_ne!(
        id_before, id_after,
        "container must be recreated when env changes, even with an unchanged image tag"
    );

    let env_after = docker_container_env(container_name);
    assert!(
        env_after.contains(&"SECRET=v2".to_owned()),
        "recreated container must have the new env value, got: {env_after:?}"
    );

    runtime.destroy(&id).await.expect("cleanup");
}

#[tokio::test]
#[ignore]
async fn docker_runtime_deploy_does_not_recreate_when_unchanged() {
    // Sanity check for the RUN-10a fix: deploy() must remain a no-op (not recreate
    // the container) when neither the image nor the env vars changed.
    use nasiko_runtime::ContainerRuntime;

    let cfg = DockerRuntimeConfig::default();
    let runtime = DockerRuntime::new(cfg)
        .await
        .expect("Docker must be running");
    let id = ContainerId::new("test-run10a-no-change");
    let _ = runtime.destroy(&id).await;

    let spec = DeploymentSpec {
        container_id: id.clone(),
        name: "test-run10a-no-change".to_owned(),
        image: "alpine:latest".to_owned(),
        min_replicas: 1,
        max_replicas: 1,
        env_vars: HashMap::from([("SECRET".to_owned(), "v1".to_owned())]),
        ports: vec![9997],
        resources: None,
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: false,
        network_override: None,
        workload_kind: Default::default(),
    };

    runtime.deploy(&spec).await.expect("initial deploy");
    let container_name = "nasiko-agent-test-run10a-no-change";
    let id_before = docker_container_id(container_name);

    runtime
        .deploy(&spec)
        .await
        .expect("redeploy, unchanged spec");
    let id_after = docker_container_id(container_name);
    assert_eq!(
        id_before, id_after,
        "deploy() must not recreate the container when image and env are unchanged"
    );

    runtime.destroy(&id).await.expect("cleanup");
}
