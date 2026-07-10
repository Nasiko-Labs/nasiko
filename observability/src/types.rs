use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::pricing::CostBreakdown;

/// A user-facing conversation, identified by the A2A `contextId`
/// (e.g. `ses_14cda...`), carried on spans as the `session.id` attribute.
///
/// One session groups **many** traces: each user query produces one
/// `trace_id`, and all agents participating in that query share it via
/// W3C `traceparent` propagation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// A2A contextId (`session.id` span attribute) — NOT a trace id.
    pub session_id: String,
    /// Agent (Tempo `resource.service.name`) this summary was aggregated for.
    pub agent_id: String,
    /// One trace per user query in this session.
    pub trace_ids: Vec<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// First model observed in the session's spans.
    pub model_used: Option<String>,
    /// Percentiles over chat-span durations within the session.
    pub latency_ms_p50: Option<f64>,
    pub latency_ms_p99: Option<f64>,
    #[serde(skip)]
    pub cost: CostBreakdown,
}

/// Full session drill-down: one [`TraceSummary`] per user query.
#[derive(Debug, Clone)]
pub struct SessionDetails {
    pub session_id: String,
    pub traces: Vec<TraceSummary>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model_used: Option<String>,
    pub latency_ms_p50: Option<f64>,
    pub cost: CostBreakdown,
}

/// One user query (= one trace) inside a session.
#[derive(Debug, Clone)]
pub struct TraceSummary {
    pub trace_id: String,
    /// Root span of the trace (entry point of the user query).
    pub root_span: Span,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model_used: Option<String>,
    pub duration_ms: Option<u64>,
    pub cost: CostBreakdown,
    /// Prompt content from Loki for the root span, when captured.
    pub input_content: Option<String>,
    /// Completion content from Loki for the root span, when captured.
    pub output_content: Option<String>,
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

/// Extract `(input_tokens, output_tokens, model)` from a span's attributes.
/// Covers current GenAI semconv names and older/deprecated variants.
pub fn extract_token_attrs(attrs: &HashMap<String, serde_json::Value>) -> (u64, u64, Option<String>) {
    let get_u64 = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| attrs.get(*k))
            .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(0)
    };

    let input = get_u64(&[
        "gen_ai.usage.input_tokens",  // semconv v1.27+
        "gen_ai.usage.prompt_tokens", // pre-1.27, still common
        "llm.usage.prompt_tokens",    // LangChain / LlamaIndex
        "input_tokens",
    ]);
    let output = get_u64(&[
        "gen_ai.usage.output_tokens",
        "gen_ai.usage.completion_tokens",
        "llm.usage.completion_tokens",
        "output_tokens",
    ]);

    let model = ["gen_ai.request.model", "llm.request.model", "model"]
        .iter()
        .find_map(|k| attrs.get(*k))
        .and_then(|v| v.as_str())
        .map(String::from);

    (input, output, model)
}

impl TraceDetails {
    /// Aggregate token counts across all spans:
    /// `(input_tokens, output_tokens, first_model_seen)`.
    ///
    /// Cost is intentionally not computed here — resolve it through a
    /// [`crate::pricing::PricingSource`] (see [`crate::pricing::compute_cost`]).
    pub fn token_totals(&self) -> (u64, u64, Option<String>) {
        let mut input = 0u64;
        let mut output = 0u64;
        let mut model: Option<String> = None;
        for span in &self.spans {
            let (inp, out, m) = extract_token_attrs(&span.attributes);
            if inp == 0 && out == 0 {
                continue;
            }
            input += inp;
            output += out;
            if model.is_none() {
                model = m;
            }
        }
        (input, output, model)
    }

    /// Per-model token totals, for pricing mixed-model traces correctly.
    pub fn token_totals_by_model(&self) -> Vec<(Option<String>, u64, u64)> {
        let mut by_model: Vec<(Option<String>, u64, u64)> = Vec::new();
        for span in &self.spans {
            let (inp, out, model) = extract_token_attrs(&span.attributes);
            if inp == 0 && out == 0 {
                continue;
            }
            match by_model.iter_mut().find(|(m, _, _)| *m == model) {
                Some((_, i, o)) => {
                    *i += inp;
                    *o += out;
                }
                None => by_model.push((model, inp, out)),
            }
        }
        by_model
    }
}

/// Span detail enriched with prompt/completion content parsed from Loki logs.
#[derive(Debug, Clone)]
pub struct SpanDetails {
    pub span: Span,
    /// Prompt content, when captured (`OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true`).
    pub input_content: Option<String>,
    /// Completion content, when captured.
    pub output_content: Option<String>,
    pub cost: CostBreakdown,
}

/// Aggregated token counts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

/// Per-agent performance stats over a time window.
#[derive(Debug, Clone)]
pub struct AgentStats {
    pub agent_id: String,
    /// Number of user-query traces in the window.
    pub trace_count: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model_used: Option<String>,
    pub latency_ms_p50: Option<f64>,
    pub latency_ms_p99: Option<f64>,
    pub cost: CostBreakdown,
    pub period_start: DateTime<Utc>,
}

/// Cost and token breakdown for a single agent in the FinOps view.
#[derive(Debug, Clone)]
pub struct AgentFinOps {
    pub agent_id: String,
    /// User-query trace count in the window.
    pub operations: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model_used: Option<String>,
    pub latency_ms_p50: Option<f64>,
    pub cost: CostBreakdown,
}

/// Sorted-percentile helper over millisecond durations.
pub fn latency_percentiles(mut durations: Vec<u64>) -> (Option<f64>, Option<f64>) {
    durations.sort_unstable();
    let len = durations.len();
    let p50 = durations.get(len / 2).map(|&v| v as f64);
    let p99 = durations
        .get((len * 99 / 100).saturating_sub(1))
        .map(|&v| v as f64);
    (p50, p99)
}
