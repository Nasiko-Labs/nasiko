pub mod error;
pub mod injector;
pub mod loki;
pub mod provider;
pub mod runtime_ext;
pub mod tempo;
pub mod types;

pub use error::ObservabilityError;
pub use injector::{AgentContext, InstrumentationInjector, OtelInjector};
pub use loki::LokiClient;
pub use provider::{ObservabilityProvider, TempoLokiProvider};
pub use runtime_ext::InstrumentedRuntime;
pub use tempo::TempoClient;
pub use types::{
    AgentFinOps, AgentStats, FinOpsDashboard, Session, Span, SpanDetails, TokenUsage,
    TraceDetails,
};

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
            service_name: std::env::var("OTEL_SERVICE_NAME")
                .unwrap_or_else(|_| "nasiko".into()),
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            otlp_protocol: std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL")
                .unwrap_or_else(|_| "grpc".into()),
            otlp_headers: std::env::var("OTEL_EXPORTER_OTLP_HEADERS").ok(),
            sample_ratio: std::env::var("OTEL_TRACES_SAMPLER_ARG")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0),
        }
    }
}

pub fn init_telemetry(_config: &TelemetryConfig) {
    // TODO(BACKEND-18): full OpenTelemetry initialization (traces + metrics via OTLP)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".parse().unwrap()),
        )
        .init();
}