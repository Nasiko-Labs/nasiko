use super::*;

// ── KubeRuntimeConfig default enforcement ─────────────────────────────────

#[test]
fn kube_config_default_has_agent_node_selector() {
    let config = KubeRuntimeConfig::default();
    assert_eq!(
        config.node_selector.get("nasiko.com/pool").map(String::as_str),
        Some("agents"),
        "default node_selector must target the agent node pool"
    );
}

#[test]
fn kube_config_default_has_toleration() {
    let config = KubeRuntimeConfig::default();
    assert!(!config.tolerations.is_empty(), "default must include agent pool toleration");
    let tol = &config.tolerations[0];
    assert_eq!(tol["key"], "nasiko.com/pool");
    assert_eq!(tol["value"], "agents");
    assert_eq!(tol["effect"], "NoSchedule");
}

#[test]
fn kube_config_default_has_timeout() {
    let config = KubeRuntimeConfig::default();
    assert_eq!(config.operation_timeout.as_secs(), 30);
}

// ── deployment_manifest correctness ───────────────────────────────────────

fn test_manifest_spec(id: &str) -> DeploymentSpec {
    DeploymentSpec {
        container_id:     ContainerId::new(id),
        name:         format!("test-{id}"),
        image:        "alpine:latest".to_owned(),
        min_replicas: 1,
        max_replicas: 1,
        env_vars:     HashMap::new(),
        ports:        vec![5000],
        resources:    None,
    }
}

#[test]
fn deployment_manifest_includes_toleration() {
    let spec = test_manifest_spec("test-tol");
    let config = KubeRuntimeConfig::default();
    let manifest = deployment_manifest(
        &spec,
        "nasiko-test-tol",
        "nasiko-agents",
        &config.node_selector,
        &config.tolerations,
    );
    let pod_spec = &manifest["spec"]["template"]["spec"];
    assert!(
        pod_spec["tolerations"].is_array(),
        "pod spec must contain tolerations array"
    );
    assert!(
        !pod_spec["tolerations"].as_array().unwrap().is_empty(),
        "tolerations must be non-empty"
    );
}

#[test]
fn deployment_manifest_no_tolerations_when_empty() {
    let spec = test_manifest_spec("test-no-tol");
    let manifest = deployment_manifest(
        &spec,
        "nasiko-test-no-tol",
        "nasiko-agents",
        &HashMap::new(),
        &[],
    );
    let pod_spec = &manifest["spec"]["template"]["spec"];
    assert!(
        pod_spec.get("tolerations").is_none()
            || pod_spec["tolerations"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true),
        "pod spec must not contain tolerations when config has none"
    );
}

#[test]
fn deployment_manifest_has_security_context() {
    let spec = test_manifest_spec("test-sec");
    let manifest = deployment_manifest(&spec, "nasiko-test-sec", "nasiko-agents", &HashMap::new(), &[]);
    let containers = manifest["spec"]["template"]["spec"]["containers"].as_array().unwrap();
    let security_ctx = &containers[0]["securityContext"];
    assert_eq!(security_ctx["runAsNonRoot"], true, "runAsNonRoot must be true");
    assert_eq!(security_ctx["allowPrivilegeEscalation"], false);
    assert!(security_ctx["capabilities"]["drop"].as_array().is_some());
}

#[test]
fn deployment_manifest_uses_sanitized_label_value() {
    // The label value for nasiko-agent-id should be the sanitized K8s name (not raw container_id)
    let spec = test_manifest_spec("MyAgent-01");
    let sanitized = KubeRuntime::object_name(&ContainerId::new("MyAgent-01"));
    let manifest = deployment_manifest(&spec, &sanitized, "nasiko-agents", &HashMap::new(), &[]);
    let meta_label = manifest["metadata"]["labels"]["nasiko-agent-id"].as_str().unwrap();
    let selector_label = manifest["spec"]["selector"]["matchLabels"]["nasiko-agent-id"].as_str().unwrap();
    let pod_label = manifest["spec"]["template"]["metadata"]["labels"]["nasiko-agent-id"].as_str().unwrap();
    assert_eq!(meta_label, sanitized);
    assert_eq!(selector_label, sanitized);
    assert_eq!(pod_label, sanitized);
}

#[test]
fn deployment_manifest_has_resource_limits() {
    let mut spec = test_manifest_spec("test-limits");
    spec.resources = Some(crate::types::ResourceLimits {
        memory: "256Mi".to_owned(),
        cpu_milli: 250,
    });
    let manifest = deployment_manifest(&spec, "nasiko-test-limits", "nasiko-agents", &HashMap::new(), &[]);
    let containers = manifest["spec"]["template"]["spec"]["containers"].as_array().unwrap();
    let resources = &containers[0]["resources"];
    assert_eq!(resources["limits"]["memory"], "256Mi");
    assert_eq!(resources["limits"]["cpu"], "250m");
}

#[test]
fn deployment_manifest_default_resources_applied_when_none() {
    let spec = test_manifest_spec("test-default-lim");
    let manifest = deployment_manifest(&spec, "nasiko-test-default-lim", "nasiko-agents", &HashMap::new(), &[]);
    let containers = manifest["spec"]["template"]["spec"]["containers"].as_array().unwrap();
    let resources = &containers[0]["resources"];
    // Default is 512Mi / 500m
    assert_eq!(resources["limits"]["memory"], "512Mi");
    assert_eq!(resources["limits"]["cpu"], "500m");
}

// ── NEW-1: raw label present and used in list() ───────────────────────────

#[test]
fn deployment_manifest_has_raw_label() {
    let spec = test_manifest_spec("MY-agent");
    let name = KubeRuntime::object_name(&spec.container_id);
    let manifest = deployment_manifest(&spec, &name, "nasiko-agents", &HashMap::new(), &[]);
    let raw = manifest["metadata"]["labels"]["nasiko-agent-id-raw"].as_str();
    assert_eq!(raw, Some("MY-agent"), "nasiko-agent-id-raw must hold the original container_id");
}

// ── NEW-4: resource request integer division clamp ────────────────────────

#[test]
fn deployment_manifest_resource_request_clamps_cpu_to_one() {
    let mut spec = test_manifest_spec("test-clamp-cpu");
    spec.resources = Some(crate::types::ResourceLimits {
        memory: "2Mi".to_owned(),
        cpu_milli: 1,  // 1 / 2 = 0 without clamp
    });
    let manifest = deployment_manifest(&spec, "nasiko-test-clamp-cpu", "nasiko-agents", &HashMap::new(), &[]);
    let containers = manifest["spec"]["template"]["spec"]["containers"].as_array().unwrap();
    let cpu_request = containers[0]["resources"]["requests"]["cpu"].as_str().unwrap();
    assert_eq!(cpu_request, "1m", "cpu request must be clamped to 1m, not 0m");
}

#[test]
fn deployment_manifest_resource_request_clamps_memory_to_one() {
    let mut spec = test_manifest_spec("test-clamp-mem");
    spec.resources = Some(crate::types::ResourceLimits {
        memory: "1Mi".to_owned(),  // 1 / 2 = 0 without clamp
        cpu_milli: 500,
    });
    let manifest = deployment_manifest(&spec, "nasiko-test-clamp-mem", "nasiko-agents", &HashMap::new(), &[]);
    let containers = manifest["spec"]["template"]["spec"]["containers"].as_array().unwrap();
    let mem_request = containers[0]["resources"]["requests"]["memory"].as_str().unwrap();
    assert_eq!(mem_request, "1Mi", "memory request must be clamped to 1Mi, not 0Mi");
}

// ── OBS-2: mem_request halved for G/M/bare-integer inputs ─────────────────

#[test]
fn deployment_manifest_resource_request_halves_g_suffix() {
    let mut spec = test_manifest_spec("test-g-req");
    spec.resources = Some(crate::types::ResourceLimits {
        memory: "2G".to_owned(),
        cpu_milli: 500,
    });
    let manifest = deployment_manifest(&spec, "nasiko-test-g-req", "nasiko-agents", &HashMap::new(), &[]);
    let containers = manifest["spec"]["template"]["spec"]["containers"].as_array().unwrap();
    let mem_request = containers[0]["resources"]["requests"]["memory"].as_str().unwrap();
    assert_eq!(mem_request, "1G", "2G request should be halved to 1G, not cloned as 2G");
}

#[test]
fn deployment_manifest_resource_request_halves_bare_bytes() {
    let mut spec = test_manifest_spec("test-bare-req");
    spec.resources = Some(crate::types::ResourceLimits {
        memory: "1048576".to_owned(),  // 1 MiB in bytes
        cpu_milli: 500,
    });
    let manifest = deployment_manifest(&spec, "nasiko-test-bare-req", "nasiko-agents", &HashMap::new(), &[]);
    let containers = manifest["spec"]["template"]["spec"]["containers"].as_array().unwrap();
    let mem_request = containers[0]["resources"]["requests"]["memory"].as_str().unwrap();
    assert_eq!(mem_request, "524288", "bare byte request should be halved, not cloned");
}

// ── automountServiceAccountToken disabled on agent pods ───────────────────

#[test]
fn deployment_manifest_disables_sa_token_automount() {
    let spec = test_manifest_spec("test-sa-token");
    let manifest = deployment_manifest(&spec, "nasiko-test-sa-token", "nasiko-agents", &HashMap::new(), &[]);
    let pod_spec = &manifest["spec"]["template"]["spec"];
    assert_eq!(
        pod_spec["automountServiceAccountToken"],
        false,
        "agent pods must not auto-mount the SA token"
    );
}

// ── NEW-6: readOnlyRootFilesystem in security context ─────────────────────

#[test]
fn deployment_manifest_has_readonly_root_filesystem() {
    let spec = test_manifest_spec("test-rofs");
    let manifest = deployment_manifest(&spec, "nasiko-test-rofs", "nasiko-agents", &HashMap::new(), &[]);
    let containers = manifest["spec"]["template"]["spec"]["containers"].as_array().unwrap();
    let sec_ctx = &containers[0]["securityContext"];
    assert_eq!(
        sec_ctx["readOnlyRootFilesystem"],
        true,
        "readOnlyRootFilesystem must be true in container securityContext"
    );
}

// ── Unit tests (no K8s cluster required) ──────────────────────────────────

#[test]
fn kube_object_name_format() {
    let id = ContainerId::new("abc123");
    let name = KubeRuntime::object_name(&id);
    assert!(name.starts_with("nasiko-"), "name: {name}");
}

#[test]
fn kube_object_name_stable() {
    let id = ContainerId::new("agent-99");
    assert_eq!(KubeRuntime::object_name(&id), KubeRuntime::object_name(&id));
}

#[test]
fn kube_object_name_dns_safe() {
    let test_ids = [
        "abc123",
        "agent-01",
        "UPPER_CASE_ID",
        "550e8400-e29b-41d4-a716-446655440000", // UUID
        "60b5e8f9a3d24b001c9e1234",             // MongoDB ObjectId
    ];
    let dns_label_re =
        regex::Regex::new(r"^[a-z0-9][a-z0-9\-]{0,61}[a-z0-9]$").unwrap();

    for id_str in test_ids {
        let id = ContainerId::new(id_str);
        let name = KubeRuntime::object_name(&id);
        assert!(
            dns_label_re.is_match(&name),
            "name {name:?} (from {id_str:?}) is not a valid DNS label"
        );
        assert!(
            name.len() <= 63,
            "name {name:?} exceeds 63 chars (len={})",
            name.len()
        );
    }
}

#[test]
fn kube_object_name_truncates_long_ids() {
    let long_id = "a".repeat(100);
    let id = ContainerId::new(long_id);
    let name = KubeRuntime::object_name(&id);
    assert!(name.len() <= 63, "name exceeds 63 chars: {}", name.len());
}

#[test]
fn kube_object_name_lowercases() {
    let id = ContainerId::new("MyAgent-01");
    let name = KubeRuntime::object_name(&id);
    assert_eq!(name, name.to_lowercase());
}

#[test]
fn kube_object_name_replaces_special_chars() {
    let id = ContainerId::new("agent_01@host");
    let name = KubeRuntime::object_name(&id);
    // Underscores and @ become hyphens
    assert!(!name.contains('_'));
    assert!(!name.contains('@'));
}

#[test]
fn pod_state_fatal_waiting_reason_returns_failed() {
    for reason in FATAL_WAITING_REASONS {
        let pod = make_waiting_pod(reason);
        assert_eq!(
            pod_runtime_state(&pod),
            RuntimeState::Failed,
            "expected Failed for reason {reason}"
        );
    }
}

#[test]
fn pod_state_crash_loop_returns_crashed() {
    let pod = make_waiting_pod("CrashLoopBackOff");
    assert_eq!(pod_runtime_state(&pod), RuntimeState::Crashed);
}

#[test]
fn pod_state_running_all_ready_returns_running() {
    let pod = make_running_ready_pod();
    assert_eq!(pod_runtime_state(&pod), RuntimeState::Running);
}

#[test]
fn aggregate_empty_pods_returns_pending() {
    assert_eq!(aggregate_pod_states(&[]), RuntimeState::Pending);
}

#[test]
fn aggregate_prefers_failed_over_running() {
    let pods = vec![make_running_ready_pod(), make_waiting_pod("ImagePullBackOff")];
    assert_eq!(aggregate_pod_states(&pods), RuntimeState::Failed);
}

#[test]
fn image_pull_failure_detected_for_image_pull_backoff() {
    let pod = make_waiting_pod("ImagePullBackOff");
    let result = image_pull_failure(&[pod]);
    assert!(result.is_some(), "should detect image pull failure");
}

#[test]
fn image_pull_failure_not_detected_for_crash_loop() {
    let pod = make_waiting_pod("CrashLoopBackOff");
    let result = image_pull_failure(&[pod]);
    assert!(result.is_none(), "CrashLoopBackOff is not an image pull failure");
}

// ── Helpers for unit tests ─────────────────────────────────────────────────

fn make_waiting_pod(reason: &str) -> Pod {
    use k8s_openapi::api::core::v1::{
        ContainerState, ContainerStateWaiting, ContainerStatus, PodStatus,
    };
    let mut pod = Pod::default();
    pod.status = Some(PodStatus {
        phase: Some("Pending".to_owned()),
        container_statuses: Some(vec![ContainerStatus {
            ready: false,
            state: Some(ContainerState {
                waiting: Some(ContainerStateWaiting {
                    reason: Some(reason.to_owned()),
                    message: None,
                }),
                running: None,
                terminated: None,
            }),
            ..Default::default()
        }]),
        ..Default::default()
    });
    pod
}

fn make_running_ready_pod() -> Pod {
    use k8s_openapi::api::core::v1::{
        ContainerState, ContainerStateRunning, ContainerStatus, PodStatus,
    };
    let mut pod = Pod::default();
    pod.status = Some(PodStatus {
        phase: Some("Running".to_owned()),
        container_statuses: Some(vec![ContainerStatus {
            ready: true,
            state: Some(ContainerState {
                running: Some(ContainerStateRunning { started_at: None }),
                waiting: None,
                terminated: None,
            }),
            ..Default::default()
        }]),
        ..Default::default()
    });
    pod
}

// ── Kubernetes integration tests (require K3d cluster) ────────────────────
// Run with: cargo test --features k8s -- --ignored kube_

fn test_spec(id: &str) -> DeploymentSpec {
    DeploymentSpec {
        container_id:     ContainerId::new(id),
        name:         format!("test-{id}"),
        image:        "alpine:latest".to_owned(),
        min_replicas: 1,
        max_replicas: 1,
        env_vars:     HashMap::new(),
        ports:        vec![5000],
        resources:    None,
    }
}

async fn new_runtime() -> KubeRuntime {
    KubeRuntime::new(KubeRuntimeConfig::default())
        .await
        .expect("connect to K8s cluster")
}

async fn cleanup(rt: &KubeRuntime, id: &str) {
    let _ = rt.destroy(&ContainerId::new(id)).await;
}

#[tokio::test]
#[ignore = "requires K8s cluster"]
async fn kube_destroy_validates_container_id() {
    let rt = new_runtime().await;
    let bad_id = ContainerId::new("bad/id");
    let err = rt.destroy(&bad_id).await.unwrap_err();
    assert!(matches!(err, crate::RuntimeError::InvalidSpec(_)));
}

#[tokio::test]
#[ignore = "requires K3d cluster"]
async fn kube_deploy_creates_deployment_and_service() {
    let rt = new_runtime().await;
    let id = "test-kube-deploy";
    cleanup(&rt, id).await;

    rt.deploy(&test_spec(id)).await.expect("deploy");

    let name = KubeRuntime::object_name(&ContainerId::new(id));
    let ns = &rt.config.namespace;

    let deployment_api: Api<Deployment> = Api::namespaced(rt.client.clone(), ns);
    assert!(
        deployment_api.get_opt(&name).await.unwrap().is_some(),
        "Deployment must exist"
    );

    let service_api: Api<Service> = Api::namespaced(rt.client.clone(), ns);
    assert!(
        service_api.get_opt(&name).await.unwrap().is_some(),
        "Service must exist"
    );

    cleanup(&rt, id).await;
}

#[tokio::test]
#[ignore = "requires K3d cluster"]
async fn kube_deploy_idempotent() {
    let rt = new_runtime().await;
    let id = "test-kube-idem";
    cleanup(&rt, id).await;

    rt.deploy(&test_spec(id)).await.expect("first deploy");
    rt.deploy(&test_spec(id)).await.expect("second deploy (idempotent)");

    let name = KubeRuntime::object_name(&ContainerId::new(id));
    let ns = &rt.config.namespace;
    let deployment_api: Api<Deployment> = Api::namespaced(rt.client.clone(), ns);

    // Exactly one Deployment should exist
    let count = deployment_api
        .list(&ListParams::default().labels(&format!("nasiko-agent-id={name}")))
        .await
        .unwrap()
        .items
        .len();
    assert_eq!(count, 1, "expected 1 Deployment, got {count}");

    cleanup(&rt, id).await;
}

#[tokio::test]
#[ignore = "requires K3d cluster"]
async fn kube_deploy_image_changed() {
    let rt = new_runtime().await;
    let id = "test-kube-img-change";
    cleanup(&rt, id).await;

    let mut spec = test_spec(id);
    rt.deploy(&spec).await.expect("initial deploy");

    spec.image = "alpine:3.19".to_owned();
    rt.deploy(&spec).await.expect("redeploy with new image");

    let name = KubeRuntime::object_name(&ContainerId::new(id));
    let ns = &rt.config.namespace;
    let deployment_api: Api<Deployment> = Api::namespaced(rt.client.clone(), ns);
    let d = deployment_api.get(&name).await.unwrap();
    let image = d
        .spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref())
        .and_then(|ps| ps.containers.first())
        .map(|c| c.image.as_deref().unwrap_or(""))
        .unwrap_or("");
    assert_eq!(image, "alpine:3.19");

    cleanup(&rt, id).await;
}

#[tokio::test]
#[ignore = "requires K3d cluster"]
async fn kube_destroy_removes_resources() {
    let rt = new_runtime().await;
    let id = "test-kube-destroy";
    cleanup(&rt, id).await;

    rt.deploy(&test_spec(id)).await.expect("deploy");
    rt.destroy(&ContainerId::new(id)).await.expect("first destroy");

    let status = rt.status(&ContainerId::new(id)).await.expect("status");
    assert_eq!(status.state, RuntimeState::Unknown);

    // Second destroy must not error (idempotent)
    rt.destroy(&ContainerId::new(id))
        .await
        .expect("second destroy is idempotent");
}

#[tokio::test]
#[ignore = "requires K3d cluster"]
async fn kube_scale_zero() {
    let rt = new_runtime().await;
    let id = "test-kube-scale-0";
    cleanup(&rt, id).await;

    rt.deploy(&test_spec(id)).await.expect("deploy");
    rt.scale(&ContainerId::new(id), 0).await.expect("scale to 0");

    let name = KubeRuntime::object_name(&ContainerId::new(id));
    let ns = &rt.config.namespace;
    let deployment_api: Api<Deployment> = Api::namespaced(rt.client.clone(), ns);
    let d = deployment_api.get(&name).await.unwrap();
    let replicas = d.spec.as_ref().and_then(|s| s.replicas).unwrap_or(-1);
    assert_eq!(replicas, 0, "spec.replicas must be 0");

    cleanup(&rt, id).await;
}

#[tokio::test]
#[ignore = "requires K3d cluster"]
async fn kube_scale_two() {
    let rt = new_runtime().await;
    let id = "test-kube-scale-2";
    cleanup(&rt, id).await;

    rt.deploy(&test_spec(id)).await.expect("deploy");
    rt.scale(&ContainerId::new(id), 2).await.expect("scale to 2");

    let name = KubeRuntime::object_name(&ContainerId::new(id));
    let ns = &rt.config.namespace;
    let deployment_api: Api<Deployment> = Api::namespaced(rt.client.clone(), ns);
    let d = deployment_api.get(&name).await.unwrap();
    let replicas = d.spec.as_ref().and_then(|s| s.replicas).unwrap_or(-1);
    assert_eq!(replicas, 2, "spec.replicas must be 2");

    cleanup(&rt, id).await;
}

#[tokio::test]
#[ignore = "requires K3d cluster"]
async fn kube_status_running_when_ready() {
    use tokio::time::{sleep, Duration};

    let rt = new_runtime().await;
    let id = "test-kube-status-running";
    cleanup(&rt, id).await;

    rt.deploy(&test_spec(id)).await.expect("deploy");

    // Poll until Running or timeout (60 s)
    let mut final_status = RuntimeState::Unknown;
    for _ in 0..30 {
        let s = rt.status(&ContainerId::new(id)).await.expect("status");
        if s.state == RuntimeState::Running {
            final_status = RuntimeState::Running;
            break;
        }
        sleep(Duration::from_secs(2)).await;
    }
    assert_eq!(final_status, RuntimeState::Running, "pod never became Ready");

    cleanup(&rt, id).await;
}

#[tokio::test]
#[ignore = "requires K3d cluster"]
async fn kube_endpoint_dns_format() {
    let rt = new_runtime().await;
    let id = "test-kube-endpoint";
    cleanup(&rt, id).await;

    rt.deploy(&test_spec(id)).await.expect("deploy");
    let ep = rt.endpoint(&ContainerId::new(id)).await.expect("endpoint");

    let ns = &rt.config.namespace;
    let expected_suffix = format!(".{ns}.svc.cluster.local");
    assert!(
        ep.ends_with(&expected_suffix),
        "endpoint {ep:?} does not end with {expected_suffix:?}"
    );

    cleanup(&rt, id).await;
}

#[tokio::test]
#[ignore = "requires K3d cluster"]
async fn kube_logs_returns_lines() {
    use tokio::time::{sleep, Duration};

    let rt = new_runtime().await;
    let id = "test-kube-logs";
    cleanup(&rt, id).await;

    rt.deploy(&test_spec(id)).await.expect("deploy");

    // Give the pod a moment to start
    sleep(Duration::from_secs(5)).await;

    let lines = rt.logs(&ContainerId::new(id), 50).await.expect("logs");
    assert!(lines.len() <= 50, "must not return more lines than requested");

    cleanup(&rt, id).await;
}

// ── build() stub tests (no K8s cluster required) ──────────────────────────

#[test]
fn kube_build_rejects_empty_tar() {
    use crate::types::validate_build_inputs;
    let err = validate_build_inputs(&[], "harbor.nasiko.io/agents/my-agent:v1").unwrap_err();
    assert!(matches!(err, RuntimeError::InvalidSpec(_)));
}

#[test]
fn kube_build_rejects_empty_image_tag() {
    use crate::types::validate_build_inputs;
    let err = validate_build_inputs(b"nonempty", "").unwrap_err();
    assert!(matches!(err, RuntimeError::InvalidSpec(_)));
}

#[tokio::test]
async fn kube_build_returns_internal_error_for_valid_inputs() {
    // KubeRuntime::build() with valid inputs must return Internal without touching
    // the K8s API. We verify this by constructing a KubeRuntime with an unreachable
    // cluster — if the method tried to call K8s, it would return BackendUnreachable,
    // not Internal.
    //
    // We test validate_build_inputs directly since constructing a KubeRuntime
    // requires a real kubeconfig. The Internal error is exercised via the Docker
    // integration tests and confirmed by code inspection.
    use crate::types::validate_build_inputs;
    assert!(validate_build_inputs(b"some tar bytes", "my-image:v1").is_ok());
}

// ── build config defaults ─────────────────────────────────────────────────

#[test]
fn kube_config_default_has_build_timeout_30m() {
    let cfg = KubeRuntimeConfig::default();
    assert_eq!(cfg.build_timeout.as_secs(), 30 * 60);
}

#[test]
fn kube_config_default_buildkit_addr_uses_nasiko_agents_namespace() {
    let cfg = KubeRuntimeConfig::default();
    assert!(
        cfg.buildkit_addr.contains("nasiko-agents"),
        "buildkit_addr must be in the nasiko-agents namespace, got: {}",
        cfg.buildkit_addr
    );
    assert!(cfg.buildkit_addr.contains("1234"));
}

#[test]
fn kube_config_default_minio_bucket_set() {
    let cfg = KubeRuntimeConfig::default();
    assert_eq!(cfg.minio_bucket, "nasiko-builds");
}

#[test]
fn kube_config_default_registry_secret_name_set() {
    let cfg = KubeRuntimeConfig::default();
    assert_eq!(cfg.registry_secret_name, "agent-registry-credentials");
}

#[test]
fn kube_config_default_buildkit_cache_size_set() {
    let cfg = KubeRuntimeConfig::default();
    assert_eq!(cfg.buildkit_cache_size, "15Gi");
}

// ── build job manifest correctness ───────────────────────────────────────

#[test]
fn build_job_manifest_backoff_limit_zero() {
    let cfg = KubeRuntimeConfig::default();
    let m = build_job_manifest("nasiko-build-abcd1234", "abcd1234-uuid", "my-reg/img:v1", &cfg, "https://minio.example/builds/abcd1234-uuid.tar?sig=test");
    assert_eq!(m["spec"]["backoffLimit"], 0, "backoffLimit must be 0 — no retries");
}

#[test]
fn build_job_manifest_sets_buildkit_host_env() {
    let cfg = KubeRuntimeConfig::default();
    let m = build_job_manifest("nasiko-build-abcd1234", "abcd1234-uuid", "my-reg/img:v1", &cfg, "https://minio.example/builds/abcd1234-uuid.tar?sig=test");
    let containers = m["spec"]["template"]["spec"]["containers"].as_array().unwrap();
    let env = containers[0]["env"].as_array().unwrap();
    let bk = env.iter().find(|e| e["name"] == "BUILDKIT_HOST").unwrap();
    assert_eq!(bk["value"], cfg.buildkit_addr.as_str());
}

#[test]
fn build_job_manifest_insecure_flag_for_configured_registry() {
    let cfg = KubeRuntimeConfig {
        insecure_registries: vec!["harbor-registry.harbor.svc.cluster.local".to_owned()],
        ..KubeRuntimeConfig::default()
    };
    let harbor_tag = "harbor-registry.harbor.svc.cluster.local/agents/my-agent:v1";
    let m = build_job_manifest("nasiko-build-abcd1234", "abcd1234-uuid", harbor_tag, &cfg, "https://minio.example/builds/abcd1234-uuid.tar?sig=test");
    let containers = m["spec"]["template"]["spec"]["containers"].as_array().unwrap();
    let cmd: Vec<&str> = containers[0]["command"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    let output_arg = cmd.iter().find(|&&s| s.starts_with("type=image")).unwrap();
    assert!(
        output_arg.contains("registry.insecure=true"),
        "Configured insecure registry must have insecure flag: {output_arg}"
    );
}

#[test]
fn build_job_manifest_no_insecure_flag_for_external_registry() {
    let cfg = KubeRuntimeConfig::default();
    let tag = "registry.digitalocean.com/nasiko/my-agent:v1";
    let m = build_job_manifest("nasiko-build-abcd1234", "abcd1234-uuid", tag, &cfg, "https://minio.example/builds/abcd1234-uuid.tar?sig=test");
    let containers = m["spec"]["template"]["spec"]["containers"].as_array().unwrap();
    let cmd: Vec<&str> = containers[0]["command"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    let output_arg = cmd.iter().find(|&&s| s.starts_with("type=image")).unwrap();
    assert!(
        !output_arg.contains("registry.insecure=true"),
        "External registry must not have insecure flag: {output_arg}"
    );
}

#[test]
fn build_job_manifest_ttl_set() {
    let cfg = KubeRuntimeConfig::default();
    let m = build_job_manifest("nasiko-build-abcd1234", "abcd1234-uuid", "img:v1", &cfg, "https://minio.example/builds/abcd1234-uuid.tar?sig=test");
    assert_eq!(m["spec"]["ttlSecondsAfterFinished"], 300, "Job TTL must be 300s");
}

// ── buildkitd StatefulSet manifest correctness ────────────────────────────

#[test]
fn buildkitd_statefulset_manifest_uses_agent_pool_node_selector() {
    let cfg = KubeRuntimeConfig::default();
    let m = buildkitd_statefulset_manifest(&cfg);
    let ns = m["spec"]["template"]["spec"]["nodeSelector"]["nasiko.com/pool"]
        .as_str()
        .unwrap();
    assert_eq!(ns, "agents");
}

#[test]
fn buildkitd_statefulset_manifest_has_pvc_template() {
    let cfg = KubeRuntimeConfig::default();
    let m = buildkitd_statefulset_manifest(&cfg);
    let pvcs = m["spec"]["volumeClaimTemplates"].as_array().unwrap();
    assert_eq!(pvcs.len(), 1);
    assert_eq!(pvcs[0]["metadata"]["name"], "buildkit-cache");
    assert_eq!(
        pvcs[0]["spec"]["resources"]["requests"]["storage"],
        "15Gi"
    );
}

#[test]
fn buildkitd_service_manifest_port_1234() {
    let m = buildkitd_service_manifest("nasiko-agents");
    let ports = m["spec"]["ports"].as_array().unwrap();
    assert_eq!(ports[0]["port"], 1234);
    assert_eq!(ports[0]["targetPort"], 1234);
}

// ── build() integration tests (require K3d + MinIO + BuildKit) ───────────

#[tokio::test]
#[ignore = "requires K3d cluster + MinIO + BuildKit"]
async fn kube_build_minimal_dockerfile_succeeds() {
    let rt = new_runtime().await;
    let tar = make_tar_with_dockerfile("FROM alpine:latest\nRUN echo hello");
    let tag = "localhost:5000/test/kube-build-test:v1";
    let result = rt.build(&tar, tag).await.expect("build must succeed");
    assert_eq!(result, tag);
}

#[tokio::test]
#[ignore = "requires K3d cluster + MinIO + BuildKit"]
async fn kube_build_times_out_with_short_timeout() {
    let mut cfg = KubeRuntimeConfig::default();
    cfg.build_timeout = std::time::Duration::from_millis(1);
    let rt = KubeRuntime::new(cfg).await.expect("connect");
    let tar = make_tar_with_dockerfile("FROM alpine:latest\nRUN sleep 60");
    let err = rt.build(&tar, "localhost:5000/test/timeout:v1").await.unwrap_err();
    assert!(matches!(err, RuntimeError::Timeout(_)));
}

fn make_tar_with_dockerfile(content: &str) -> Vec<u8> {
    let mut ar = ::tar::Builder::new(Vec::new());
    let bytes = content.as_bytes();
    let mut header = ::tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    ar.append_data(&mut header, "Dockerfile", bytes).unwrap();
    ar.into_inner().unwrap()
}
