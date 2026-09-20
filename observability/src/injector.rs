use std::collections::HashMap;

/// Context passed to [`InstrumentationInjector::inject`] when deploying an agent.
pub struct AgentContext {
    /// The agent's stable identifier (used as `OTEL_SERVICE_NAME`).
    pub agent_id: String,
    /// Optional tenant identifier, added to `OTEL_RESOURCE_ATTRIBUTES`.
    pub tenant_id: Option<String>,
    /// Optional image version, added to `OTEL_RESOURCE_ATTRIBUTES` as `service.version`.
    pub version: Option<String>,
    /// Whether to capture prompt/completion content on spans and log events
    /// (`OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT`).
    pub capture_content: bool,
    /// OTLP collector endpoint.
    pub otel_collector_endpoint: String,
    /// OTLP export protocol (`grpc` or `http/protobuf`).
    pub otel_protocol: String,
}

/// Injects OpenTelemetry environment variables into an agent's `env_vars` map
/// at deploy time. Language-agnostic: any A2A agent that ships an OTel SDK
/// (e.g. the official GenAI instrumentations — `opentelemetry-instrumentation-openai-v2`
/// for Python, `@opentelemetry/auto-instrumentations-node` for JS, manual
/// semconv spans for Rust/Go) picks these up with zero platform-side code in
/// the image. The platform never rewrites agent images or sources.
pub trait InstrumentationInjector: Send + Sync {
    fn inject(&self, env_vars: &mut HashMap<String, String>, ctx: &AgentContext);
}

/// OSS implementation: injects the standard `OTEL_*` env vars.
pub struct OtelInjector;

impl InstrumentationInjector for OtelInjector {
    fn inject(&self, env_vars: &mut HashMap<String, String>, ctx: &AgentContext) {
        env_vars.insert(
            "OTEL_EXPORTER_OTLP_ENDPOINT".into(),
            ctx.otel_collector_endpoint.clone(),
        );
        env_vars.insert(
            "OTEL_EXPORTER_OTLP_PROTOCOL".into(),
            ctx.otel_protocol.clone(),
        );
        env_vars.insert("OTEL_SERVICE_NAME".into(), ctx.agent_id.clone());

        let mut resource_attrs = format!("agent.id={}", ctx.agent_id);
        if let Some(tenant) = &ctx.tenant_id {
            resource_attrs.push_str(&format!(",tenant.id={tenant}"));
        }
        if let Some(version) = &ctx.version {
            resource_attrs.push_str(&format!(",service.version={version}"));
        }
        env_vars.insert("OTEL_RESOURCE_ATTRIBUTES".into(), resource_attrs);

        env_vars.insert("OTEL_TRACES_EXPORTER".into(), "otlp".into());
        env_vars.insert("OTEL_LOGS_EXPORTER".into(), "otlp".into());
        // Opt the official GenAI instrumentations into the latest GenAI
        // semantic conventions. Without this flag they stay on legacy
        // conventions, parse the capture variable as a boolean, and the enum
        // below would read as "false".
        env_vars.insert(
            "OTEL_SEMCONV_STABILITY_OPT_IN".into(),
            "gen_ai_latest_experimental".into(),
        );
        // Content capture mode (latest GenAI semconv):
        //   no_content     — no prompt/completion captured (opt-out for compliance)
        //   event_only     — content emitted as OTel log events → Loki
        //   span_only      — gen_ai.input/output.messages span attributes → Tempo
        //   span_and_event — both
        // span_and_event lets the dashboard read content straight off the span
        // while keeping the Loki event path working.
        env_vars.insert(
            "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT".into(),
            if ctx.capture_content {
                "span_and_event"
            } else {
                "no_content"
            }
            .into(),
        );
    }
}
