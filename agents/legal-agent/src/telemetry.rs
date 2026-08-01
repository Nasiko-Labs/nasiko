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
    let service_name = std::env::var("OTEL_SERVICE_NAME")
        .unwrap_or_else(|_| env!("CARGO_PKG_NAME").into());

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info".parse().unwrap());

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
        let otel_layer = tracing_opentelemetry::layer()
            .with_tracer(tracer_provider.tracer(service_name));

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

/// Whether message content may be recorded on spans, per the platform-injected
/// `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT`. The platform sets an
/// enum ("NO_CONTENT"/"EVENT_ONLY"/"SPAN_ONLY"/…); older configs use booleans.
/// Anything except an explicit opt-out means content capture is on.
pub fn capture_content() -> bool {
    match std::env::var("OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT") {
        Ok(v) => !matches!(
            v.to_ascii_uppercase().as_str(),
            "NO_CONTENT" | "FALSE" | "0" | ""
        ),
        Err(_) => true,
    }
}

/// Convert OpenAI-format chat `messages` into the OTel GenAI semconv shape for
/// `gen_ai.input.messages`: `[{role, parts:[{type:"text",content}|{type:"tool_call",…}|
/// {type:"tool_call_response",…}]}]` (semconv ≥ 1.36, experimental).
pub fn genai_input_messages(messages: &[serde_json::Value]) -> serde_json::Value {
    let converted: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            let role = m["role"].as_str().unwrap_or("user");
            let mut parts = Vec::new();
            if let Some(text) = m["content"].as_str().filter(|t| !t.is_empty()) {
                if role == "tool" {
                    parts.push(serde_json::json!({
                        "type": "tool_call_response",
                        "id": m["tool_call_id"].as_str().unwrap_or(""),
                        "response": text,
                    }));
                } else {
                    parts.push(serde_json::json!({"type": "text", "content": text}));
                }
            }
            if let Some(calls) = m["tool_calls"].as_array() {
                for call in calls {
                    parts.push(serde_json::json!({
                        "type": "tool_call",
                        "id": call["id"],
                        "name": call["function"]["name"],
                        "arguments": call["function"]["arguments"],
                    }));
                }
            }
            serde_json::json!({"role": role, "parts": parts})
        })
        .collect();
    serde_json::Value::Array(converted)
}

/// Build a single-message GenAI semconv array (`gen_ai.output.messages` /
/// `gen_ai.input.messages`) from plain text.
pub fn genai_text_message(role: &str, text: &str) -> serde_json::Value {
    serde_json::json!([{
        "role": role,
        "parts": [{"type": "text", "content": text}],
    }])
}

/// Build `gen_ai.output.messages` for an assistant turn: streamed text plus any
/// tool calls, with the semconv `finish_reason` on the message.
pub fn genai_output_message(
    text: &str,
    tool_calls: &[serde_json::Value],
    finish_reason: &str,
) -> serde_json::Value {
    let mut parts = Vec::new();
    if !text.is_empty() {
        parts.push(serde_json::json!({"type": "text", "content": text}));
    }
    for call in tool_calls {
        parts.push(serde_json::json!({
            "type": "tool_call",
            "id": call["id"],
            "name": call["function"]["name"],
            "arguments": call["function"]["arguments"],
        }));
    }
    serde_json::json!([{
        "role": "assistant",
        "parts": parts,
        "finish_reason": finish_reason,
    }])
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
