pub mod error;
pub mod injector;
pub mod loki;
pub mod pricing;
pub mod provider;
pub mod runtime_ext;
pub mod tempo;
pub mod types;

pub use error::ObservabilityError;
pub use injector::{
    AgentContext, InstrumentationInjector, OtelInjector, patch_dockerfile_for_otel,
    write_otel_patch_file,
};
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

/// Initialize OpenTelemetry: OTLP trace + metric exporter when an endpoint is
/// configured, falling back to a plain fmt subscriber for local development.
///
/// Call once at binary startup before any `tracing::` calls.
pub fn init_telemetry(config: &TelemetryConfig) {
    use opentelemetry::KeyValue;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::{
        Resource,
        metrics::{PeriodicReader, SdkMeterProvider},
        trace::{RandomIdGenerator, Sampler, SdkTracerProvider},
    };
    use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info".parse().unwrap());

    if let Some(endpoint) = &config.otlp_endpoint {
        let resource = Resource::builder_empty()
            .with_attribute(KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                config.service_name.clone(),
            ))
            .build();

        // ── Trace provider ────────────────────────────────────────────────
        let trace_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint.clone())
            .build()
            .expect("failed to build OTLP span exporter");

        let sampler = if config.sample_ratio >= 1.0 {
            Sampler::AlwaysOn
        } else {
            Sampler::TraceIdRatioBased(config.sample_ratio)
        };

        let tracer_provider = SdkTracerProvider::builder()
            .with_batch_exporter(trace_exporter)
            .with_sampler(sampler)
            .with_id_generator(RandomIdGenerator::default())
            .with_resource(resource.clone())
            .build();

        let otel_trace_layer = tracing_opentelemetry::layer()
            .with_tracer(tracer_provider.tracer(config.service_name.clone()));

        // ── Metrics provider ──────────────────────────────────────────────
        let metrics_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint.clone())
            .build()
            .expect("failed to build OTLP metric exporter");

        let reader = PeriodicReader::builder(metrics_exporter).build();

        let _meter_provider = SdkMeterProvider::builder()
            .with_reader(reader)
            .with_resource(resource)
            .build();

        // Register as global so GenAiMetrics picks it up
        opentelemetry::global::set_meter_provider(_meter_provider);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer())
            .with(otel_trace_layer)
            .init();
    } else {
        // Local dev: structured fmt only, no OTLP
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer())
            .init();
    }
}