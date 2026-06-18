use std::collections::HashMap;

/// Context passed to [`InstrumentationInjector::inject`] when deploying an agent.
pub struct AgentContext {
    /// The agent's stable identifier (used as `OTEL_SERVICE_NAME`).
    pub agent_id: String,
    /// Optional tenant identifier, added to `OTEL_RESOURCE_ATTRIBUTES`.
    pub tenant_id: Option<String>,
    /// Optional image version, added to `OTEL_RESOURCE_ATTRIBUTES` as `service.version`.
    pub version: Option<String>,
    /// Whether to capture prompt/completion content in logs
    /// (`OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT`).
    pub capture_content: bool,
    /// OTLP collector endpoint, e.g. `http://otel-collector.nasiko-infra:4318`.
    pub otel_collector_endpoint: String,
}

/// Injects OpenTelemetry environment variables into an agent's `env_vars` map
/// at deploy time so the agent is automatically instrumented without code changes.
pub trait InstrumentationInjector: Send + Sync {
    fn inject(&self, env_vars: &mut HashMap<String, String>, ctx: &AgentContext);
}

/// OSS implementation: injects the 7 standard `OTEL_*` env vars.
pub struct OtelInjector;

impl InstrumentationInjector for OtelInjector {
    fn inject(&self, env_vars: &mut HashMap<String, String>, ctx: &AgentContext) {
        env_vars.insert(
            "OTEL_EXPORTER_OTLP_ENDPOINT".into(),
            ctx.otel_collector_endpoint.clone(),
        );
        env_vars.insert("OTEL_EXPORTER_OTLP_PROTOCOL".into(), "http/protobuf".into());
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
        env_vars.insert(
            "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT".into(),
            if ctx.capture_content { "true" } else { "false" }.into(),
        );
    }
}