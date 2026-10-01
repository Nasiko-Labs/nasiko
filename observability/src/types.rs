use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
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
    /// Unique trace IDs found by the session search. When `has_more_traces` is
    /// true this is a lower bound capped by the provider safety limit.
    pub trace_count: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Prompt tokens served from provider cache, summed over the session.
    pub cache_read_tokens: u64,
    /// Prompt tokens written to provider cache, summed over the session.
    pub cache_creation_tokens: u64,
    pub model_used: Option<String>,
    pub latency_ms_p50: Option<f64>,
    pub latency_ms_p99: Option<f64>,
    /// Mean trace duration. The percentiles above answer "how bad does it get";
    /// the session KPI strip asks for the plain average.
    pub latency_ms_avg: Option<f64>,
    /// More matching traces existed than the provider's bounded detail read.
    pub has_more_traces: bool,
    /// False when search was truncated or any matching trace failed to load.
    pub metrics_complete: bool,
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
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub model_used: Option<String>,
    pub duration_ms: Option<u64>,
    pub cost: CostBreakdown,
    /// Prompt content from Loki for the root span, when captured.
    pub input_content: Option<String>,
    /// Completion content from Loki for the root span, when captured.
    pub output_content: Option<String>,
}

/// A single event attached to a span (e.g. `gen_ai.content.prompt`,
/// `gen_ai.content.completion`, tool call records).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanEvent {
    pub name: String,
    pub timestamp: Option<DateTime<Utc>>,
    /// Event-level attributes (e.g. `gen_ai.prompt`, `gen_ai.completion`).
    pub attributes: HashMap<String, serde_json::Value>,
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
    /// Span events — prompt/completion content, tool calls, and any other
    /// structured events emitted by the OTel instrumentation.
    pub events: Vec<SpanEvent>,
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
pub fn extract_token_attrs(
    attrs: &HashMap<String, serde_json::Value>,
) -> (u64, u64, Option<String>) {
    let get_u64 = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| attrs.get(*k))
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
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

/// Extract `(cache_read_tokens, cache_creation_tokens)` from a span's
/// attributes. "Cache read" covers OpenAI-style cached prompt tokens and
/// Anthropic cache reads; "cache creation" is Anthropic-style cache writes.
/// Covers current GenAI semconv names and common instrumentation variants.
pub fn extract_cache_token_attrs(attrs: &HashMap<String, serde_json::Value>) -> (u64, u64) {
    let get_u64 = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| attrs.get(*k))
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0)
    };

    let cache_read = get_u64(&[
        "gen_ai.usage.cached_input_tokens", // semconv (experimental)
        // Dotted form, emitted by opentelemetry-instrumentation-openai. Missing it is not
        // cosmetic: cache reads then read as zero and every cached token is priced at the
        // full input rate.
        "gen_ai.usage.cache_read.input_tokens",
        "gen_ai.usage.cache_read_input_tokens",
        "gen_ai.usage.cached_tokens",
        "llm.usage.cache_read_input_tokens",
        "cache_read_input_tokens",
    ]);
    let cache_creation = get_u64(&[
        "gen_ai.usage.cache_creation.input_tokens",
        "gen_ai.usage.cache_creation_input_tokens",
        "gen_ai.usage.cache_write.input_tokens",
        "gen_ai.usage.cache_write_input_tokens",
        "llm.usage.cache_creation_input_tokens",
        "cache_creation_input_tokens",
    ]);

    (cache_read, cache_creation)
}

/// One span's token usage, with the prompt already split into its billable parts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpanUsage {
    /// Prompt tokens billed at the full input rate — cached tokens are **not** included.
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub cache_creation_5m: Option<u64>,
    pub cache_creation_1h: Option<u64>,
    pub speed: Option<String>,
    pub service_tier: Option<String>,
    pub inference_geo: Option<String>,
    pub conflicting_observations: bool,
    pub model: Option<String>,
}

impl SpanUsage {
    /// Every prompt token, cached or not. What an OpenAI response calls `prompt_tokens`.
    pub fn total_prompt(&self) -> u64 {
        self.input + self.cache_read + self.cache_creation
    }

    pub fn is_empty(&self) -> bool {
        self.input == 0 && self.output == 0 && self.cache_read == 0 && self.cache_creation == 0
    }
}

/// Extract a span's usage with the cached portion separated out.
///
/// Prefer this over calling [`extract_token_attrs`] and [`extract_cache_token_attrs`]
/// separately: costing charges `input` at the full rate and `cache_read` at the cache rate
/// and sums them, so it needs an `input` that excludes the cached tokens. Instrumentations
/// disagree about whether it already does. Known producers declare a prompt convention;
/// legacy spans fall back to the shared engine's total-based inference.
pub fn extract_usage_attrs(attrs: &HashMap<String, serde_json::Value>) -> SpanUsage {
    let (raw_input, output, model) = extract_token_attrs(attrs);
    let (cache_read, cache_creation) = extract_cache_token_attrs(attrs);
    let total = read_u64(
        attrs,
        &["gen_ai.usage.total_tokens", "llm.usage.total_tokens"],
    );

    use nasiko_pricing::{PromptConvention, RawUsage, normalize_usage};

    let convention = match attrs
        .get("nasiko.usage.prompt_convention")
        .and_then(serde_json::Value::as_str)
    {
        Some("exclusive") => PromptConvention::Exclusive,
        Some("inclusive") => PromptConvention::Inclusive,
        _ => PromptConvention::Infer,
    };
    let usage = normalize_usage(
        RawUsage {
            input: raw_input,
            output,
            cache_read,
            cache_creation,
            total,
        },
        convention,
    );
    SpanUsage {
        input: usage.input,
        output: usage.output,
        cache_read: usage.cache_read,
        cache_creation: usage.cache_creation,
        cache_creation_5m: read_u64(attrs, &["nasiko.usage.cache_creation_5m_tokens"]),
        cache_creation_1h: read_u64(attrs, &["nasiko.usage.cache_creation_1h_tokens"]),
        speed: attrs
            .get("nasiko.usage.speed")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        service_tier: attrs
            .get("nasiko.usage.service_tier")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        inference_geo: attrs
            .get("nasiko.usage.inference_geo")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        conflicting_observations: attrs
            .get("nasiko.usage.conflicting_observations")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        model,
    }
}

fn read_u64(attrs: &HashMap<String, serde_json::Value>, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|k| attrs.get(*k)).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

/// Whether this span is the LLM router's own record of a call it served.
///
/// `gen_ai.agent.id` is set only there (`oss/llm-router/src/handlers/chat.rs`),
/// which makes it an unambiguous marker. Such a span carries the **resolved**
/// provider and model — the ones actually called — where the calling agent's
/// span carries only what it asked for. The two disagree whenever an agent's
/// LLM config re-routes it, and the router is the one telling the truth.
pub fn is_router_llm_span(span: &Span) -> bool {
    span.attributes.contains_key("gen_ai.agent.id")
}

/// How each LLM-router span relates to the agent span for the same call.
pub struct RouterSpanLinks<'a> {
    /// Router span id → service name of the agent whose call it served.
    pub attributed: HashMap<&'a str, &'a str>,
    /// Span ids to leave out of any usage aggregation, because another span in
    /// the trace already accounts for the same call. Holds the agent's span
    /// where the router's superseded it, and the router's own span where it
    /// could not be attributed.
    pub excluded: HashSet<&'a str>,
}

/// Pair up each router span with the agent span describing the same LLM call.
///
/// One call produces two spans: the agent's, labelled with the model it asked
/// for, and the router's, labelled with the model that ran. Both carry the same
/// token counts, so any sum over a trace counts the call twice — and pricing the
/// agent's label charges a cheap model for an expensive call. Neither is
/// detectable downstream, which is why this pairing has to happen before any
/// aggregation rather than being patched afterwards.
///
/// The router's span sits under the HTTP span for the agent's request, whose
/// parent is the agent's own LLM span, so the nearest ancestor from a different
/// service is the calling agent, and that ancestor's usage is the duplicate.
///
/// A router span with no such ancestor is dropped instead. It reaches a trace at
/// all only because some caller propagated span context, and in this platform
/// that caller is an agent recording the same call on a span of its own — just a
/// sibling rather than an ancestor, because it propagated the context it was
/// called with instead of its own. Keeping it would double the tokens and book
/// them against the control plane, which is not an agent; dropping it costs only
/// the model correction, which is where things stood before any of this.
pub fn link_router_spans(spans: &[Span]) -> RouterSpanLinks<'_> {
    let by_id: HashMap<&str, &Span> = spans.iter().map(|s| (s.span_id.as_str(), s)).collect();
    let mut attributed = HashMap::new();
    let mut excluded = HashSet::new();
    for span in spans {
        if !is_router_llm_span(span) {
            continue;
        }
        let mut parent = span.parent_span_id.as_deref();
        let mut agent = None;
        while let Some(id) = parent {
            let Some(ancestor) = by_id.get(id) else { break };
            if ancestor.service_name != span.service_name && !ancestor.service_name.is_empty() {
                agent = Some(*ancestor);
                break;
            }
            parent = ancestor.parent_span_id.as_deref();
        }
        match agent {
            Some(agent) => {
                attributed.insert(span.span_id.as_str(), agent.service_name.as_str());
                if !extract_usage_attrs(&agent.attributes).is_empty() {
                    excluded.insert(agent.span_id.as_str());
                }
            }
            None => {
                excluded.insert(span.span_id.as_str());
            }
        }
    }
    RouterSpanLinks {
        attributed,
        excluded,
    }
}

impl TraceDetails {
    /// Aggregate token counts across all spans:
    /// `(input_tokens, output_tokens, first_model_seen)`.
    ///
    /// `input_tokens` excludes the cached prompt — [`Self::cache_token_totals`] reports that
    /// separately, and [`Self::usage_totals`] adds the two. Reading the raw span attribute
    /// here instead would leave the cached tokens inside `input` as well, so every caller
    /// that sums the classes would count them twice.
    ///
    /// Cost is intentionally not computed here — resolve it through a
    /// [`nasiko_pricing::PricingEngine`] (see [`crate::pricing::compute_cost`]).
    pub fn token_totals(&self) -> (u64, u64, Option<String>) {
        let excluded = link_router_spans(&self.spans).excluded;
        let mut seen = std::collections::HashSet::new();
        let mut input = 0u64;
        let mut output = 0u64;
        let mut model: Option<String> = None;
        for span in &self.spans {
            if !seen.insert(&span.span_id) || excluded.contains(span.span_id.as_str()) {
                continue;
            }
            let u = extract_usage_attrs(&span.attributes);
            // Skip on the whole usage, not on input/output alone: a turn served entirely
            // from cache has no fresh input and no output on that span, but it is not empty.
            if u.is_empty() {
                continue;
            }
            input += u.input;
            output += u.output;
            if model.is_none() {
                model = u.model;
            }
        }
        (input, output, model)
    }

    /// Aggregate cache token counts across all spans:
    /// `(cache_read_tokens, cache_creation_tokens)`.
    pub fn cache_token_totals(&self) -> (u64, u64) {
        let excluded = link_router_spans(&self.spans).excluded;
        let mut seen = std::collections::HashSet::new();
        let mut read = 0u64;
        let mut creation = 0u64;
        for span in &self.spans {
            if !seen.insert(&span.span_id) || excluded.contains(span.span_id.as_str()) {
                continue;
            }
            let u = extract_usage_attrs(&span.attributes);
            read += u.cache_read;
            creation += u.cache_creation;
        }
        (read, creation)
    }

    /// All four token-class totals plus the first model observed.
    pub fn usage_totals(&self) -> (TokenUsage, Option<String>) {
        let (input_tokens, output_tokens, model) = self.token_totals();
        let (cache_read_tokens, cache_creation_tokens) = self.cache_token_totals();
        (
            TokenUsage {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                total_tokens: input_tokens
                    + output_tokens
                    + cache_read_tokens
                    + cache_creation_tokens,
            },
            model,
        )
    }

    /// Per-model four-class token totals, for mixed-model trace reporting.
    pub fn token_totals_by_model(&self) -> Vec<(Option<String>, u64, u64, u64, u64)> {
        let excluded = link_router_spans(&self.spans).excluded;
        let mut seen = std::collections::HashSet::new();
        let mut by_model: Vec<(Option<String>, u64, u64, u64, u64)> = Vec::new();
        for span in &self.spans {
            if !seen.insert(&span.span_id) || excluded.contains(span.span_id.as_str()) {
                continue;
            }
            let u = extract_usage_attrs(&span.attributes);
            let (inp, out, model) = (u.input, u.output, u.model.clone());
            let (cache_read, cache_creation) = (u.cache_read, u.cache_creation);
            if u.is_empty() {
                continue;
            }
            match by_model.iter_mut().find(|(m, _, _, _, _)| *m == model) {
                Some((_, i, o, r, c)) => {
                    *i += inp;
                    *o += out;
                    *r += cache_read;
                    *c += cache_creation;
                }
                None => by_model.push((model, inp, out, cache_read, cache_creation)),
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
    pub token_usage: TokenUsage,
    pub cost: CostBreakdown,
}

/// Aggregated token counts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
}

/// Per-agent performance stats over a time window.
#[derive(Debug, Clone)]
pub struct AgentStats {
    pub agent_id: String,
    /// Number of user-query traces in the window.
    pub trace_count: usize,
    /// True when `trace_count` exceeded the token-aggregation trace cap, so
    /// `input_tokens`/`output_tokens`/`cost` were only summed over the first
    /// `TOKEN_AGGREGATION_TRACE_CAP` traces and understate the real total.
    pub is_capped: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
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
    /// True when `operations` exceeded the token-aggregation trace cap, so
    /// the token/cost fields below were only summed over the first
    /// `TOKEN_AGGREGATION_TRACE_CAP` traces and understate the real total.
    pub is_capped: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Prompt tokens served from provider cache (OpenAI cached / Anthropic cache read).
    pub cache_read_tokens: u64,
    /// Prompt tokens written to provider cache (Anthropic cache creation).
    pub cache_creation_tokens: u64,
    pub model_used: Option<String>,
    pub latency_ms_p50: Option<f64>,
    pub cost: CostBreakdown,
}

/// One row materialized from a Tempo trace for the `trace_usage` table.
/// Keyed by `(trace_id, agent_name)` — a multi-agent trace produces one row
/// per participating agent. Produced by [`ObservabilityProvider::extract_trace_usage`].
#[derive(Debug, Clone)]
pub struct TraceUsageRow {
    pub trace_id: String,
    pub agent_name: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub tool_call_count: u32,
    pub cost_estimated: bool,
    pub cost_usd: f64,
    pub prompt_cost_usd: f64,
    pub completion_cost_usd: f64,
    pub latency_ms: Option<i64>,
    pub started_at: DateTime<Utc>,
}

/// Sorted-percentile helper over millisecond durations.
pub fn latency_percentiles(mut durations: Vec<u64>) -> (Option<f64>, Option<f64>) {
    durations.sort_unstable();
    let len = durations.len();
    let nearest_rank = |percent: usize| {
        len.checked_mul(percent)
            .map(|rank| rank.div_ceil(100).saturating_sub(1))
            .and_then(|index| durations.get(index))
            .map(|&v| v as f64)
    };
    let p50 = nearest_rank(50);
    let p99 = nearest_rank(99);
    (p50, p99)
}
