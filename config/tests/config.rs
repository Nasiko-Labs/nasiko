/// Tests for nasiko-config `Config::from_env()`.
///
/// All tests manipulate environment variables and must be isolated. We use
/// `#[serial]` from the `serial_test` crate so tests never run in parallel
/// against each other (env vars are process-global state).
use nasiko_config::Config;
use serial_test::serial;

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Minimum set of environment variables required for `Config::from_env()` to
/// succeed. Returns a list of `(key, value)` pairs. Call `set_required_vars`
/// before the test and `unset_required_vars` after.
fn required_vars() -> Vec<(&'static str, &'static str)> {
    vec![
        ("DATABASE_URL", "postgres://test:test@localhost/testdb"),
        ("REDIS_URL", "redis://localhost:6379"),
        ("S3_SECRET_KEY", "test-s3-secret"),
        ("SECRETS_ENCRYPTION_KEY", "test-encryption-key"),
        ("ADMIN_PASSWORD", "test-admin-password"),
    ]
}

fn set_required_vars() {
    for (k, v) in required_vars() {
        // SAFETY: single-threaded due to #[serial]
        unsafe { std::env::set_var(k, v) };
    }
}

fn unset_required_vars() {
    for (k, _) in required_vars() {
        unsafe { std::env::remove_var(k) };
    }
}

/// Unset a list of optional env vars so defaults kick in.
fn unset_vars(keys: &[&str]) {
    for k in keys {
        unsafe { std::env::remove_var(k) };
    }
}

// ─── Basic construction ──────────────────────────────────────────────────────

#[test]
#[serial]
fn from_env_succeeds_with_required_vars() {
    set_required_vars();

    let cfg = Config::from_env().expect("Config::from_env() should succeed when required vars are set");

    assert_eq!(cfg.database_url, "postgres://test:test@localhost/testdb");
    assert_eq!(cfg.redis_url, "redis://localhost:6379");
    assert_eq!(cfg.s3_secret_key, "test-s3-secret");
    assert_eq!(cfg.secrets_encryption_key, "test-encryption-key");
    assert_eq!(cfg.admin_password, "test-admin-password");

    unset_required_vars();
}

// ─── Missing required vars return errors ────────────────────────────────────

#[test]
#[serial]
fn missing_database_url_returns_error() {
    set_required_vars();
    unsafe { std::env::remove_var("DATABASE_URL") };

    let result = Config::from_env();
    assert!(result.is_err(), "Expected error when DATABASE_URL is missing");
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("DATABASE_URL"), "Error message should name the missing var: {msg}");

    unset_required_vars();
}

#[test]
#[serial]
fn missing_redis_url_returns_error() {
    set_required_vars();
    unsafe { std::env::remove_var("REDIS_URL") };

    let result = Config::from_env();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("REDIS_URL"));

    unset_required_vars();
}

#[test]
#[serial]
fn missing_s3_secret_key_returns_error() {
    set_required_vars();
    unsafe { std::env::remove_var("S3_SECRET_KEY") };

    let result = Config::from_env();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("S3_SECRET_KEY"));

    unset_required_vars();
}

#[test]
#[serial]
fn missing_secrets_encryption_key_returns_error() {
    set_required_vars();
    unsafe { std::env::remove_var("SECRETS_ENCRYPTION_KEY") };

    let result = Config::from_env();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("SECRETS_ENCRYPTION_KEY"));

    unset_required_vars();
}

#[test]
#[serial]
fn missing_admin_password_returns_error() {
    set_required_vars();
    unsafe { std::env::remove_var("ADMIN_PASSWORD") };

    let result = Config::from_env();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("ADMIN_PASSWORD"));

    unset_required_vars();
}

// ─── Default values for optional fields ─────────────────────────────────────

#[test]
#[serial]
fn optional_fields_use_defaults_when_not_set() {
    set_required_vars();
    // Ensure none of the optional vars are set
    unset_vars(&[
        "CP_BIND",
        "CP_DOMAIN",
        "AGENT_RUNTIME",
        "K8S_NAMESPACE",
        "KUBECONFIG",
        "S3_ENDPOINT",
        "S3_BUCKET",
        "S3_ACCESS_KEY",
        "S3_REGION",
        "OCI_STORAGE_BUCKET",
        "AGENT_IMAGE_REGISTRY",
        "SEED_AGENTS",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENAI_MODEL",
        "ROUTER_MODEL",
        "CAPABILITY_GENERATOR_MODEL",
        "A2A_DISCOVERY_URL",
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_PROTOCOL",
        "OTEL_EXPORTER_OTLP_HEADERS",
        "OTEL_SERVICE_NAME",
        "OTEL_TRACES_SAMPLER_ARG",
        "OTEL_COLLECTOR_ENDPOINT",
        "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT",
        "TEMPO_URL",
        "LOKI_URL",
        "NASIKO_FLOW_MAX_DEPTH",
        "NASIKO_FLOW_MAX_FAN_OUT",
        "NASIKO_FLOW_MAX_TOKENS",
        "NASIKO_FLOW_TIMEOUT_SECS",
        "GITHUB_CLIENT_ID",
        "GITHUB_CLIENT_SECRET",
        "ROUTER_SHORTLIST_THRESHOLD",
        "ROUTER_SHORTLIST_SIZE",
        "MAX_ROUTER_HISTORY_MESSAGES",
        "EMBEDDING_MODEL",
        "ROUTER_AGENT_TIMEOUT_SECS",
        "GITHUB_CALLBACK_URL",
        "DOCKER_AGENT_NETWORK",
        "OCI_REGISTRY_HOST",
        "CONTAINER_HOURS_POLL_SECS",
        "GIT_CLONE_ALLOWED_HOSTS",
        "REGISTRY_IMPORT_ALLOWED_HOSTS",
        "ADMIN_USERNAME",
    ]);

    let cfg = Config::from_env().expect("Config::from_env() should succeed with all defaults");

    assert_eq!(cfg.bind, "0.0.0.0:8080");
    assert_eq!(cfg.domain, None);
    assert_eq!(cfg.agent_runtime, "local");
    assert_eq!(cfg.k8s_namespace, "nasiko-agents");
    assert_eq!(cfg.kubeconfig, None);
    assert_eq!(cfg.s3_endpoint, "http://localhost:9000");
    assert_eq!(cfg.s3_bucket, "nasiko");
    assert_eq!(cfg.s3_access_key, "nasiko");
    assert_eq!(cfg.s3_region, "us-east-1");
    assert_eq!(cfg.oci_storage_bucket, "nasiko-artifacts");
    assert_eq!(cfg.agent_image_registry, "");
    assert_eq!(cfg.seed_agents, None);
    assert_eq!(cfg.openai_api_key, None);
    assert_eq!(cfg.openai_base_url, None);
    assert_eq!(cfg.openai_model, "gpt-4o-mini");
    assert_eq!(cfg.router_model, "gpt-4o-mini");
    assert_eq!(cfg.capability_generator_model, "gpt-4o-mini");
    assert_eq!(cfg.a2a_discovery_url, None);
    assert_eq!(cfg.otel_endpoint, None);
    assert_eq!(cfg.otel_protocol, "grpc");
    assert_eq!(cfg.otel_headers, None);
    assert_eq!(cfg.otel_service_name, "nasiko-cp");
    assert_eq!(cfg.otel_sample_ratio, "1.0");
    assert_eq!(cfg.otel_collector_endpoint, "http://otel-collector:4318");
    assert!(cfg.otel_capture_content);
    assert_eq!(cfg.tempo_url, "http://tempo.nasiko-infra.svc.cluster.local:3200");
    assert_eq!(cfg.loki_url, "http://loki.nasiko-infra.svc.cluster.local:3100");
    assert_eq!(cfg.flow_max_depth, 5);
    assert_eq!(cfg.flow_max_fan_out, 20);
    assert_eq!(cfg.flow_max_tokens, 100_000);
    assert_eq!(cfg.flow_timeout_secs, 120);
    assert_eq!(cfg.github_client_id, None);
    assert_eq!(cfg.github_client_secret, None);
    assert_eq!(cfg.router_shortlist_threshold, 15);
    assert_eq!(cfg.router_shortlist_size, 10);
    assert_eq!(cfg.max_router_history_messages, 20);
    assert_eq!(cfg.embedding_model, "text-embedding-3-small");
    assert_eq!(cfg.router_agent_timeout_secs, 60);
    assert_eq!(cfg.github_callback_url, None);
    assert_eq!(cfg.docker_agent_network, None);
    assert_eq!(cfg.oci_registry_host, None);
    assert_eq!(cfg.container_hours_poll_secs, 60);
    assert_eq!(cfg.admin_username, "admin");

    // git_clone_allowed_hosts has a hardcoded default
    assert!(cfg.git_clone_allowed_hosts.contains(&"github.com".to_string()));
    assert!(cfg.git_clone_allowed_hosts.contains(&"gitlab.com".to_string()));
    assert!(cfg.git_clone_allowed_hosts.contains(&"bitbucket.org".to_string()));

    // registry_import_allowed_hosts defaults to empty
    assert!(cfg.registry_import_allowed_hosts.is_empty());

    unset_required_vars();
}

// ─── Boolean env var parsing ─────────────────────────────────────────────────

#[test]
#[serial]
fn otel_capture_content_true_when_set_to_true() {
    set_required_vars();
    unsafe { std::env::set_var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "true") };

    let cfg = Config::from_env().unwrap();
    assert!(cfg.otel_capture_content);

    unsafe { std::env::remove_var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT") };
    unset_required_vars();
}

#[test]
#[serial]
fn otel_capture_content_false_when_set_to_false() {
    set_required_vars();
    unsafe { std::env::set_var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "false") };

    let cfg = Config::from_env().unwrap();
    assert!(!cfg.otel_capture_content);

    unsafe { std::env::remove_var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT") };
    unset_required_vars();
}

#[test]
#[serial]
fn otel_capture_content_false_when_set_to_1() {
    // The config only checks `v == "true"` — "1" does NOT enable it.
    set_required_vars();
    unsafe { std::env::set_var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT", "1") };

    let cfg = Config::from_env().unwrap();
    assert!(!cfg.otel_capture_content, "\"1\" should not be treated as true by Config");

    unsafe { std::env::remove_var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT") };
    unset_required_vars();
}

// ─── Numeric field parsing ────────────────────────────────────────────────────

#[test]
#[serial]
fn router_shortlist_threshold_is_parsed_from_env() {
    set_required_vars();
    unsafe { std::env::set_var("ROUTER_SHORTLIST_THRESHOLD", "42") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.router_shortlist_threshold, 42);

    unsafe { std::env::remove_var("ROUTER_SHORTLIST_THRESHOLD") };
    unset_required_vars();
}

#[test]
#[serial]
fn router_shortlist_size_is_parsed_from_env() {
    set_required_vars();
    unsafe { std::env::set_var("ROUTER_SHORTLIST_SIZE", "5") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.router_shortlist_size, 5);

    unsafe { std::env::remove_var("ROUTER_SHORTLIST_SIZE") };
    unset_required_vars();
}

#[test]
#[serial]
fn flow_max_depth_is_parsed_from_env() {
    set_required_vars();
    unsafe { std::env::set_var("NASIKO_FLOW_MAX_DEPTH", "10") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.flow_max_depth, 10);

    unsafe { std::env::remove_var("NASIKO_FLOW_MAX_DEPTH") };
    unset_required_vars();
}

#[test]
#[serial]
fn flow_max_tokens_is_parsed_from_env() {
    set_required_vars();
    unsafe { std::env::set_var("NASIKO_FLOW_MAX_TOKENS", "999999") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.flow_max_tokens, 999_999);

    unsafe { std::env::remove_var("NASIKO_FLOW_MAX_TOKENS") };
    unset_required_vars();
}

#[test]
#[serial]
fn router_agent_timeout_secs_is_parsed_from_env() {
    set_required_vars();
    unsafe { std::env::set_var("ROUTER_AGENT_TIMEOUT_SECS", "120") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.router_agent_timeout_secs, 120);

    unsafe { std::env::remove_var("ROUTER_AGENT_TIMEOUT_SECS") };
    unset_required_vars();
}

#[test]
#[serial]
fn invalid_numeric_env_var_falls_back_to_default() {
    set_required_vars();
    unsafe { std::env::set_var("ROUTER_SHORTLIST_THRESHOLD", "not-a-number") };

    let cfg = Config::from_env().unwrap();
    // Falls back to default of 15
    assert_eq!(cfg.router_shortlist_threshold, 15);

    unsafe { std::env::remove_var("ROUTER_SHORTLIST_THRESHOLD") };
    unset_required_vars();
}

// ─── Optional string fields ───────────────────────────────────────────────────

#[test]
#[serial]
fn seed_agents_is_read_from_env() {
    set_required_vars();
    unsafe { std::env::set_var("SEED_AGENTS", "agent1:latest agent2:latest") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.seed_agents, Some("agent1:latest agent2:latest".to_string()));

    unsafe { std::env::remove_var("SEED_AGENTS") };
    unset_required_vars();
}

#[test]
#[serial]
fn openai_api_key_is_read_from_env() {
    set_required_vars();
    unsafe { std::env::set_var("OPENAI_API_KEY", "sk-test-key") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.openai_api_key, Some("sk-test-key".to_string()));

    unsafe { std::env::remove_var("OPENAI_API_KEY") };
    unset_required_vars();
}

#[test]
#[serial]
fn cp_domain_is_read_from_env() {
    set_required_vars();
    unsafe { std::env::set_var("CP_DOMAIN", "example.nasiko.dev") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.domain, Some("example.nasiko.dev".to_string()));

    unsafe { std::env::remove_var("CP_DOMAIN") };
    unset_required_vars();
}

// ─── Vec<String> fields ──────────────────────────────────────────────────────

#[test]
#[serial]
fn git_clone_allowed_hosts_parses_comma_separated_list() {
    set_required_vars();
    unsafe { std::env::set_var("GIT_CLONE_ALLOWED_HOSTS", "github.com, internal.corp.dev , bitbucket.org") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.git_clone_allowed_hosts, vec![
        "github.com".to_string(),
        "internal.corp.dev".to_string(),
        "bitbucket.org".to_string(),
    ]);

    unsafe { std::env::remove_var("GIT_CLONE_ALLOWED_HOSTS") };
    unset_required_vars();
}

#[test]
#[serial]
fn registry_import_allowed_hosts_parses_comma_separated_list() {
    set_required_vars();
    unsafe { std::env::set_var("REGISTRY_IMPORT_ALLOWED_HOSTS", "ghcr.io,quay.io,registry.nasiko.dev") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.registry_import_allowed_hosts, vec![
        "ghcr.io".to_string(),
        "quay.io".to_string(),
        "registry.nasiko.dev".to_string(),
    ]);

    unsafe { std::env::remove_var("REGISTRY_IMPORT_ALLOWED_HOSTS") };
    unset_required_vars();
}

#[test]
#[serial]
fn registry_import_allowed_hosts_empty_when_not_set() {
    set_required_vars();
    unsafe { std::env::remove_var("REGISTRY_IMPORT_ALLOWED_HOSTS") };

    let cfg = Config::from_env().unwrap();
    assert!(cfg.registry_import_allowed_hosts.is_empty());

    unset_required_vars();
}

// ─── Empty-string filtering for optional fields ──────────────────────────────

#[test]
#[serial]
fn docker_agent_network_is_none_when_set_to_empty_string() {
    set_required_vars();
    unsafe { std::env::set_var("DOCKER_AGENT_NETWORK", "") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.docker_agent_network, None,
        "Empty DOCKER_AGENT_NETWORK should be treated as None");

    unsafe { std::env::remove_var("DOCKER_AGENT_NETWORK") };
    unset_required_vars();
}

#[test]
#[serial]
fn docker_agent_network_is_some_when_set_to_nonempty_string() {
    set_required_vars();
    unsafe { std::env::set_var("DOCKER_AGENT_NETWORK", "nasiko-cloud-rs_default") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.docker_agent_network, Some("nasiko-cloud-rs_default".to_string()));

    unsafe { std::env::remove_var("DOCKER_AGENT_NETWORK") };
    unset_required_vars();
}

#[test]
#[serial]
fn kubeconfig_is_none_when_set_to_empty_string() {
    set_required_vars();
    unsafe { std::env::set_var("KUBECONFIG", "") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.kubeconfig, None,
        "Empty KUBECONFIG should be treated as None");

    unsafe { std::env::remove_var("KUBECONFIG") };
    unset_required_vars();
}

#[test]
#[serial]
fn oci_registry_host_is_none_when_set_to_empty_string() {
    set_required_vars();
    unsafe { std::env::set_var("OCI_REGISTRY_HOST", "") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.oci_registry_host, None);

    unsafe { std::env::remove_var("OCI_REGISTRY_HOST") };
    unset_required_vars();
}

#[test]
#[serial]
fn oci_registry_host_is_some_when_set() {
    set_required_vars();
    unsafe { std::env::set_var("OCI_REGISTRY_HOST", "localhost:8443") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.oci_registry_host, Some("localhost:8443".to_string()));

    unsafe { std::env::remove_var("OCI_REGISTRY_HOST") };
    unset_required_vars();
}

// ─── Override defaults ───────────────────────────────────────────────────────

#[test]
#[serial]
fn bind_address_can_be_overridden() {
    set_required_vars();
    unsafe { std::env::set_var("CP_BIND", "127.0.0.1:9090") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.bind, "127.0.0.1:9090");

    unsafe { std::env::remove_var("CP_BIND") };
    unset_required_vars();
}

#[test]
#[serial]
fn admin_username_default_is_admin() {
    set_required_vars();
    unsafe { std::env::remove_var("ADMIN_USERNAME") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.admin_username, "admin");

    unset_required_vars();
}

#[test]
#[serial]
fn admin_username_can_be_overridden() {
    set_required_vars();
    unsafe { std::env::set_var("ADMIN_USERNAME", "superadmin") };

    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.admin_username, "superadmin");

    unsafe { std::env::remove_var("ADMIN_USERNAME") };
    unset_required_vars();
}