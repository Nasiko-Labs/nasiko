use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    Resource,
    trace::{RandomIdGenerator, Sampler, SdkTracerProvider},
};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize tracing with OTLP export when `OTEL_EXPORTER_OTLP_ENDPOINT` is set.
/// Falls back to plain fmt subscriber for local dev.
pub fn init() {
    let service_name =
        std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| env!("CARGO_PKG_NAME").into());

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".parse().unwrap());

    if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        let resource = Resource::builder_empty()
            .with_attribute(KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                service_name.clone(),
            ))
            .build();

        let trace_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()
            .expect("failed to build OTLP span exporter");

        let tracer_provider = SdkTracerProvider::builder()
            .with_batch_exporter(trace_exporter)
            .with_sampler(Sampler::AlwaysOn)
            .with_id_generator(RandomIdGenerator::default())
            .with_resource(resource)
            .build();

        opentelemetry::global::set_tracer_provider(tracer_provider.clone());
        let otel_layer =
            tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer(service_name));

        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer())
            .with(otel_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer())
            .init();
    }
}

/// Parse a W3C `traceparent` header (`00-<trace_id>-<span_id>-<flags>`) into a
/// remote OTel context, so spans created under it join the caller's trace.
/// Returns None for anything malformed — the caller then starts a local root
/// trace, which is the correct fallback.
pub fn remote_context_from_traceparent(tp: &str) -> Option<opentelemetry::Context> {
    use opentelemetry::propagation::TextMapPropagator;
    let carrier: std::collections::HashMap<String, String> =
        [("traceparent".to_string(), tp.to_string())].into();
    let ctx = opentelemetry_sdk::propagation::TraceContextPropagator::new().extract(&carrier);
    use opentelemetry::trace::TraceContextExt;
    ctx.span().span_context().is_valid().then_some(ctx)
}
