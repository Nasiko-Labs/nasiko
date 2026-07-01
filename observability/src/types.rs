use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single distributed trace session, identified by its W3C trace_id.
/// In the Nasiko model, one session == one trace_id shared across all agents
/// that participated in a single user interaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub trace_id: String,
    /// All agent service names observed in this trace.
    pub agent_ids: Vec<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub span_count: u32,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
}

/// A single span within a distributed trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    /// `resource.service.name` from the OTLP resource attributes.
    pub service_name: String,
    /// OTLP span kind integer (0=unspecified 1=internal 2=server 3=client 4=producer 5=consumer).
    pub kind: u8,
    /// OTLP status code integer (0=unset 1=ok 2=error).
    pub status_code: u8,
    pub status_message: String,
    /// All span-level attributes (gen_ai.*, http.*, etc.).
    pub attributes: HashMap<String, serde_json::Value>,
}

/// Full trace with all its spans. Returned by `get_trace`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceDetails {
    pub trace_id: String,
    pub spans: Vec<Span>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
}

impl TraceDetails {
    /// Aggregate `gen_ai.usage.*_tokens` across all spans in this trace.
    pub fn token_usage(&self) -> TokenUsage {
        let mut usage = TokenUsage::default();
        for span in &self.spans {
            if let Some(v) = span.attributes.get("gen_ai.usage.input_tokens") {
                usage.input_tokens += v.as_u64().unwrap_or(0);
            }
            if let Some(v) = span.attributes.get("gen_ai.usage.output_tokens") {
                usage.output_tokens += v.as_u64().unwrap_or(0);
            }
        }
        usage.total_tokens = usage.input_tokens + usage.output_tokens;
        usage
    }
}

/// Span detail enriched with prompt/completion log lines fetched from Loki.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanDetails {
    pub span: Span,
    /// Raw prompt log line from Loki, if captured (`OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true`).
    pub prompt_content: Option<String>,
    /// Raw completion log line from Loki, if captured.
    pub completion_content: Option<String>,
}

/// Aggregated token counts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

/// Per-agent performance stats over a time window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStats {
    pub agent_id: String,
    pub total_requests: u64,
    pub total_tokens: TokenUsage,
    pub avg_latency_ms: f64,
    /// Fraction of requests where any span has `otel.status_code = "ERROR"` (0.0–1.0).
    pub error_rate: f64,
    pub period_start: DateTime<Utc>,
}

/// Cost and token breakdown for a single agent in the FinOps view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentFinOps {
    pub agent_id: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    /// Rough USD estimate based on a blended token price.
    pub estimated_cost_usd: f64,
    pub request_count: u64,
}

/// Aggregated FinOps dashboard across all requested agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinOpsDashboard {
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub agents: Vec<AgentFinOps>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_estimated_cost_usd: f64,
}