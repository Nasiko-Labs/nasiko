use std::collections::HashMap;

/// Patch a Dockerfile to add OpenTelemetry auto-instrumentation.
///
/// Supports Python agents. Detects the runtime from the Dockerfile content and:
/// 1. Inserts a `RUN pip install` step for the OTel packages + bootstrap before the CMD.
/// 2. Wraps the CMD with `opentelemetry-instrument` so traces are emitted without
///    any changes to the agent's own source code.
///
/// Idempotent: if `opentelemetry-instrument` is already present, the Dockerfile is
/// returned unchanged.
pub fn patch_dockerfile_for_otel(content: &str) -> String {
    // Already patched — don't double-inject.
    if content.contains("opentelemetry-instrument") {
        return content.to_string();
    }

    let is_python = content.contains("pip install") || content.contains("pip3 install")
        || content.to_lowercase().contains("python");

    if !is_python {
        // Non-Python runtimes: return unchanged for now.
        return content.to_string();
    }

    let otel_install = concat!(
        "RUN pip install --no-cache-dir opentelemetry-distro opentelemetry-exporter-otlp ",
        "&& opentelemetry-bootstrap -a install"
    );

    // Rewrite lines: insert the pip install before the first CMD, then wrap CMD.
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    let mut cmd_index: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim().to_uppercase();
        if trimmed.starts_with("CMD") {
            cmd_index = Some(i);
            break;
        }
    }

    let Some(cmd_idx) = cmd_index else {
        // No CMD found — just append the install and a wrapped CMD.
        lines.push(otel_install.to_string());
        return lines.join("\n");
    };

    // Insert the OTel install line just before the CMD.
    lines.insert(cmd_idx, otel_install.to_string());
    // cmd_idx is now the install line; original CMD is at cmd_idx + 1.
    let cmd_line = &lines[cmd_idx + 1];
    let trimmed = cmd_line.trim();

    let new_cmd = if trimmed.starts_with("CMD [") {
        // JSON exec form: CMD ["python", "main.py", ...] → CMD ["opentelemetry-instrument", "python", ...]
        let inner = trimmed
            .trim_start_matches("CMD [")
            .trim_end_matches(']');
        format!("CMD [\"opentelemetry-instrument\", {inner}]")
    } else {
        // Shell form: CMD python main.py → CMD opentelemetry-instrument python main.py
        let args = trimmed.trim_start_matches("CMD").trim();
        format!("CMD opentelemetry-instrument {args}")
    };

    lines[cmd_idx + 1] = new_cmd;
    lines.join("\n")
}

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