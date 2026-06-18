use nasiko_utils::{env_or, env_parse, required_env};

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub domain: Option<String>,
    pub database_url: String,
    pub redis_url: String,
    pub scheduler_mode: String,
    pub k8s_namespace: String,
    pub kubeconfig: Option<String>,
    pub s3_endpoint: String,
    pub s3_bucket: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub s3_region: String,
    pub secrets_encryption_key: String,
    pub oci_storage_bucket: String,
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
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            bind: env_or("CP_BIND", "0.0.0.0:8080"),
            domain: std::env::var("CP_DOMAIN").ok(),
            database_url: required_env("DATABASE_URL")?,
            redis_url: required_env("REDIS_URL")?,
            scheduler_mode: env_or("SCHEDULER_MODE", "local"),
            k8s_namespace: env_or("K8S_NAMESPACE", "nasiko-agents"),
            kubeconfig: std::env::var("KUBECONFIG").ok().filter(|s| !s.is_empty()),
            s3_endpoint: env_or("S3_ENDPOINT", "http://localhost:9000"),
            s3_bucket: env_or("S3_BUCKET", "nasiko"),
            s3_access_key: env_or("S3_ACCESS_KEY", "nasiko"),
            s3_secret_key: required_env("S3_SECRET_KEY")?,
            s3_region: env_or("S3_REGION", "us-east-1"),
            secrets_encryption_key: required_env("SECRETS_ENCRYPTION_KEY")?,
            oci_storage_bucket: env_or("OCI_STORAGE_BUCKET", "nasiko-artifacts"),
            seed_agents: std::env::var("SEED_AGENTS").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            openai_base_url: std::env::var("OPENAI_BASE_URL").ok(),
            openai_model: env_or("OPENAI_MODEL", "deepseek-v4-flash"),
            router_model: env_or("ROUTER_MODEL", "deepseek-v4-pro"),
            capability_generator_model: env_or("CAPABILITY_GENERATOR_MODEL", "deepseek-v4-flash"),
            a2a_discovery_url: std::env::var("A2A_DISCOVERY_URL").ok(),
            otel_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            otel_protocol: env_or("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
            otel_headers: std::env::var("OTEL_EXPORTER_OTLP_HEADERS").ok(),
            otel_service_name: env_or("OTEL_SERVICE_NAME", "nasiko-cp"),
            otel_sample_ratio: env_or("OTEL_TRACES_SAMPLER_ARG", "1.0"),
            otel_collector_endpoint: env_or(
                "OTEL_EXPORTER_OTLP_ENDPOINT",
                "http://otel-collector.nasiko-infra:4318",
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
        })
    }
}
