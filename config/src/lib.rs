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
    /// TODO: this needs to be removed.
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
    /// Model for the MCP-connector description LLM fallback — only called when
    /// a connector/tool description couldn't be fetched from its native source.
    pub mcp_description_model: String,
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
    /// Opaque identifier for the tenant this deployment belongs to, added as
    /// a `tenant.id` OTel resource attribute on every agent this instance
    /// deploys — see `InstrumentedRuntime`. `None` for a standalone/non-
    /// multi-tenant deployment. This crate has no notion of what a "tenant"
    /// is; it only passes the value through.
    pub tenant_id: Option<String>,
    pub flow_max_depth: i32,
    pub flow_max_fan_out: i32,
    pub flow_max_tokens: i64,
    pub flow_timeout_secs: i32,
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
    /// OIDC issuer authority, e.g. `https://login.microsoftonline.com/<tenant-id>/v2.0`
    /// for Microsoft Entra ID — or any other OIDC-compliant provider. `None`
    /// disables OIDC login entirely (see the enterprise OIDC SSO guide).
    pub oidc_issuer_url: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_client_secret: Option<String>,
    /// Must exactly match the redirect URI registered with the IdP, e.g.
    /// `https://<host>/api/auth/oidc/callback`.
    pub oidc_redirect_uri: Option<String>,
    /// Full origins (`scheme://host[:port]`) a post-login OIDC `redirect`
    /// target is allowed to point at, in addition to a same-origin relative
    /// path — needed when the frontend is a separate deployment on its own
    /// domain rather than this binary's embedded UI (comma-separated, e.g.
    /// `"https://app.example.com,http://localhost:5173"`). Empty (the
    /// default) means only same-origin relative paths are accepted; see
    /// `ee/server/src/auth.rs::is_safe_redirect_target`.
    pub oidc_allowed_redirect_origins: Vec<String>,
    pub oidc_scopes: String,
    /// Stored as `user_identities.provider` for OIDC-authenticated users.
    /// Override if fronting a non-Entra OIDC provider.
    pub oidc_provider_label: String,
    /// Multi-tenant mode (per-CP): when on, this control plane runs behind the
    /// multi-tenant BFF — it serves no UI (root 302s to the BFF) and enforces
    /// the corporate-only admission gate below. Default off = ordinary
    /// single-tenant behavior, unchanged.
    pub multi_tenant_mode: bool,
    /// Only consulted when `multi_tenant_mode` is on. Off (the default)
    /// restricts logins to corporate identities (a Google `hd`, or a verified
    /// email whose domain isn't a known personal provider). On also admits
    /// personal emails, which may only ever *join* a workspace, never create
    /// one. No effect outside multi-tenant mode.
    pub allow_personal_emails: bool,
    /// Base URL of the multi-tenant BFF/dashboard. Used only in
    /// `multi_tenant_mode`: this headless control plane serves no UI, so browser
    /// navigations are redirected here. `None` (the default) outside
    /// multi-tenant mode.
    pub nasiko_bff_url: Option<String>,
    pub router_shortlist_threshold: usize,
    pub router_shortlist_size: usize,
    pub max_router_history_messages: usize,
    /// OpenAI-compatible model used for Stage 1 vector embeddings.
    /// Default: `text-embedding-3-small`. Stage 1 is skipped if `openai_api_key` is unset.
    pub embedding_model: String,
    pub router_agent_timeout_secs: u64,
    pub github_callback_url: Option<String>,
    /// Central OAuth callback relay URL (multi-tenant deployments): used as the
    /// GitHub `redirect_uri` for both authorize and token exchange instead of
    /// `github_callback_url`, so many clusters can share one GitHub OAuth app
    /// whose single registered callback points at the relay. Includes this
    /// cluster's tenant-id path suffix. Unset (the default, and always for
    /// standalone deployments) means GitHub calls this cluster back directly.
    pub github_central_callback_url: Option<String>,
    /// The OIDC analogue of [`Self::github_central_callback_url`]: the fleet
    /// relay callback used as the OIDC `redirect_uri` for both authorize and
    /// token exchange (multi-tenant workspace CPs), so many clusters share one
    /// Google/OIDC app whose single registered callback points at the relay.
    /// Includes this cluster's tenant-id path suffix. Unset (default, and always
    /// standalone) means the IdP calls this cluster back directly.
    pub oidc_central_callback_url: Option<String>,
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
    /// Base URL for the generic-connector OAuth 2.1 browser redirect
    /// (`{base}/oauth/callback`), distinct from `mcp_gateway_public_url` on
    /// purpose: that value is told to agent *containers* (may be a
    /// Docker-internal address like `host.docker.internal`, meaningless to a
    /// browser or a real OAuth provider's redirect-uri validation —
    /// confirmed live: Notion's DCR endpoint rejects it with "Redirect URI
    /// must use HTTPS unless it is a loopback HTTP URI"). This one is opened
    /// in the *user's own browser*, so it needs to satisfy that requirement
    /// instead. Falls back to `mcp_gateway_public_url` when unset, which is
    /// correct in production (a real HTTPS domain satisfies both audiences)
    /// but not for local dev with a Docker-only `MCP_GATEWAY_PUBLIC_URL`.
    pub mcp_oauth_redirect_base_url: Option<String>,
    /// Same browser-reachable-redirect problem as `mcp_oauth_redirect_base_url`
    /// above, but for the separate Composio OAuth connect flow
    /// (`oss/mcp-gateway/src/connect.rs`). COMPOSIO_CALLBACK_BASE_URL, optional.
    pub composio_callback_base_url: Option<String>,
    /// TTL (seconds) for the Redis-cached resolved backend/session list.
    pub mcp_session_ttl_seconds: u64,
    /// TTL (seconds) for the Redis-cached per-agent permission context.
    pub mcp_perm_cache_ttl_seconds: u64,
    /// TTL (seconds) for the Redis-cached aggregated tool manifest.
    pub mcp_manifest_ttl_seconds: u64,
    /// Max upload size for a user's own MCP server zip. MCP_UPLOAD_MAX_BYTES,
    /// default 50 MiB — deliberately smaller than agents' 100 MiB default,
    /// since MCP servers are typically much smaller than full agent codebases.
    pub mcp_upload_max_bytes: u64,
    /// Port an uploaded MCP server container is expected to bind via `$PORT`.
    /// MCP_UPLOAD_DEFAULT_PORT, default 8080.
    pub mcp_upload_default_port: u16,
    /// Docker network uploaded MCP server containers are deployed onto,
    /// isolated from the default network (DB/Redis/agents). MCP_SERVERS_NETWORK,
    /// default "nasiko-mcp-servers-net" (the server's own compose config must
    /// also join this network — see docker-compose.infra.yml).
    pub mcp_servers_network: String,
    /// Maximum replica count for uploaded MCP server pods under Kubernetes
    /// (KEDA ScaledObject). MCP_UPLOAD_MAX_REPLICAS, default 1 (matches
    /// agents; set higher when KEDA is installed). Ignored by DockerRuntime.
    pub mcp_upload_max_replicas: u32,
    /// Maximum replica count for deployed agent pods under Kubernetes (KEDA
    /// ScaledObject) — same mechanism and same global-ceiling shape as
    /// `mcp_upload_max_replicas` above, just for regular agents instead of
    /// MCP connectors. AGENT_MAX_REPLICAS, default 1 (no autoscaling unless
    /// explicitly raised). Ignored by DockerRuntime.
    pub agent_max_replicas: u32,
    /// TTL (seconds) for the Redis-cached Composio toolkit tool count shown on
    /// unconnected catalog cards — changes rarely, so a much longer TTL than
    /// the permission/session caches.
    pub mcp_toolcount_ttl_seconds: u64,
    /// Comma-separated Composio toolkit names to auto-register at first boot.
    /// SEED_TOOLKITS, default empty. Requires COMPOSIO_API_KEY to be set.
    pub seed_toolkits: Vec<String>,
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
            mcp_description_model: env_or("MCP_DESCRIPTION_MODEL", "gpt-4o-mini"),
            a2a_discovery_url: std::env::var("A2A_DISCOVERY_URL").ok(),
            otel_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            otel_protocol: env_or("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
            otel_headers: std::env::var("OTEL_EXPORTER_OTLP_HEADERS").ok(),
            otel_service_name: env_or("OTEL_SERVICE_NAME", "nasiko-cp"),
            otel_sample_ratio: env_or("OTEL_TRACES_SAMPLER_ARG", "1.0"),
            // Port 4317 (OTLP gRPC), not 4318 (OTLP HTTP), because this endpoint is
            // injected into agents alongside `otel_protocol`, which defaults to
            // "grpc" — the pairing has to agree or every agent's exporter speaks
            // gRPC at an HTTP port and silently fails to export. 4317+grpc is also
            // the conventional OTLP default pairing.
            otel_collector_endpoint: env_or(
                "OTEL_COLLECTOR_ENDPOINT",
                "http://otel-collector:4317",
            ),
            otel_capture_content: std::env::var(
                "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT",
            )
            .map(|v| v == "true")
            .unwrap_or(true),
            tempo_url: env_or("TEMPO_URL", ""),
            loki_url: env_or("LOKI_URL", ""),
            // Enabled only when BOTH backends are explicitly configured; a
            // partial config is treated as disabled. Computed here, the one place
            // env is read, so every consumer agrees on whether it's enabled.
            observability_enabled: std::env::var("TEMPO_URL").is_ok_and(|v| !v.is_empty())
                && std::env::var("LOKI_URL").is_ok_and(|v| !v.is_empty()),
            tenant_id: std::env::var("TENANT_ID").ok(),
            flow_max_depth: env_parse("NASIKO_FLOW_MAX_DEPTH", 5),
            flow_max_fan_out: env_parse("NASIKO_FLOW_MAX_FAN_OUT", 20),
            flow_max_tokens: env_parse("NASIKO_FLOW_MAX_TOKENS", 100000),
            flow_timeout_secs: env_parse("NASIKO_FLOW_TIMEOUT_SECS", 120),
            github_client_id: std::env::var("GITHUB_CLIENT_ID").ok(),
            github_client_secret: std::env::var("GITHUB_CLIENT_SECRET").ok(),
            oidc_issuer_url: std::env::var("OIDC_ISSUER_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            oidc_client_id: std::env::var("OIDC_CLIENT_ID")
                .ok()
                .filter(|s| !s.is_empty()),
            oidc_client_secret: std::env::var("OIDC_CLIENT_SECRET")
                .ok()
                .filter(|s| !s.is_empty()),
            oidc_redirect_uri: std::env::var("OIDC_REDIRECT_URI")
                .ok()
                .filter(|s| !s.is_empty()),
            oidc_allowed_redirect_origins: std::env::var("OIDC_ALLOWED_REDIRECT_ORIGINS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            oidc_scopes: env_or("OIDC_SCOPES", "openid profile email"),
            oidc_provider_label: env_or("OIDC_PROVIDER_LABEL", "microsoft_entra"),
            multi_tenant_mode: std::env::var("MULTI_TENANT_MODE")
                .map(|v| v == "true")
                .unwrap_or(false),
            allow_personal_emails: std::env::var("ALLOW_PERSONAL_EMAILS")
                .map(|v| v == "true")
                .unwrap_or(false),
            nasiko_bff_url: std::env::var("NASIKO_BFF_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            router_shortlist_threshold: env_parse("ROUTER_SHORTLIST_THRESHOLD", 15),
            router_shortlist_size: env_parse("ROUTER_SHORTLIST_SIZE", 10),
            max_router_history_messages: env_parse("MAX_ROUTER_HISTORY_MESSAGES", 20),
            embedding_model: env_or("EMBEDDING_MODEL", "text-embedding-3-small"),
            router_agent_timeout_secs: env_parse("ROUTER_AGENT_TIMEOUT_SECS", 60),
            github_callback_url: std::env::var("GITHUB_CALLBACK_URL").ok(),
            github_central_callback_url: std::env::var("GITHUB_CENTRAL_CALLBACK_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            oidc_central_callback_url: std::env::var("OIDC_CENTRAL_CALLBACK_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            app_base_url: env_or("APP_BASE_URL", ""),
            docker_agent_network: std::env::var("DOCKER_AGENT_NETWORK")
                .ok()
                .filter(|s| !s.is_empty()),
            oci_registry_host: std::env::var("OCI_REGISTRY_HOST")
                .ok()
                .filter(|s| !s.is_empty()),
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

            composio_api_key: std::env::var("COMPOSIO_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            composio_base_url: env_or("COMPOSIO_BASE_URL", "https://backend.composio.dev"),
            composio_webhook_secret: std::env::var("COMPOSIO_WEBHOOK_SECRET")
                .ok()
                .filter(|s| !s.is_empty()),
            mcp_gateway_public_url: std::env::var("MCP_GATEWAY_PUBLIC_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            mcp_oauth_redirect_base_url: std::env::var("MCP_OAUTH_REDIRECT_BASE_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            composio_callback_base_url: std::env::var("COMPOSIO_CALLBACK_BASE_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            mcp_session_ttl_seconds: env_parse("MCP_SESSION_TTL_SECONDS", 300),
            mcp_perm_cache_ttl_seconds: env_parse("MCP_PERM_CACHE_TTL_SECONDS", 30),
            mcp_manifest_ttl_seconds: env_parse("MCP_MANIFEST_TTL_SECONDS", 300),
            mcp_upload_max_bytes: env_parse("MCP_UPLOAD_MAX_BYTES", 50 * 1024 * 1024),
            mcp_upload_default_port: env_parse("MCP_UPLOAD_DEFAULT_PORT", 8080),
            mcp_servers_network: env_or("MCP_SERVERS_NETWORK", "nasiko-mcp-servers-net"),
            mcp_upload_max_replicas: env_parse("MCP_UPLOAD_MAX_REPLICAS", 1),
            agent_max_replicas: env_parse("AGENT_MAX_REPLICAS", 1),
            mcp_toolcount_ttl_seconds: env_parse("MCP_TOOLCOUNT_TTL_SECONDS", 3600),
            seed_toolkits: std::env::var("SEED_TOOLKITS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
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

/// Strips a trailing `/v1` (and any trailing slashes) from an OpenAI-compatible
/// base URL, for callers that append their own `/v1/...` path segment.
///
/// `OPENAI_BASE_URL` is commonly written *with* the `/v1` — that is how
/// `cp.nasiko.dev` and `ee/server/.env` have it — so appending `/v1/whatever`
/// to the raw value doubles up into `.../v1/v1/whatever`, which 404s.
///
/// Deliberately a free function rather than normalization applied to
/// [`Config::openai_base_url`] itself: `ee/artifact-registry` uses the opposite
/// convention (base URL *includes* `/v1`, it appends bare `/embeddings`), so
/// the stored value has to stay verbatim.
pub fn openai_base_url_without_v1(base_url: &str) -> &str {
    let trimmed = base_url.trim_end_matches('/');
    trimmed.strip_suffix("/v1").unwrap_or(trimmed)
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
        assert!(
            validate_secrets_key_format("QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=").is_ok()
        );
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

    #[test]
    fn base_url_written_with_v1_is_stripped() {
        // The cp.nasiko.dev form — appending `/v1/audio/transcriptions` to the
        // raw value produced `.../v1/v1/audio/transcriptions` and 404'd.
        assert_eq!(
            openai_base_url_without_v1("https://api.openai.com/v1"),
            "https://api.openai.com"
        );
        assert_eq!(
            openai_base_url_without_v1("https://api.openai.com/v1/"),
            "https://api.openai.com"
        );
    }

    #[test]
    fn base_url_written_without_v1_is_unchanged() {
        assert_eq!(
            openai_base_url_without_v1("https://api.deepseek.com"),
            "https://api.deepseek.com"
        );
        assert_eq!(
            openai_base_url_without_v1("http://localhost:11434/"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn only_a_trailing_v1_segment_is_stripped() {
        // A host or path that merely contains "v1" must survive intact.
        assert_eq!(
            openai_base_url_without_v1("https://v1.example.com"),
            "https://v1.example.com"
        );
        assert_eq!(
            openai_base_url_without_v1("https://example.com/openai/v1/proxy"),
            "https://example.com/openai/v1/proxy"
        );
    }
}
