use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::{WithExportConfig, WithTonicConfig};
use opentelemetry_sdk::{
    metrics::SdkMeterProvider,
    trace::{SdkTracerProvider, Sampler},
    Resource,
};
use tonic::metadata::{Ascii, MetadataKey, MetadataMap, MetadataValue};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Configuration for OTel export.
/// Enterprise customers configure their own OTLP endpoint.
#[derive(Clone, Debug)]
pub struct TelemetryConfig {
    pub service_name: String,
    pub otlp_endpoint: Option<String>,
    pub otlp_protocol: OtlpProtocol,
    /// Additional headers (e.g. API keys for Datadog/Grafana Cloud)
    pub otlp_headers: Vec<(String, String)>,
    pub sample_ratio: f64,
}

#[derive(Clone, Debug, Default)]
pub enum OtlpProtocol {
    #[default]
    Grpc,
    Http,
}

impl TelemetryConfig {
    pub fn from_env() -> Self {
        let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
        let protocol = match std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL")
            .unwrap_or_default()
            .as_str()
        {
            "http/protobuf" | "http" => OtlpProtocol::Http,
            _ => OtlpProtocol::Grpc,
        };

        let headers = std::env::var("OTEL_EXPORTER_OTLP_HEADERS")
            .unwrap_or_default()
            .split(',')
            .filter_map(|kv| {
                let mut parts = kv.splitn(2, '=');
                Some((parts.next()?.trim().to_string(), parts.next()?.trim().to_string()))
            })
            .collect();

        let sample_ratio = std::env::var("OTEL_TRACES_SAMPLER_ARG")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1.0);

        Self {
            service_name: std::env::var("OTEL_SERVICE_NAME")
                .unwrap_or_else(|_| "nasiko-cp".to_string()),
            otlp_endpoint: endpoint,
            otlp_protocol: protocol,
            otlp_headers: headers,
            sample_ratio,
        }
    }
}

/// Initialize OpenTelemetry tracing + metrics pipeline.
/// If no OTLP endpoint is configured, only local tracing-subscriber is set up.
pub fn init_telemetry(config: &TelemetryConfig) {
    let resource = Resource::builder()
        .with_attributes([
            KeyValue::new("service.name", config.service_name.clone()),
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
        ])
        .build();

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,nasiko_cp_lib=debug"));

    if let Some(ref endpoint) = config.otlp_endpoint {
        let tracer_provider = build_tracer_provider(endpoint, config, resource.clone());
        let tracer = tracer_provider.tracer("nasiko-cp");
        global::set_tracer_provider(tracer_provider);

        let meter_provider = build_meter_provider(endpoint, config, resource);
        global::set_meter_provider(meter_provider);

        let otel_layer = OpenTelemetryLayer::new(tracer);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().compact())
            .with(otel_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().compact())
            .init();
    }
}

fn build_metadata(headers: &[(String, String)]) -> MetadataMap {
    let mut map = MetadataMap::new();
    for (key, value) in headers {
        if let (Ok(k), Ok(v)) = (
            key.parse::<MetadataKey<Ascii>>(),
            value.parse::<MetadataValue<Ascii>>(),
        ) {
            map.insert(k, v);
        }
    }
    map
}

fn build_tracer_provider(
    endpoint: &str,
    config: &TelemetryConfig,
    resource: Resource,
) -> SdkTracerProvider {
    let metadata = build_metadata(&config.otlp_headers);

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_metadata(metadata)
        .build()
        .expect("failed to build OTLP span exporter");

    SdkTracerProvider::builder()
        .with_resource(resource)
        .with_sampler(Sampler::TraceIdRatioBased(config.sample_ratio))
        .with_batch_exporter(exporter)
        .build()
}

fn build_meter_provider(
    endpoint: &str,
    config: &TelemetryConfig,
    resource: Resource,
) -> SdkMeterProvider {
    let metadata = build_metadata(&config.otlp_headers);

    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_metadata(metadata)
        .build()
        .expect("failed to build OTLP metric exporter");

    SdkMeterProvider::builder()
        .with_resource(resource)
        .with_periodic_exporter(exporter)
        .build()
}
