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
    assert!(cfg.registry_username.is_none());
    assert!(cfg.registry_password.is_none());
    assert_eq!(cfg.agent_memory_volume, "nasiko-agent-memory");
    assert_eq!(cfg.agent_memory_init_image, "alpine:3.21");
}

#[test]
fn docker_runtime_config_custom_construction() {
    let cfg = DockerRuntimeConfig {
        bind_host: "0.0.0.0".to_owned(),
        network: Some("my-net".to_owned()),
        operation_timeout: Duration::from_secs(10),
        build_timeout: Duration::from_secs(600),
        registry_host: Some("localhost:5000".to_owned()),
        registry_username: Some("registry-user".to_owned()),
        registry_password: Some("registry-pass".to_owned()),
        registry_credentials_host: Some("ghcr.io".to_owned()),
        agent_memory_volume: "custom-memory-volume".to_owned(),
        agent_memory_init_image: "busybox:1.36".to_owned(),
    };
    assert_eq!(cfg.bind_host, "0.0.0.0");
    assert_eq!(cfg.network.as_deref(), Some("my-net"));
    assert_eq!(cfg.operation_timeout, Duration::from_secs(10));
    assert_eq!(cfg.registry_host.as_deref(), Some("localhost:5000"));
    assert_eq!(cfg.registry_username.as_deref(), Some("registry-user"));
    assert_eq!(cfg.registry_password.as_deref(), Some("registry-pass"));
    assert_eq!(cfg.agent_memory_volume, "custom-memory-volume");
    assert_eq!(cfg.agent_memory_init_image, "busybox:1.36");
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
        writable: false,
        writable_path: None,
        owner_id: uuid::Uuid::nil(),
        force_pull: false,
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
        writable: false,
        writable_path: None,
        owner_id: uuid::Uuid::nil(),
        force_pull: false,
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
        writable: false,
        writable_path: None,
        owner_id: uuid::Uuid::nil(),
        force_pull: false,
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
        writable: false,
        writable_path: None,
        owner_id: uuid::Uuid::nil(),
        force_pull: false,
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

// ─── writable: persistent, private-per-agent /workspace ──────────────────────

fn docker_exec(container_name: &str, args: &[&str]) -> std::process::Output {
    std::process::Command::new("docker")
        .args(["exec", container_name])
        .args(args)
        .output()
        .expect("docker exec should run")
}

#[tokio::test]
#[ignore]
async fn docker_runtime_writable_persists_across_redeploy_and_is_private_per_agent() {
    use nasiko_runtime::ContainerRuntime;

    let cfg = DockerRuntimeConfig::default();
    let runtime = DockerRuntime::new(cfg)
        .await
        .expect("Docker must be running");

    let id_a = ContainerId::new("test-writable-agent-a");
    let id_b = ContainerId::new("test-writable-agent-b");
    let _ = runtime.destroy(&id_a).await;
    let _ = runtime.destroy(&id_b).await;

    fn writable_spec(id: ContainerId, port: u16) -> DeploymentSpec {
        let name = id.as_str().to_owned();
        DeploymentSpec {
            container_id: id,
            name,
            // DeploymentSpec has no way to override the image's default CMD —
            // alpine's (`/bin/sh`, no tty) exits almost immediately once
            // created, too fast to reliably `docker exec` into. redis-alpine's
            // default CMD (`redis-server`) stays up on its own, and it's a
            // standard Alpine base, so `sh`/`cat` are on PATH the same way.
            image: "redis:7-alpine".to_owned(),
            min_replicas: 1,
            max_replicas: 1,
            env_vars: HashMap::new(),
            ports: vec![port],
            resources: None,
            image_pull_secret_name: None,
            image_pull_credential_seed: None,
            harden: false,
            network_override: None,
            workload_kind: Default::default(),
            writable: true,
            writable_path: None,
            owner_id: uuid::Uuid::nil(),
            force_pull: false,
        }
    }

    let spec_a = writable_spec(id_a.clone(), 9996);
    runtime.deploy(&spec_a).await.expect("deploy agent A");
    let container_a = "nasiko-agent-test-writable-agent-a";

    let write = docker_exec(
        container_a,
        &["sh", "-c", "echo hello-from-a > /workspace/note.txt"],
    );
    assert!(
        write.status.success(),
        "write into /workspace should succeed: {}",
        String::from_utf8_lossy(&write.stderr)
    );

    // A second agent mounting the SAME shared volume must not see A's file —
    // private by default (subPath isolation), not a shared pool.
    let spec_b = writable_spec(id_b.clone(), 9995);
    runtime.deploy(&spec_b).await.expect("deploy agent B");
    let container_b = "nasiko-agent-test-writable-agent-b";
    let b_sees_a = docker_exec(container_b, &["cat", "/workspace/note.txt"]);
    assert!(
        !b_sees_a.status.success(),
        "agent B must not see agent A's /workspace contents"
    );

    // Destroying and redeploying A must not lose what was written — the data
    // lives in the volume, not the container.
    runtime.destroy(&id_a).await.expect("destroy agent A");
    runtime.deploy(&spec_a).await.expect("redeploy agent A");
    let read_back = docker_exec(container_a, &["cat", "/workspace/note.txt"]);
    assert!(
        read_back.status.success(),
        "redeployed agent A should still have its old /workspace contents"
    );
    assert_eq!(
        String::from_utf8_lossy(&read_back.stdout).trim(),
        "hello-from-a"
    );

    runtime.destroy(&id_a).await.expect("cleanup A");
    runtime.destroy(&id_b).await.expect("cleanup B");
}

/// The workspace reader, end to end against a real daemon: the shell scripts
/// (`find -exec … +`, `stat -c`), the exec plumbing and the `--writable`
/// hardening can only be proven here — a unit test can assert the argv but not
/// that busybox accepts it.
#[tokio::test]
#[ignore]
async fn docker_runtime_reads_workspace_files_even_after_the_agent_is_gone() {
    use futures_util::StreamExt;
    use nasiko_runtime::{ContainerRuntime, WorkspaceRef};

    let runtime = DockerRuntime::new(DockerRuntimeConfig::default())
        .await
        .expect("Docker must be running");

    let id = ContainerId::new("test-workspace-reader");
    let _ = runtime.destroy(&id).await;

    let owner_id = uuid::Uuid::from_u128(0x5EED_u128);
    let spec = DeploymentSpec {
        container_id: id.clone(),
        name: id.as_str().to_owned(),
        image: "redis:7-alpine".to_owned(),
        min_replicas: 1,
        max_replicas: 1,
        env_vars: HashMap::new(),
        ports: vec![9994],
        resources: None,
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: false,
        network_override: None,
        workload_kind: Default::default(),
        writable: true,
        writable_path: None,
        owner_id,
        force_pull: false,
    };
    runtime.deploy(&spec).await.expect("deploy writable agent");
    let container = "nasiko-agent-test-workspace-reader";

    // A --writable agent keeps the image user (root here) and can write its
    // mount regardless of the subdir's ownership — read-only root confines it to
    // the mount, running as root lets it write there without a matching chown.
    let write = docker_exec(
        container,
        &[
            "sh",
            "-c",
            "mkdir -p /workspace/sub && printf 'print(1 + 1)\\n' > /workspace/sub/add_numbers.py",
        ],
    );
    assert!(
        write.status.success(),
        "a --writable agent must be able to write its mount: {}",
        String::from_utf8_lossy(&write.stderr)
    );

    // ...and nowhere else. This is the whole point of the read-only root: the
    // same write used to land on the ephemeral layer and vanish on restart.
    let stray = docker_exec(container, &["sh", "-c", "echo x > /stray.txt"]);
    assert!(
        !stray.status.success(),
        "a write outside the mount must fail, not silently hit the container layer"
    );

    let workspace = WorkspaceRef {
        owner_id,
        container_id: id.clone(),
    };

    let files = runtime
        .list_workspace(&workspace)
        .await
        .expect("list_workspace");
    assert_eq!(
        files
            .iter()
            .map(|f| (f.path.as_str(), f.size))
            .collect::<Vec<_>>(),
        vec![("sub/add_numbers.py", 13)],
        "paths must come back relative to the agent's own directory"
    );

    // Destroy the agent: the reader mounts the volume, not the container, so
    // this must not affect either call.
    runtime.destroy(&id).await.expect("destroy agent");

    let after = runtime
        .list_workspace(&workspace)
        .await
        .expect("list_workspace after destroy");
    assert_eq!(after.len(), 1, "files outlive the agent");

    let mut file = runtime
        .read_workspace_file(&workspace, "sub/add_numbers.py")
        .await
        .expect("read_workspace_file after destroy");
    assert_eq!(file.size, 13);
    let mut body = Vec::new();
    while let Some(chunk) = file.stream.next().await {
        body.extend_from_slice(&chunk.expect("stream chunk"));
    }
    assert_eq!(String::from_utf8_lossy(&body), "print(1 + 1)\n");

    // An agent that never wrote anything lists empty rather than erroring.
    let empty = runtime
        .list_workspace(&WorkspaceRef {
            owner_id,
            container_id: ContainerId::new("never-deployed"),
        })
        .await
        .expect("listing an unknown agent is empty, not an error");
    assert!(empty.is_empty());

    // Traversal is refused at the runtime too, not only at the HTTP edge.
    assert!(
        runtime
            .read_workspace_file(&workspace, "../../etc/passwd")
            .await
            .is_err()
    );
}

/// A `--writable` agent can plant a symlink pointing out of its own subtree —
/// its container has read-write access to the same bytes the read-only reader
/// mounts. Absent an in-reader containment check the reader would follow it and
/// `cat` another agent's file across the shared volume. This proves the check
/// (`readlink -f` + prefix match, run in the same shell as the `cat`) refuses
/// both a symlinked file and a symlinked intermediate directory, while a real
/// file and an in-scope relative symlink still read.
#[tokio::test]
#[ignore]
async fn docker_runtime_workspace_read_refuses_symlink_escape() {
    use futures_util::StreamExt;
    use nasiko_runtime::{ContainerRuntime, WorkspaceRef};

    let runtime = DockerRuntime::new(DockerRuntimeConfig::default())
        .await
        .expect("Docker must be running");

    let spec = |id: &ContainerId, owner: uuid::Uuid, port: u16| DeploymentSpec {
        container_id: id.clone(),
        name: id.as_str().to_owned(),
        image: "redis:7-alpine".to_owned(),
        min_replicas: 1,
        max_replicas: 1,
        env_vars: HashMap::new(),
        ports: vec![port],
        resources: None,
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: false,
        network_override: None,
        workload_kind: Default::default(),
        writable: true,
        writable_path: None,
        owner_id: owner,
        force_pull: false,
    };

    // Victim agent, holding a secret only its owner may download.
    let victim_id = ContainerId::new("test-symlink-victim");
    let victim_owner = uuid::Uuid::from_u128(0xC0FFEE_u128);
    let _ = runtime.destroy(&victim_id).await;
    runtime
        .deploy(&spec(&victim_id, victim_owner, 9995))
        .await
        .expect("deploy victim");
    let victim = WorkspaceRef {
        owner_id: victim_owner,
        container_id: victim_id.clone(),
    };
    let victim_secret_abs = format!("/data/{}/secret.txt", victim.subpath());
    let victim_dir_abs = format!("/data/{}", victim.subpath());
    let seed = docker_exec(
        "nasiko-agent-test-symlink-victim",
        &["sh", "-c", "printf 'TOP-SECRET' > /workspace/secret.txt"],
    );
    assert!(seed.status.success(), "seed victim secret");

    // Attacker agent: plants symlinks into the victim's subtree by absolute
    // path (its own container can't see `/data`, but a symlink is just a string;
    // resolution happens later, inside the whole-volume reader).
    let attacker_id = ContainerId::new("test-symlink-attacker");
    let attacker_owner = uuid::Uuid::from_u128(0xBADBAD_u128);
    let _ = runtime.destroy(&attacker_id).await;
    runtime
        .deploy(&spec(&attacker_id, attacker_owner, 9996))
        .await
        .expect("deploy attacker");
    let attacker = WorkspaceRef {
        owner_id: attacker_owner,
        container_id: attacker_id.clone(),
    };
    let plant = docker_exec(
        "nasiko-agent-test-symlink-attacker",
        &[
            "sh",
            "-c",
            &format!(
                "ln -sf {victim_secret_abs} /workspace/steal.txt; \
                 ln -sf {victim_dir_abs} /workspace/adir; \
                 printf 'mine' > /workspace/mine.txt; \
                 ln -sf mine.txt /workspace/rel_link"
            ),
        ],
    );
    assert!(plant.status.success(), "plant symlinks: {plant:?}");

    // A symlinked file pointing at another agent's data: refused.
    assert!(
        runtime
            .read_workspace_file(&attacker, "steal.txt")
            .await
            .is_err(),
        "a symlink to another agent's file must not be readable"
    );
    // A symlinked intermediate directory: also refused.
    assert!(
        runtime
            .read_workspace_file(&attacker, "adir/secret.txt")
            .await
            .is_err(),
        "a symlinked intermediate directory must not let a read escape"
    );

    // Containment, not a blanket symlink ban: the attacker's own real file and
    // an in-scope relative symlink to it both still read.
    let mut own = runtime
        .read_workspace_file(&attacker, "mine.txt")
        .await
        .expect("own real file still reads");
    let mut body = Vec::new();
    while let Some(chunk) = own.stream.next().await {
        body.extend_from_slice(&chunk.expect("chunk"));
    }
    assert_eq!(String::from_utf8_lossy(&body), "mine");

    let mut via_link = runtime
        .read_workspace_file(&attacker, "rel_link")
        .await
        .expect("in-scope relative symlink still reads");
    let mut linked = Vec::new();
    while let Some(chunk) = via_link.stream.next().await {
        linked.extend_from_slice(&chunk.expect("chunk"));
    }
    assert_eq!(String::from_utf8_lossy(&linked), "mine");

    runtime.destroy(&victim_id).await.expect("cleanup victim");
    runtime
        .destroy(&attacker_id)
        .await
        .expect("cleanup attacker");
}
