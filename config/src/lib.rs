use nasiko_utils::{env_or, env_parse, required_env};

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub domain: Option<String>,
    pub database_url: String,
    pub redis_url: String,
    pub agent_runtime: String,
    pub k8s_namespace: String,
    pub kubeconfig: Option<String>,
    pub s3_endpoint: String,
    pub s3_bucket: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub s3_region: String,
    pub secrets_encryption_key: String,
    pub oci_storage_bucket: String,
    /// Registry prefix prepended to agent image tags at build time.
    /// e.g. `"host.docker.internal:5001"` for local K8s dev.
    /// Empty string → no prefix (Docker local mode).
    pub agent_image_registry: String,
    /// Shared credential the in-cluster BuildKit build Job presents (HTTP
    /// Basic auth, username `"build-service"`) to push freshly-built agent
    /// images into the built-in OCI registry — see
    /// `nasiko_oci::authz::Writer::BuildService`. Empty means not configured
    /// (fine for `AGENT_RUNTIME=local`, where no such build path exists).
    pub build_push_token: String,
    pub seed_agents: Option<String>,
    pub openai_api_key: Option<String>,
    pub openai_base_url: Option<String>,
    pub openai_model: String,
    pub router_model: String,
    pub capability_generator_model: String,
    pub a2a_discovery_url: Option<String>,
    pub otel_endpoint: Option<String>,
    pub otel_protocol: String,
    pub otel_headers: Option<String>,
    pub otel_service_name: String,
    pub otel_sample_ratio: String,
    pub otel_collector_endpoint: String,
    pub otel_capture_content: bool,
    pub tempo_url: String,
    pub loki_url: String,
    /// Whether the Tempo/Loki observability backend is enabled — the SINGLE
    /// source of truth for "is observability configured". True iff both
    /// `TEMPO_URL` and `LOKI_URL` were explicitly set in the environment
    /// (the URLs above always carry a default, so their presence alone can't
    /// answer this). Everything that gates on observability reads this flag
    /// rather than re-inspecting env, so no two code paths can disagree.
    pub observability_enabled: bool,
    pub flow_max_depth: i32,
    pub flow_max_fan_out: i32,
    pub flow_max_tokens: i64,
    pub flow_timeout_secs: i32,
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
    /// OIDC issuer authority, e.g. `https://login.microsoftonline.com/<tenant-id>/v2.0`
    /// for Microsoft Entra ID — or any other OIDC-compliant provider. `None`
    /// disables OIDC login entirely (see `docs/OIDC_SSO_SETUP.md`).
    pub oidc_issuer_url: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_client_secret: Option<String>,
    /// Must exactly match the redirect URI registered with the IdP, e.g.
    /// `https://<host>/api/auth/oidc/callback`.
    pub oidc_redirect_uri: Option<String>,
    pub oidc_scopes: String,
    /// Stored as `user_identities.provider` for OIDC-authenticated users.
    /// Override if fronting a non-Entra OIDC provider.
    pub oidc_provider_label: String,
    pub router_shortlist_threshold: usize,
    pub router_shortlist_size: usize,
    pub max_router_history_messages: usize,
    /// OpenAI-compatible model used for Stage 1 vector embeddings.
    /// Default: `text-embedding-3-small`. Stage 1 is skipped if `openai_api_key` is unset.
    pub embedding_model: String,
    pub router_agent_timeout_secs: u64,
    pub github_callback_url: Option<String>,
    /// Base URL to redirect to after a successful OAuth login. In production
    /// this is the same origin as the server. Override via `APP_BASE_URL` in
    /// dev when the server and app run on different ports.
    pub app_base_url: String,
    pub git_clone_allowed_hosts: Vec<String>,
    /// Allowed OCI registry hosts for `POST /api/catalog/import/registry`.
    /// Comma-separated.  Empty = reject all registry imports (safest default for
    /// new deployments).  Example: "ghcr.io,quay.io,registry.nasiko.dev"
    pub registry_import_allowed_hosts: Vec<String>,
    /// Browser origins allowed to make cross-origin requests (comma-separated,
    /// e.g. "https://app.example.com,http://localhost:5173"). Empty (the
    /// default) allows none — the UI is served same-origin by this binary's
    /// own static handler in normal deployments, so cross-origin access is
    /// opt-in only for split dev servers or external integrations.
    pub cors_allowed_origins: Vec<String>,
    pub admin_username: String,
    pub admin_password: String,
    /// Docker network to attach agent containers to.
    /// Set to the compose network name (e.g. `nasiko-cloud-rs_default`) when the
    /// server itself runs inside Docker so agents are reachable via container IP.
    pub docker_agent_network: Option<String>,
    /// OCI registry host to pull agent images from (e.g. `"localhost:8443"`).
    /// When set, the Docker runtime pulls images from this registry before creating containers.
    /// Maps to env var `OCI_REGISTRY_HOST`.
    pub oci_registry_host: Option<String>,
    /// Poll interval in seconds for the container-hours meter. 0 disables metering.
    pub container_hours_poll_secs: u64,

    // ─── MCP Gateway ────────────────────────────────────────────────────────
    /// Composio platform API key. When unset, Composio integration is disabled
    /// (generic MCP servers still work).
    pub composio_api_key: Option<String>,
    /// Composio v3 HTTP API base URL.
    pub composio_base_url: String,
    /// HMAC secret used to verify inbound Composio webhooks. When unset,
    /// signature verification is skipped (dev only).
    pub composio_webhook_secret: Option<String>,
    /// Public URL of the MCP gateway, injected into every deployed agent as
    /// `MCP_GATEWAY_URL`. When unset, no MCP env is injected at deploy time.
    pub mcp_gateway_public_url: Option<String>,
    /// TTL (seconds) for the Redis-cached resolved backend/session list.
    pub mcp_session_ttl_seconds: u64,
    /// TTL (seconds) for the Redis-cached per-agent permission context.
    pub mcp_perm_cache_ttl_seconds: u64,
    /// TTL (seconds) for the Redis-cached aggregated tool manifest.
    pub mcp_manifest_ttl_seconds: u64,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            bind: env_or("CP_BIND", "0.0.0.0:8080"),
            domain: std::env::var("CP_DOMAIN").ok(),
            database_url: required_env("DATABASE_URL")?,
            redis_url: required_env("REDIS_URL")?,
            agent_runtime: env_or("AGENT_RUNTIME", "local"),
            k8s_namespace: env_or("K8S_NAMESPACE", "nasiko-agents"),
            kubeconfig: std::env::var("KUBECONFIG").ok().filter(|s| !s.is_empty()),
            s3_endpoint: env_or("S3_ENDPOINT", "http://localhost:9000"),
            s3_bucket: env_or("S3_BUCKET", "nasiko"),
            s3_access_key: env_or("S3_ACCESS_KEY", "nasiko"),
            s3_secret_key: required_env("S3_SECRET_KEY")?,
            s3_region: env_or("S3_REGION", "us-east-1"),
            secrets_encryption_key: required_env("SECRETS_ENCRYPTION_KEY")?,
            oci_storage_bucket: env_or("OCI_STORAGE_BUCKET", "nasiko-artifacts"),
            agent_image_registry: env_or("AGENT_IMAGE_REGISTRY", ""),
            build_push_token: env_or("BUILD_PUSH_TOKEN", ""),
            seed_agents: std::env::var("SEED_AGENTS").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            openai_base_url: std::env::var("OPENAI_BASE_URL").ok(),
            openai_model: env_or("OPENAI_MODEL", "gpt-4o-mini"),
            router_model: env_or("ROUTER_MODEL", "gpt-4o-mini"),
            capability_generator_model: env_or("CAPABILITY_GENERATOR_MODEL", "gpt-4o-mini"),
            a2a_discovery_url: std::env::var("A2A_DISCOVERY_URL").ok(),
            otel_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            otel_protocol: env_or("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
            otel_headers: std::env::var("OTEL_EXPORTER_OTLP_HEADERS").ok(),
            otel_service_name: env_or("OTEL_SERVICE_NAME", "nasiko-cp"),
            otel_sample_ratio: env_or("OTEL_TRACES_SAMPLER_ARG", "1.0"),
            otel_collector_endpoint: env_or(
                "OTEL_COLLECTOR_ENDPOINT",
                "http://otel-collector:4318",
            ),
            otel_capture_content: std::env::var(
                "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT",
            )
            .map(|v| v == "true")
            .unwrap_or(true),
            tempo_url: env_or(
                "TEMPO_URL",
                "http://tempo.nasiko-infra.svc.cluster.local:3200",
            ),
            loki_url: env_or(
                "LOKI_URL",
                "http://loki.nasiko-infra.svc.cluster.local:3100",
            ),
            // Enabled only when BOTH backends are explicitly configured; a
            // partial config is treated as disabled. Computed here, the one place
            // env is read, so every consumer agrees on whether it's enabled.
            observability_enabled: std::env::var("TEMPO_URL").is_ok_and(|v| !v.is_empty())
                && std::env::var("LOKI_URL").is_ok_and(|v| !v.is_empty()),
            flow_max_depth: env_parse("NASIKO_FLOW_MAX_DEPTH", 5),
            flow_max_fan_out: env_parse("NASIKO_FLOW_MAX_FAN_OUT", 20),
            flow_max_tokens: env_parse("NASIKO_FLOW_MAX_TOKENS", 100000),
            flow_timeout_secs: env_parse("NASIKO_FLOW_TIMEOUT_SECS", 120),
            github_client_id: std::env::var("GITHUB_CLIENT_ID").ok(),
            github_client_secret: std::env::var("GITHUB_CLIENT_SECRET").ok(),
            oidc_issuer_url: std::env::var("OIDC_ISSUER_URL").ok().filter(|s| !s.is_empty()),
            oidc_client_id: std::env::var("OIDC_CLIENT_ID").ok().filter(|s| !s.is_empty()),
            oidc_client_secret: std::env::var("OIDC_CLIENT_SECRET").ok().filter(|s| !s.is_empty()),
            oidc_redirect_uri: std::env::var("OIDC_REDIRECT_URI").ok().filter(|s| !s.is_empty()),
            oidc_scopes: env_or("OIDC_SCOPES", "openid profile email"),
            oidc_provider_label: env_or("OIDC_PROVIDER_LABEL", "microsoft_entra"),
            router_shortlist_threshold: env_parse("ROUTER_SHORTLIST_THRESHOLD", 15),
            router_shortlist_size: env_parse("ROUTER_SHORTLIST_SIZE", 10),
            max_router_history_messages: env_parse("MAX_ROUTER_HISTORY_MESSAGES", 20),
            embedding_model: env_or("EMBEDDING_MODEL", "text-embedding-3-small"),
            router_agent_timeout_secs: env_parse("ROUTER_AGENT_TIMEOUT_SECS", 60),
            github_callback_url: std::env::var("GITHUB_CALLBACK_URL").ok(),
            app_base_url: env_or("APP_BASE_URL", ""),
            docker_agent_network: std::env::var("DOCKER_AGENT_NETWORK").ok().filter(|s| !s.is_empty()),
            oci_registry_host: std::env::var("OCI_REGISTRY_HOST").ok().filter(|s| !s.is_empty()),
            container_hours_poll_secs: env_parse("CONTAINER_HOURS_POLL_SECS", 60),
            git_clone_allowed_hosts: std::env::var("GIT_CLONE_ALLOWED_HOSTS")
                .unwrap_or_else(|_| "github.com,gitlab.com,bitbucket.org".to_owned())
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            registry_import_allowed_hosts: std::env::var("REGISTRY_IMPORT_ALLOWED_HOSTS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            cors_allowed_origins: std::env::var("CORS_ALLOWED_ORIGINS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            admin_username: env_or("ADMIN_USERNAME", "admin"),
            admin_password: required_env("ADMIN_PASSWORD")?,

            composio_api_key: std::env::var("COMPOSIO_API_KEY").ok().filter(|s| !s.is_empty()),
            composio_base_url: env_or("COMPOSIO_BASE_URL", "https://backend.composio.dev"),
            composio_webhook_secret: std::env::var("COMPOSIO_WEBHOOK_SECRET")
                .ok()
                .filter(|s| !s.is_empty()),
            mcp_gateway_public_url: std::env::var("MCP_GATEWAY_PUBLIC_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            mcp_session_ttl_seconds: env_parse("MCP_SESSION_TTL_SECONDS", 300),
            mcp_perm_cache_ttl_seconds: env_parse("MCP_PERM_CACHE_TTL_SECONDS", 30),
            mcp_manifest_ttl_seconds: env_parse("MCP_MANIFEST_TTL_SECONDS", 300),
        })
    }

    /// Fail fast if `SECRETS_ENCRYPTION_KEY` can't actually be used to construct
    /// a `SecretsCrypto` (base64-decodes to exactly 32 bytes). Both the OSS
    /// HKDF-per-scope crypto and EE's `nasiko-secrets::SecretsCrypto::from_key`
    /// require this shape; previously an invalid key (e.g. 32 raw alphanumeric
    /// characters, which decode to only 24 bytes) passed config validation
    /// silently and only surfaced as a panic/error on the first secret
    /// encrypt/decrypt call, at request time, long after boot.
    pub fn validate_secrets_key(&self) -> Result<(), String> {
        validate_secrets_key_format(&self.secrets_encryption_key)
    }
}

fn validate_secrets_key_format(key: &str) -> Result<(), String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(key)
        .map_err(|e| format!("SECRETS_ENCRYPTION_KEY is not valid base64: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "SECRETS_ENCRYPTION_KEY must decode to exactly 32 bytes, got {} — expected base64(32 random bytes)",
            bytes.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_32_byte_base64_key_passes() {
        assert!(validate_secrets_key_format("QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=").is_ok());
    }

    #[test]
    fn raw_32_char_alphanumeric_key_fails() {
        // The original bug: 32 raw characters decode to only 24 bytes.
        assert!(validate_secrets_key_format("dev-only-change-in-prod-32chars!!").is_err());
    }

    #[test]
    fn wrong_byte_length_after_decode_fails() {
        use base64::Engine;
        let sixteen_bytes = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        assert!(validate_secrets_key_format(&sixteen_bytes).is_err());
    }

    #[test]
    fn invalid_base64_fails() {
        assert!(validate_secrets_key_format("not base64 at all!!!").is_err());
    }
}
