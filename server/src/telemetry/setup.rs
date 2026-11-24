use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::{WithExportConfig, WithTonicConfig};
use opentelemetry_sdk::{
    Resource,
    metrics::{PeriodicReader, SdkMeterProvider},
    trace::{Sampler, SdkTracerProvider},
};
use std::time::Duration;
use tonic::metadata::{Ascii, MetadataKey, MetadataMap, MetadataValue};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

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
    /// How often accumulated metrics are pushed to the OTLP endpoint. Consumers that react to
    /// load read these series, so the push cadence bounds how stale they are.
    pub metric_export_interval: Duration,
}

/// Default push cadence for metrics (`OTEL_METRIC_EXPORT_INTERVAL`).
pub const DEFAULT_METRIC_EXPORT_INTERVAL: Duration = Duration::from_millis(15_000);

/// Parse `OTEL_METRIC_EXPORT_INTERVAL` (milliseconds, the OTel spec name and unit). Unset gives the default; anything that is not a
/// positive whole number of milliseconds is an error, not a silent fallback.
fn parse_metric_export_interval(raw: Option<&str>) -> Result<Duration, String> {
    let Some(raw) = raw else {
        return Ok(DEFAULT_METRIC_EXPORT_INTERVAL);
    };
    match raw.trim().parse::<u64>() {
        Ok(ms) if ms > 0 => Ok(Duration::from_millis(ms)),
        _ => Err(format!(
            "OTEL_METRIC_EXPORT_INTERVAL must be a positive integer number of milliseconds, got {raw:?}"
        )),
    }
}

/// Tracing is not installed yet when this runs, so a bad value is reported on stderr before the
/// default is used.
fn metric_export_interval_from_env() -> Duration {
    let raw = std::env::var("OTEL_METRIC_EXPORT_INTERVAL").ok();
    parse_metric_export_interval(raw.as_deref()).unwrap_or_else(|message| {
        eprintln!("{message}; using the default of {DEFAULT_METRIC_EXPORT_INTERVAL:?}");
        DEFAULT_METRIC_EXPORT_INTERVAL
    })
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
                Some((
                    parts.next()?.trim().to_string(),
                    parts.next()?.trim().to_string(),
                ))
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
            metric_export_interval: metric_export_interval_from_env(),
        }
    }
}

/// Initialize OpenTelemetry tracing + metrics pipeline.
/// If no OTLP endpoint is configured, only local tracing-subscriber is set up.
/// Call `error_reporting::init` first: its Sentry layer joins here only if it is on.
pub fn init_telemetry(config: &TelemetryConfig) {
    // `service.instance.id` keeps each replica's metric series apart: without it two replicas
    // export the same series, the collector keeps one, and a sum across replicas (the agent
    // autoscaler's in-flight count, the server's own) reads one replica's share.
    let resource = Resource::builder()
        .with_attributes([
            KeyValue::new("service.name", config.service_name.clone()),
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            KeyValue::new(
                "service.instance.id",
                crate::coding::turn::replica_id().to_owned(),
            ),
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
            .with(crate::error_reporting::tracing_layer())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().compact())
            .with(crate::error_reporting::tracing_layer())
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

    let reader = PeriodicReader::builder(exporter)
        .with_interval(config.metric_export_interval)
        .build();

    SdkMeterProvider::builder()
        .with_resource(resource)
        .with_reader(reader)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_export_interval_defaults_when_unset() {
        assert_eq!(
            parse_metric_export_interval(None),
            Ok(DEFAULT_METRIC_EXPORT_INTERVAL)
        );
    }

    #[test]
    fn metric_export_interval_reads_milliseconds() {
        assert_eq!(
            parse_metric_export_interval(Some(" 5000 ")),
            Ok(Duration::from_millis(5_000))
        );
    }

    #[test]
    fn metric_export_interval_rejects_zero_and_garbage() {
        for bad in ["0", "-1", "15s", ""] {
            assert!(parse_metric_export_interval(Some(bad)).is_err(), "{bad}");
        }
    }
}
