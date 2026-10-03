use nasiko_utils::{env_bool, env_or, env_parse, required_env};

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub domain: Option<String>,
    pub database_url: String,
    pub redis_url: String,
    pub agent_runtime: String,
    pub k8s_namespace: String,
    pub kubeconfig: Option<String>,
    /// Which object-storage protocol backs artifacts: `s3` (default; any
    /// S3-compatible store) or `azure-blob`. Carried here as an opaque string
    /// and interpreted by the composition root that selects the backend — the
    /// same arrangement as `agent_runtime`, whose `kubernetes` value only one
    /// edition can serve. Held on `Config` so the value is visible to every
    /// consumer without a second env read.
    pub storage_provider: String,
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
    /// Username for authenticating agent-image pulls from a private registry
    /// (e.g. a private Docker Hub repo). `None` (default) means anonymous
    /// pulls only — unchanged behavior for public images. Only meaningful
    /// together with `agent_registry_password`; `DockerRuntime` treats a
    /// pair where only one is set as "not configured."
    pub agent_registry_username: Option<String>,
    /// Password or access token paired with `agent_registry_username`. Never
    /// logged, never returned in any API response.
    pub agent_registry_password: Option<String>,
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
    /// MAF "decompose one instruction into atomic sub-queries" service.
    /// `None` disables `/maf/workflow/from-instruction` (503).
    pub decomposer_api_url: Option<String>,
    pub decomposer_api_key: Option<String>,
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
    /// OTLP/HTTP JSON base endpoint used only by the durable coding-agent
    /// telemetry outbox. Unset leaves receipts pending and disables its worker.
    pub coding_agent_otlp_endpoint: Option<String>,
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
    /// When true, a background loop refreshes `provider_models` from each
    /// configured provider's `GET /models`. Reaches the network at boot, so
    /// tests and benches turn it off; a disabled sync just means tier routing
    /// falls back to whatever catalog rows already exist.
    pub model_catalog_sync_enabled: bool,
    /// When true, a background loop mirrors the Portkey price book into
    /// `model_pricing`. Reaches the network at boot, so tests and benches turn
    /// it off; a disabled sync leaves the boot seed rows as the only pricing.
    /// Also the switch for air-gapped installs that must not call out.
    pub model_pricing_sync_enabled: bool,
    pub flow_max_depth: i32,
    pub flow_max_fan_out: i32,
    pub flow_max_tokens: i64,
    /// Wall-clock budget for one flow, from `NASIKO_FLOW_TIMEOUT_SECS`. The
    /// platform's widest window: the flow guard enforces it, and both the MCP
    /// gateway (`tools/call`) and the LLM router (token attribution) refuse to
    /// serve a flow older than this, so nothing an agent turn depends on may
    /// outlive it.
    pub flow_timeout_secs: i32,
    /// How long a HITL pause (`hitl_requests`) stays answerable before the dispatcher's poll
    /// loop expires it. `oss/hitl`'s own store applies this at row-creation time — see
    /// `PgHitlStore::with_ttl_days`.
    pub hitl_request_ttl_days: i64,
    /// `nasiko_hitl::dispatcher::DispatcherConfig`'s remaining tunables (the `mcp_tool`-origin
    /// resume dispatcher, `oss/hitl/src/dispatcher.rs`) — every comparable tunable elsewhere in
    /// this codebase goes through this single `Config` struct, and `hitl_request_ttl_days` right
    /// above is the same feature's own TTL knob, so these were the odd ones out as compile-time
    /// constants (found in review).
    pub hitl_resume_poll_interval_secs: u64,
    pub hitl_resume_recovery_interval_secs: u64,
    /// Claim-lease floor shared by *both* resume dispatchers — `nasiko_hitl::dispatcher`'s
    /// `mcp_tool` one (via `DispatcherConfig::effective_lease_minutes`, which holds one claim
    /// across its own in-process retry loop) and `oss/server/src/hitl/mod.rs`'s `direct_chat`/
    /// `agent_proxy`/`orchestrator`/`maf` one (via its `lease_secs` helper, which claims once per
    /// delivery attempt). Each dispatcher floors its own effective lease at what its own delivery
    /// shape needs, so raising or lowering this one knob can never reopen either's
    /// double-delivery window.
    pub hitl_resume_lease_minutes: i64,
    pub hitl_resume_max_attempts: u32,
    pub hitl_resume_retry_delay_secs: u64,
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
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
    /// How many of the most recent chat messages the PACMS context selector
    /// draws candidates from (a wide pool for the selector to choose a
    /// budget-fitting subset from). See `SessionHistory::fetch_pacms`.
    pub pacms_history_pool_size: usize,
    /// Structurally compress tool results as the ReAct loop stores them
    /// (PRD §9 IP-3). Shrinks what the loop carries, which also defers the
    /// context-compaction cliff. On by default — gated by the agent's own
    /// switch, so this is a fleet kill switch rather than an enabler.
    /// How often the brevity holdout is re-analysed into a measured effect factor.
    pub savings_factor_refresh_secs: u64,
    /// Minimum samples **per arm** before a measured factor replaces the seeded one. Below this
    /// the arm means are noise, and a noisy `fixture` figure is worse than an honest seed.
    pub savings_factor_min_samples: i64,
    /// Trailing window the holdout comparison reads.
    pub savings_factor_window_days: i64,
    pub react_compress_enabled: bool,
    /// Skip tool results below this size.
    pub react_compress_min_bytes: usize,
    /// Structurally compress each history message before context selection
    /// (PRD §9 IP-4). On by default — gated by the agent's own switch, so this
    /// is a fleet kill switch rather than an enabler.
    pub history_compress_enabled: bool,
    /// Skip history messages below this size. A short turn is mostly prose,
    /// which does not compress, so the attempt is pure cost.
    pub history_compress_min_bytes: usize,
    /// Token budget for a user on the PACMS "low" tier (`users.pacms_budget_level`).
    pub pacms_budget_low: usize,
    /// Token budget for a user on the PACMS "medium" tier — the default tier
    /// for a user who hasn't picked one.
    pub pacms_budget_medium: usize,
    /// Token budget for a user on the PACMS "high" tier.
    pub pacms_budget_high: usize,
    /// How many of the most-recent messages in the pool are force-included
    /// (PACMS `mandatory` set) regardless of relevance/coverage score, so the
    /// immediate conversational thread is never dropped.
    pub pacms_history_mandatory_recent: usize,
    /// Item count for a user on the "low" tier (`users.pacms_budget_level`),
    /// shared by the `topk` strategy's query/answer-pair count
    /// (`SessionHistory::fetch_topk`) and the `lastk` strategy's recency
    /// window (`SessionHistory::fetch`) — same tier the PACMS token budget
    /// above reads, resolved via `PacmsBudgetLevel::k`.
    pub context_k_low: usize,
    /// Item count for a user on the "medium" tier — the default tier for a
    /// user who hasn't picked one.
    pub context_k_medium: usize,
    /// Item count for a user on the "high" tier.
    pub context_k_high: usize,
    /// OpenAI-compatible model used for Stage 1 vector embeddings.
    /// Default: `text-embedding-3-small`. Stage 1 is skipped if `openai_api_key` is unset.
    pub embedding_model: String,
    /// Wall-clock budget for a single agent HTTP hop — the A2A proxy, the
    /// orchestrator's streaming and non-streaming agent calls, and the MAF
    /// executor's. An agent turn can legitimately run for minutes (long tool
    /// calls, multi-step orchestration), so this is deliberately far above the
    /// shared `http_client` default, which stays short for embeddings, registry
    /// probes and OAuth. Read from `AGENT_CALL_TIMEOUT_SECS`, falling back to
    /// the former `ROUTER_AGENT_TIMEOUT_SECS`.
    pub agent_call_timeout_secs: u64,
    pub github_callback_url: Option<String>,
    /// Central OAuth callback relay URL (multi-tenant deployments): used as the
    /// GitHub `redirect_uri` for both authorize and token exchange instead of
    /// `github_callback_url`, so many clusters can share one GitHub OAuth app
    /// whose single registered callback points at the relay. Includes this
    /// cluster's tenant-id path suffix. Unset (the default, and always for
    /// standalone deployments) means GitHub calls this cluster back directly.
    pub github_central_callback_url: Option<String>,
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
    // ─── OIDC SSO ───────────────────────────────────────────────────────────
    pub oidc_issuer_url: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_client_secret: Option<String>,
    pub oidc_redirect_uri: Option<String>,
    pub oidc_allowed_redirect_origins: Vec<String>,
    pub oidc_scopes: String,
    pub oidc_provider_label: String,
    pub oidc_central_callback_url: Option<String>,

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

    // ─── Trace Materializer ─────────────────────────────────────────────────
    /// Poll interval in seconds for the trace-usage materializer. 0 disables.
    pub trace_usage_sync_secs: u64,
    /// Overlap window in seconds: the materializer re-queries this far before
    /// the high-water mark to catch late-arriving spans and pricing corrections.
    pub trace_usage_overlap_secs: u64,
    /// Max traces to fetch from Tempo per concurrent batch.
    pub trace_usage_batch_size: usize,

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
    /// Number of agent/connector image builds the build worker runs
    /// concurrently. BUILD_CONCURRENCY, default 4. Clamped to 1..=16: each
    /// in-flight build is a `docker build` on this host (OSS) or a Kubernetes
    /// Job against one buildkitd (EE), so an unbounded value is a footgun, not
    /// a feature. Set 1 to restore the old strictly-serial behavior.
    ///
    /// Two builds for the *same* agent or connector never overlap regardless of
    /// this value — the claim query skips a target that already has a build in
    /// flight (see `build_worker::claim_next_job`).
    pub build_concurrency: usize,
    /// Default memory limit for every agent container, Kubernetes notation
    /// (`"512Mi"`, `"1Gi"`) — see `nasiko_runtime::ResourceLimits::memory`.
    /// AGENT_DEFAULT_MEMORY, default `"1Gi"`. `nasiko_runtime::ResourceLimits`
    /// itself still defaults to `"512Mi"` (its own hermetic fallback for
    /// callers outside the server, e.g. tests) — this is the value the server
    /// actually uses for every agent deploy unless a future per-agent
    /// override is added. Raised from the runtime crate's original 512Mi
    /// after opencode (3 concurrent Node/Bun processes) was repeatedly
    /// OOM-killed under real chat load at that limit.
    pub agent_default_memory: String,
    /// Name of the single named Docker volume every `--writable` agent shares
    /// (each mounted at its own `volume-subpath`) — see
    /// `nasiko_runtime::DockerRuntimeConfig::agent_memory_volume`.
    /// AGENT_MEMORY_VOLUME, default `"nasiko-agent-memory"`.
    pub agent_memory_volume: String,
    /// Image for the short-lived helper container that pre-creates a
    /// `--writable` agent's subdirectory inside `agent_memory_volume` — see
    /// `nasiko_runtime::DockerRuntimeConfig::agent_memory_init_image`.
    /// AGENT_MEMORY_INIT_IMAGE, default `"alpine:3.21"` (override for
    /// air-gapped or internal-mirror setups).
    pub agent_memory_init_image: String,
    /// TTL (seconds) for the Redis-cached Composio toolkit tool count shown on
    /// unconnected catalog cards — changes rarely, so a much longer TTL than
    /// the permission/session caches.
    pub mcp_toolcount_ttl_seconds: u64,
    /// Comma-separated Composio toolkit names to auto-register at first boot.
    /// SEED_TOOLKITS, default empty. Requires COMPOSIO_API_KEY to be set.
    pub seed_toolkits: Vec<String>,
    /// MCP tool search mode: `semantic` (embedding cosine, default), `keyword`
    /// (BM25 fallback), or `none` (eager fan-out, no search — rollback).
    pub mcp_tool_search_mode: String,
    /// Max tools returned by query-aware `tools/list`.
    pub mcp_tool_search_tool_limit: usize,
    /// Max tools returned by the `nasiko_search_tools` meta-tool.
    pub mcp_tool_search_meta_limit: usize,
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
            storage_provider: env_or("STORAGE_PROVIDER", "s3"),
            s3_endpoint: env_or("S3_ENDPOINT", "http://localhost:9000"),
            s3_bucket: env_or("S3_BUCKET", "nasiko"),
            s3_access_key: env_or("S3_ACCESS_KEY", "nasiko"),
            // Required only for an S3 backend. An Azure Blob deployment holds
            // no S3 credential at all, and demanding one there turned a
            // correct config into a startup failure — so the requirement
            // follows the selected provider rather than being unconditional.
            s3_secret_key: if uses_s3_storage(&env_or("STORAGE_PROVIDER", "s3")) {
                required_env("S3_SECRET_KEY")?
            } else {
                String::new()
            },
            s3_region: env_or("S3_REGION", "us-east-1"),
            secrets_encryption_key: required_env("SECRETS_ENCRYPTION_KEY")?,
            oci_storage_bucket: env_or("OCI_STORAGE_BUCKET", "nasiko-artifacts"),
            agent_image_registry: env_or("AGENT_IMAGE_REGISTRY", ""),
            agent_registry_username: std::env::var("AGENT_REGISTRY_USERNAME")
                .ok()
                .filter(|s| !s.is_empty()),
            agent_registry_password: std::env::var("AGENT_REGISTRY_PASSWORD")
                .ok()
                .filter(|s| !s.is_empty()),
            build_push_token: env_or("BUILD_PUSH_TOKEN", ""),
            seed_agents: std::env::var("SEED_AGENTS").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            openai_base_url: std::env::var("OPENAI_BASE_URL").ok(),
            openai_model: env_or("OPENAI_MODEL", "gpt-4o-mini"),
            decomposer_api_url: std::env::var("MODEL_API_URL").ok(),
            decomposer_api_key: std::env::var("MODEL_APIKEY").ok(),
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
            coding_agent_otlp_endpoint: std::env::var("CODING_AGENT_OTLP_ENDPOINT")
                .ok()
                .map(|value| value.trim_end_matches('/').to_owned())
                .filter(|value| !value.is_empty()),
            tempo_url: env_or("TEMPO_URL", ""),
            loki_url: env_or("LOKI_URL", ""),
            // Enabled only when BOTH backends are explicitly configured; a
            // partial config is treated as disabled. Computed here, the one place
            // env is read, so every consumer agrees on whether it's enabled.
            observability_enabled: std::env::var("TEMPO_URL").is_ok_and(|v| !v.is_empty())
                && std::env::var("LOKI_URL").is_ok_and(|v| !v.is_empty()),
            tenant_id: std::env::var("TENANT_ID").ok(),
            model_catalog_sync_enabled: env_bool("MODEL_CATALOG_SYNC_ENABLED", true),
            model_pricing_sync_enabled: env_bool("MODEL_PRICING_SYNC_ENABLED", true),
            flow_max_depth: env_parse("NASIKO_FLOW_MAX_DEPTH", 5),
            flow_max_fan_out: env_parse("NASIKO_FLOW_MAX_FAN_OUT", 20),
            flow_max_tokens: env_parse("NASIKO_FLOW_MAX_TOKENS", 100000),
            // Keep in step with `nasiko_flow::DEFAULT_FLOW_TIMEOUT_SECS` (this
            // crate is a leaf and can't reference it): an agent turn may run
            // the full `agent_call_timeout_secs`, so the flow that authorizes
            // it has to live at least as long.
            flow_timeout_secs: env_parse("NASIKO_FLOW_TIMEOUT_SECS", 600),
            hitl_request_ttl_days: env_parse("HITL_REQUEST_TTL_DAYS", 7),
            // Defaults match `nasiko_hitl::dispatcher::DispatcherConfig::default()` exactly, so
            // an unset env var changes nothing.
            hitl_resume_poll_interval_secs: env_parse("HITL_RESUME_POLL_INTERVAL_SECS", 5),
            hitl_resume_recovery_interval_secs: env_parse(
                "HITL_RESUME_RECOVERY_INTERVAL_SECS",
                10 * 60,
            ),
            // Must outlast one whole delivery, not one request: the MCP resume dispatcher holds
            // its claim across every in-process retry (3 attempts x the notifier's 300s timeout
            // + backoff ~= 15 min). At the old default of 2 the recovery sweep quarantined
            // deliveries that were still in flight. `DispatcherConfig::effective_lease_minutes`
            // enforces the floor regardless, so this default only keeps the two in agreement.
            hitl_resume_lease_minutes: env_parse("HITL_RESUME_LEASE_MINUTES", 16),
            hitl_resume_max_attempts: env_parse("HITL_RESUME_MAX_ATTEMPTS", 3),
            hitl_resume_retry_delay_secs: env_parse("HITL_RESUME_RETRY_DELAY_SECS", 2),
            github_client_id: std::env::var("GITHUB_CLIENT_ID").ok(),
            github_client_secret: std::env::var("GITHUB_CLIENT_SECRET").ok(),
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
            pacms_history_pool_size: env_parse("PACMS_HISTORY_POOL_SIZE", 150),
            savings_factor_refresh_secs: env_parse("SAVINGS_FACTOR_REFRESH_SECS", 86_400),
            savings_factor_min_samples: env_parse("SAVINGS_FACTOR_MIN_SAMPLES", 1_600),
            savings_factor_window_days: env_parse("SAVINGS_FACTOR_WINDOW_DAYS", 30),
            react_compress_enabled: env_parse("TOKEN_COMPRESS_TOOL_RESULTS", true),
            react_compress_min_bytes: env_parse("TOKEN_COMPRESS_TOOL_RESULTS_MIN_BYTES", 2048),
            history_compress_enabled: env_parse("TOKEN_COMPRESS_HISTORY", true),
            history_compress_min_bytes: env_parse("TOKEN_COMPRESS_HISTORY_MIN_BYTES", 2048),
            pacms_budget_low: env_parse("PACMS_BUDGET_LOW", 500),
            pacms_budget_medium: env_parse("PACMS_BUDGET_MEDIUM", 1000),
            pacms_budget_high: env_parse("PACMS_BUDGET_HIGH", 5000),
            pacms_history_mandatory_recent: env_parse("PACMS_HISTORY_MANDATORY_RECENT", 3),
            context_k_low: env_parse("CONTEXT_K_LOW", 1),
            context_k_medium: env_parse("CONTEXT_K_MEDIUM", 5),
            context_k_high: env_parse("CONTEXT_K_HIGH", 20),
            embedding_model: env_or("EMBEDDING_MODEL", "text-embedding-3-small"),
            agent_call_timeout_secs: std::env::var("AGENT_CALL_TIMEOUT_SECS")
                .or_else(|_| std::env::var("ROUTER_AGENT_TIMEOUT_SECS"))
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(600),
            github_callback_url: std::env::var("GITHUB_CALLBACK_URL").ok(),
            github_central_callback_url: std::env::var("GITHUB_CENTRAL_CALLBACK_URL")
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
            trace_usage_sync_secs: env_parse("TRACE_USAGE_SYNC_SECS", 120),
            trace_usage_overlap_secs: env_parse("TRACE_USAGE_OVERLAP_SECS", 600),
            trace_usage_batch_size: env_parse("TRACE_USAGE_BATCH_SIZE", 50),
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
            oidc_provider_label: env_or("OIDC_PROVIDER_LABEL", ""),
            oidc_central_callback_url: std::env::var("OIDC_CENTRAL_CALLBACK_URL")
                .ok()
                .filter(|s| !s.is_empty()),

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
            build_concurrency: env_parse("BUILD_CONCURRENCY", 4).clamp(1, 16),
            agent_default_memory: env_or("AGENT_DEFAULT_MEMORY", "1Gi"),
            agent_memory_volume: env_or("AGENT_MEMORY_VOLUME", "nasiko-agent-memory"),
            agent_memory_init_image: env_or("AGENT_MEMORY_INIT_IMAGE", "alpine:3.21"),
            mcp_toolcount_ttl_seconds: env_parse("MCP_TOOLCOUNT_TTL_SECONDS", 3600),
            seed_toolkits: std::env::var("SEED_TOOLKITS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            mcp_tool_search_mode: env_or("MCP_TOOL_SEARCH_MODE", "semantic"),
            mcp_tool_search_tool_limit: env_parse("MCP_TOOL_SEARCH_TOOL_LIMIT", 15),
            mcp_tool_search_meta_limit: env_parse("MCP_TOOL_SEARCH_META_LIMIT", 10),
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
/// `cp.nasiko.dev` and typical deployment env files have it — so appending `/v1/whatever`
/// to the raw value doubles up into `.../v1/v1/whatever`, which 404s.
///
/// Deliberately a free function rather than normalization applied to
/// [`Config::openai_base_url`] itself: the artifact registry uses the opposite
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

/// Whether `provider` selects an S3-compatible backend, and therefore whether
/// the `S3_*` credentials are required at startup.
///
/// Unknown values answer `true`: the authoritative parse lives with whichever
/// edition's composition root selects the backend, and it rejects them with a
/// proper message. Answering `false` here would pre-empt that with a confusing
/// missing-S3_SECRET_KEY error instead.
pub fn uses_s3_storage(provider: &str) -> bool {
    !matches!(
        provider.trim().to_ascii_lowercase().as_str(),
        "azure-blob" | "azure_blob" | "azure"
    )
}

#[cfg(test)]
mod storage_provider_tests {
    use super::uses_s3_storage;

    #[test]
    fn s3_credentials_are_required_only_for_an_s3_backend() {
        assert!(uses_s3_storage("s3"));
        assert!(uses_s3_storage(""));
        assert!(!uses_s3_storage("azure-blob"));
        assert!(!uses_s3_storage(" Azure-Blob "));
        // A typo must not silently waive the S3 requirement — the provider
        // parser is what reports it.
        assert!(uses_s3_storage("azureblob"));
    }
}
