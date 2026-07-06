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
    pub flow_max_depth: i32,
    pub flow_max_fan_out: i32,
    pub flow_max_tokens: i64,
    pub flow_timeout_secs: i32,
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
    pub router_shortlist_threshold: usize,
    pub router_shortlist_size: usize,
    pub max_router_history_messages: usize,
    /// OpenAI-compatible model used for Stage 1 vector embeddings.
    /// Default: `text-embedding-3-small`. Stage 1 is skipped if `openai_api_key` is unset.
    pub embedding_model: String,
    pub router_agent_timeout_secs: u64,
    pub github_callback_url: Option<String>,
    pub git_clone_allowed_hosts: Vec<String>,
    /// Allowed OCI registry hosts for `POST /api/catalog/import/registry`.
    /// Comma-separated.  Empty = reject all registry imports (safest default for
    /// new deployments).  Example: "ghcr.io,quay.io,registry.nasiko.dev"
    pub registry_import_allowed_hosts: Vec<String>,
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
            .unwrap_or(false),
            tempo_url: env_or(
                "TEMPO_URL",
                "http://tempo.nasiko-infra.svc.cluster.local:3200",
            ),
            loki_url: env_or(
                "LOKI_URL",
                "http://loki.nasiko-infra.svc.cluster.local:3100",
            ),
            flow_max_depth: env_parse("NASIKO_FLOW_MAX_DEPTH", 5),
            flow_max_fan_out: env_parse("NASIKO_FLOW_MAX_FAN_OUT", 20),
            flow_max_tokens: env_parse("NASIKO_FLOW_MAX_TOKENS", 100000),
            flow_timeout_secs: env_parse("NASIKO_FLOW_TIMEOUT_SECS", 120),
            github_client_id: std::env::var("GITHUB_CLIENT_ID").ok(),
            github_client_secret: std::env::var("GITHUB_CLIENT_SECRET").ok(),
            router_shortlist_threshold: env_parse("ROUTER_SHORTLIST_THRESHOLD", 15),
            router_shortlist_size: env_parse("ROUTER_SHORTLIST_SIZE", 10),
            max_router_history_messages: env_parse("MAX_ROUTER_HISTORY_MESSAGES", 20),
            embedding_model: env_or("EMBEDDING_MODEL", "text-embedding-3-small"),
            router_agent_timeout_secs: env_parse("ROUTER_AGENT_TIMEOUT_SECS", 60),
            github_callback_url: std::env::var("GITHUB_CALLBACK_URL").ok(),
            docker_agent_network: std::env::var("DOCKER_AGENT_NETWORK").ok().filter(|s| !s.is_empty()),
            oci_registry_host: std::env::var("OCI_REGISTRY_HOST").ok().filter(|s| !s.is_empty()),
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
            admin_username: env_or("ADMIN_USERNAME", "admin"),
            admin_password: required_env("ADMIN_PASSWORD")?,
        })
    }
}
