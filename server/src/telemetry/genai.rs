use opentelemetry::{
    global,
    metrics::{Counter, Histogram},
    KeyValue,
};
use std::time::Instant;

/// OTel GenAI semantic convention attribute names.
/// Based on: https://opentelemetry.io/docs/specs/semconv/gen-ai/
pub mod attr {
    pub const GEN_AI_SYSTEM: &str = "gen_ai.system";
    pub const GEN_AI_OPERATION_NAME: &str = "gen_ai.operation.name";
    pub const GEN_AI_REQUEST_MODEL: &str = "gen_ai.request.model";
    pub const GEN_AI_RESPONSE_MODEL: &str = "gen_ai.response.model";
    pub const GEN_AI_RESPONSE_FINISH_REASONS: &str = "gen_ai.response.finish_reasons";
    pub const GEN_AI_USAGE_INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
    pub const GEN_AI_USAGE_OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";
    pub const GEN_AI_TOKEN_TYPE: &str = "gen_ai.token.type";
    pub const GEN_AI_TOOL_NAME: &str = "gen_ai.tool.name";
    pub const GEN_AI_TOOL_CALL_ID: &str = "gen_ai.tool.call.id";

    // Nasiko-specific extensions for multi-agent flows
    pub const NASIKO_AGENT_ID: &str = "nasiko.agent.id";
    pub const NASIKO_AGENT_NAME: &str = "nasiko.agent.name";
    pub const NASIKO_FLOW_DEPTH: &str = "nasiko.flow.depth";
    pub const NASIKO_FLOW_ROOT_AGENT: &str = "nasiko.flow.root_agent";
    pub const NASIKO_FLOW_TRACE_ID: &str = "nasiko.flow.trace_id";
    pub const NASIKO_TEAM_ID: &str = "nasiko.team.id";
}

/// Metrics following OTel GenAI semantic conventions.
#[derive(Clone)]
pub struct GenAiMetrics {
    pub token_usage: Counter<u64>,
    pub operation_duration: Histogram<f64>,
    pub agent_invocations: Counter<u64>,
    pub flow_depth: Histogram<u64>,
    pub cascade_rejections: Counter<u64>,
}

impl GenAiMetrics {
    pub fn new() -> Self {
        let meter = global::meter("nasiko.cp");

        let token_usage = meter
            .u64_counter("gen_ai.client.token.usage")
            .with_description("Token usage per LLM call")
            .build();

        let operation_duration = meter
            .f64_histogram("gen_ai.client.operation.duration")
            .with_description("Duration of GenAI operations in seconds")
            .with_unit("s")
            .build();

        let agent_invocations = meter
            .u64_counter("nasiko.agent.invocations")
            .with_description("Total agent invocations proxied through CP")
            .build();

        let flow_depth = meter
            .u64_histogram("nasiko.flow.depth")
            .with_description("Depth of multi-agent invocation chains")
            .build();

        let cascade_rejections = meter
            .u64_counter("nasiko.flow.cascade_rejections")
            .with_description("Requests rejected due to cascade limits")
            .build();

        Self {
            token_usage,
            operation_duration,
            agent_invocations,
            flow_depth,
            cascade_rejections,
        }
    }

    pub fn record_tokens(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        model: &str,
        agent_name: &str,
        team_id: &str,
    ) {
        let common = &[
            KeyValue::new(attr::GEN_AI_REQUEST_MODEL, model.to_string()),
            KeyValue::new(attr::NASIKO_AGENT_NAME, agent_name.to_string()),
            KeyValue::new(attr::NASIKO_TEAM_ID, team_id.to_string()),
        ];

        self.token_usage.add(
            input_tokens,
            &[
                common[0].clone(),
                common[1].clone(),
                common[2].clone(),
                KeyValue::new(attr::GEN_AI_TOKEN_TYPE, "input"),
            ],
        );

        self.token_usage.add(
            output_tokens,
            &[
                common[0].clone(),
                common[1].clone(),
                common[2].clone(),
                KeyValue::new(attr::GEN_AI_TOKEN_TYPE, "output"),
            ],
        );
    }

    pub fn record_operation(
        &self,
        duration_secs: f64,
        operation: &str,
        model: &str,
        agent_name: &str,
        team_id: &str,
    ) {
        self.operation_duration.record(
            duration_secs,
            &[
                KeyValue::new(attr::GEN_AI_OPERATION_NAME, operation.to_string()),
                KeyValue::new(attr::GEN_AI_REQUEST_MODEL, model.to_string()),
                KeyValue::new(attr::NASIKO_AGENT_NAME, agent_name.to_string()),
                KeyValue::new(attr::NASIKO_TEAM_ID, team_id.to_string()),
            ],
        );
    }

    pub fn record_invocation(&self, agent_name: &str, team_id: &str) {
        self.agent_invocations.add(
            1,
            &[
                KeyValue::new(attr::NASIKO_AGENT_NAME, agent_name.to_string()),
                KeyValue::new(attr::NASIKO_TEAM_ID, team_id.to_string()),
            ],
        );
    }

    pub fn record_flow_depth(&self, depth: u64, root_agent: &str) {
        self.flow_depth.record(
            depth,
            &[KeyValue::new(attr::NASIKO_FLOW_ROOT_AGENT, root_agent.to_string())],
        );
    }

    pub fn record_cascade_rejection(&self, reason: &str, agent_name: &str) {
        self.cascade_rejections.add(
            1,
            &[
                KeyValue::new("reason", reason.to_string()),
                KeyValue::new(attr::NASIKO_AGENT_NAME, agent_name.to_string()),
            ],
        );
    }
}

/// Builder for creating a GenAI span on a proxied LLM request.
/// Produces a tracing span with OTel GenAI semantic convention attributes.
pub struct GenAiSpan {
    pub operation: String,
    pub system: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
    pub flow_depth: u32,
    pub flow_trace_id: Option<String>,
    pub start: Instant,
}

impl GenAiSpan {
    pub fn new(operation: &str, system: &str, model: &str) -> Self {
        Self {
            operation: operation.to_string(),
            system: system.to_string(),
            model: model.to_string(),
            agent_id: String::new(),
            agent_name: String::new(),
            flow_depth: 0,
            flow_trace_id: None,
            start: Instant::now(),
        }
    }

    pub fn with_agent(mut self, id: &str, name: &str) -> Self {
        self.agent_id = id.to_string();
        self.agent_name = name.to_string();
        self
    }

    pub fn with_flow(mut self, depth: u32, trace_id: Option<String>) -> Self {
        self.flow_depth = depth;
        self.flow_trace_id = trace_id;
        self
    }

    /// Returns OTel-compatible attribute key-value pairs for this span.
    pub fn attributes(&self) -> Vec<KeyValue> {
        let mut attrs = vec![
            KeyValue::new(attr::GEN_AI_OPERATION_NAME, self.operation.clone()),
            KeyValue::new(attr::GEN_AI_SYSTEM, self.system.clone()),
            KeyValue::new(attr::GEN_AI_REQUEST_MODEL, self.model.clone()),
            KeyValue::new(attr::NASIKO_AGENT_ID, self.agent_id.clone()),
            KeyValue::new(attr::NASIKO_AGENT_NAME, self.agent_name.clone()),
            KeyValue::new(attr::NASIKO_FLOW_DEPTH, self.flow_depth as i64),
        ];

        if let Some(ref trace_id) = self.flow_trace_id {
            attrs.push(KeyValue::new(attr::NASIKO_FLOW_TRACE_ID, trace_id.clone()));
        }

        attrs
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}
