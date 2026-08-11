//! Builds a `nasiko_config::Config` for the bench harness, pointed at the
//! in-process mock LLM and carrying a dummy (non-empty) `openai_api_key` —
//! `ee/server::build_ee_app` panics at startup without one (the MAF worker
//! requires it), even though the worker itself sits idle unless flows are
//! explicitly queued.

use nasiko_config::Config;

/// Shared with `seed::seed()` so both the env var `build_bench_config` sets
/// and the JWTs `seed()` mints are signed with the same secret.
pub const BENCH_JWT_SECRET: &str = "bench-secret-for-nasiko-benches";

pub fn pg_admin_url() -> String {
    std::env::var("BENCH_PG_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".into())
}

pub fn redis_url() -> String {
    std::env::var("BENCH_REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".into())
}

fn s3_endpoint() -> String {
    std::env::var("BENCH_S3_ENDPOINT").unwrap_or_else(|_| "http://localhost:9000".into())
}

/// Sets the handful of env vars some components read directly instead of via
/// `Config` (JWT_SECRET, S3_*, SECRETS_ENCRYPTION_KEY) — mirrors
/// `oss/server/tests/common::test_config`. Criterion benches run as a single
/// process, so mutating process env here is safe (no concurrent test threads).
pub fn build_bench_config(database_url: String, mock_llm_base_url: &str) -> Config {
    let s3_ep = s3_endpoint();
    unsafe {
        std::env::set_var("JWT_SECRET", BENCH_JWT_SECRET);
        std::env::set_var("S3_ENDPOINT", &s3_ep);
        std::env::set_var("S3_ACCESS_KEY", "nasiko");
        std::env::set_var("S3_SECRET_KEY", "nasiko123");
        std::env::set_var("S3_REGION", "us-east-1");
        std::env::set_var(
            "SECRETS_ENCRYPTION_KEY",
            "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=",
        );
        // `oss/server/src/router/a2a_dispatch.rs::orchestrator_stream` builds
        // its `OrchestratorConfig` by reading `OPENAI_API_KEY`/`OPENAI_BASE_URL`
        // straight from process env — NOT from `Config.openai_api_key`/
        // `openai_base_url` (those only reach `nasiko_react_agent::Orchestrator`
        // when a caller wires them through explicitly, which this handler
        // doesn't). Without this, `run_stream_inner` `?`-returns silently on
        // the missing key with no event sent — the SSE stream just ends after
        // its two bookkeeping frames, no error visible anywhere.
        std::env::set_var("OPENAI_API_KEY", "bench-dummy-key");
        std::env::set_var("OPENAI_BASE_URL", mock_llm_base_url);
    }

    Config {
        bind: "127.0.0.1:0".into(),
        domain: None,
        database_url,
        redis_url: redis_url(),
        agent_runtime: "simulated".into(),
        observability_enabled: false,
        tenant_id: None,
        app_base_url: String::new(),
        composio_api_key: None,
        composio_base_url: "https://backend.composio.dev".into(),
        composio_webhook_secret: None,
        mcp_gateway_public_url: None,
        mcp_oauth_redirect_base_url: None,
        composio_callback_base_url: None,
        mcp_session_ttl_seconds: 300,
        mcp_perm_cache_ttl_seconds: 30,
        mcp_manifest_ttl_seconds: 300,
        mcp_toolcount_ttl_seconds: 3600,
        seed_toolkits: vec![],
        mcp_upload_max_bytes: 50 * 1024 * 1024,
        mcp_upload_default_port: 8080,
        mcp_servers_network: "nasiko-mcp-servers-net".into(),
        mcp_upload_max_replicas: 1,
        agent_max_replicas: 1,
        k8s_namespace: "nasiko-bench".into(),
        kubeconfig: None,
        s3_endpoint: s3_ep,
        s3_bucket: "nasiko-bench".into(),
        s3_access_key: "nasiko".into(),
        s3_secret_key: "nasiko123".into(),
        s3_region: "us-east-1".into(),
        secrets_encryption_key: "12345678901234567890123456789012".into(),
        oci_storage_bucket: "nasiko-bench-artifacts".into(),
        agent_image_registry: String::new(),
        build_push_token: String::new(),
        seed_agents: None,
        // Dummy but non-empty — ee build_ee_app requires Some(), and a
        // non-empty api_key is also what lets Stage 1 (VectorStore) attempt
        // real calls; Stage 1 is instead kept skipped by seeding fewer agents
        // than `router_shortlist_threshold` below.
        openai_api_key: Some("bench-dummy-key".into()),
        openai_base_url: Some(mock_llm_base_url.to_string()),
        openai_model: "mock-model".into(),
        router_model: "mock-model".into(),
        capability_generator_model: "mock-model".into(),
        mcp_description_model: "mock-model".into(),
        a2a_discovery_url: None,
        otel_endpoint: None,
        otel_protocol: "grpc".into(),
        otel_headers: None,
        otel_service_name: "nasiko-bench".into(),
        otel_sample_ratio: "0.0".into(),
        otel_collector_endpoint: "http://localhost:4318".into(),
        otel_capture_content: false,
        tempo_url: "http://localhost:3200".into(),
        loki_url: "http://localhost:3100".into(),
        flow_max_depth: 5,
        flow_max_fan_out: 20,
        flow_max_tokens: 1_000_000,
        flow_timeout_secs: 120,
        github_client_id: None,
        github_client_secret: None,
        router_shortlist_threshold: 15,
        router_shortlist_size: 10,
        max_router_history_messages: 20,
        embedding_model: "mock-embedding".into(),
        router_agent_timeout_secs: 60,
        github_callback_url: None,
        github_central_callback_url: None,
        docker_agent_network: None,
        oci_registry_host: None,
        git_clone_allowed_hosts: vec![],
        registry_import_allowed_hosts: vec![],
        cors_allowed_origins: vec![],
        admin_username: "admin".into(),
        admin_password: "bench-admin-password".into(),
        oidc_issuer_url: None,
        oidc_client_id: None,
        oidc_client_secret: None,
        oidc_redirect_uri: None,
        oidc_allowed_redirect_origins: vec![],
        oidc_scopes: "openid profile email".into(),
        oidc_provider_label: "microsoft_entra".into(),
        container_hours_poll_secs: 60,
    }
}
