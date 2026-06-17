// TODO: Extract telemetry setup and GenAI metrics from cp-lib/src/telemetry/
// For now, re-export the setup function signature so downstream crates can reference it.

pub struct TelemetryConfig {
    pub service_name: String,
    pub otlp_endpoint: Option<String>,
    pub otlp_protocol: String,
    pub otlp_headers: Option<String>,
    pub sample_ratio: f64,
}

impl TelemetryConfig {
    pub fn from_env() -> Self {
        Self {
            service_name: std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "nasiko".into()),
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            otlp_protocol: std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or_else(|_| "grpc".into()),
            otlp_headers: std::env::var("OTEL_EXPORTER_OTLP_HEADERS").ok(),
            sample_ratio: std::env::var("OTEL_TRACES_SAMPLER_ARG")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0),
        }
    }
}

pub fn init_telemetry(_config: &TelemetryConfig) {
    // TODO: full OpenTelemetry initialization (traces + metrics)
    // For now, just set up tracing-subscriber
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".parse().unwrap()),
        )
        .init();
}
