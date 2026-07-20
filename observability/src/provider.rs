use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use crate::error::ObservabilityError;
use crate::loki::{LokiClient, parse_trace_logs};
use crate::pricing::{CostBreakdown, PricingSource, compute_cost};
use crate::tempo::{TempoClient, TraceSearchResult};
use crate::types::{
    AgentFinOps, AgentStats, Session, SessionDetails, Span, SpanDetails, TraceDetails,
    TraceSummary, extract_token_attrs, latency_percentiles,
};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstracts access to distributed trace and log data.
///
/// Data model: a **session** (A2A contextId, `session.id` span attribute)
/// groups many **traces** — one per user query — each of which contains the
/// **spans** of every agent that participated in that query.
///
/// OSS impl: [`TempoLokiProvider`] — queries Tempo and Loki directly.
/// EE impl: `RbacObservabilityProvider` — wraps the OSS impl with RBAC filtering.
#[async_trait]
pub trait ObservabilityProvider: Send + Sync {
    /// List sessions for one agent, grouping its traces by `session.id`.
    /// Traces without `session.id` are infrastructure noise and skipped.
    async fn sessions_for_agent(
        &self,
        agent_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<Session>, ObservabilityError>;

    /// Full drill-down for one session: one [`TraceSummary`] per user query.
    async fn get_session(
        &self,
        session_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<SessionDetails, ObservabilityError>;

    /// Fetch a full trace (one user query) with all spans.
    async fn get_trace(&self, trace_id: &str) -> Result<TraceDetails, ObservabilityError>;

    /// Fetch a single span, enriched with Loki prompt/completion content.
    ///
    /// `trace_id` is required because Tempo has no standalone span-search endpoint.
    async fn get_span(
        &self,
        trace_id: &str,
        span_id: &str,
    ) -> Result<SpanDetails, ObservabilityError>;

    /// Aggregate performance stats for one agent over the given window.
    async fn agent_stats(
        &self,
        agent_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<AgentStats, ObservabilityError>;

    /// Token/cost aggregation for one agent (FinOps dashboard row).
    async fn agent_finops(
        &self,
        agent_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<AgentFinOps, ObservabilityError>;

    /// Count user-query traces for an agent in a window (cheap: search only).
    async fn count_user_traces(
        &self,
        agent_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<usize, ObservabilityError>;

    /// Query raw log lines for an agent by Loki service name.
    /// Returns `(timestamp, log_line)` pairs sorted ascending.
    async fn query_logs(
        &self,
        service_name: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<(DateTime<Utc>, String)>, ObservabilityError>;

    /// Resolve a USD cost breakdown through the provider's pricing source.
    async fn cost(
        &self,
        model: Option<&str>,
        input_tokens: u64,
        output_tokens: u64,
    ) -> CostBreakdown;
}

// ---------------------------------------------------------------------------
// TraceQL helpers
// ---------------------------------------------------------------------------

/// Clamp start to at most 168 h before end (Tempo's max range).
pub fn clamp_tempo_range(start: DateTime<Utc>, end: DateTime<Utc>) -> DateTime<Utc> {
    let max_start = end - Duration::hours(168);
    if start < max_start { max_start } else { start }
}

/// TraceQL query covering all three locations where agent_id may be stored.
fn agent_query(agent_id: &str) -> String {
    format!(
        r#"{{span.agent.id="{0}"}} || {{resource.agent.id="{0}"}} || {{resource.service.name="{0}"}}"#,
        agent_id
    )
}

/// Like [`agent_query`] but restricted to traces that contain at least one
/// span with `session.id` set — i.e., user-facing request traces only,
/// excluding infrastructure traces (a2a-sdk remove_sink, dispatch loops, etc.).
fn agent_session_query(agent_id: &str) -> String {
    format!(
        r#"{{span.session.id != "" && resource.service.name="{0}"}}"#,
        agent_id
    )
}

// ---------------------------------------------------------------------------
// TempoLokiProvider — OSS implementation
// ---------------------------------------------------------------------------

/// Resolves the session ↔ trace correlation from an external mapping.
///
/// Pre-built agents (deployed via `nasiko deploy`) don't carry the
/// sitecustomize.py patch and never set `session.id` on their spans; the
/// agent_proxy records the session_id ↔ trace_id pair when it forwards A2A
/// requests. The server injects a Postgres-backed implementation.
#[async_trait]
pub trait SessionIdResolver: Send + Sync {
    async fn session_for_trace(&self, trace_id: &str) -> Option<String>;

    /// Reverse lookup: all trace_ids recorded for a session, oldest first.
    /// Default: none — only resolvers backed by a real index override this.
    async fn traces_for_session(&self, _session_id: &str) -> Vec<String> {
        Vec::new()
    }
}

/// Default resolver: no external mapping.
pub struct NoSessionIdResolver;

#[async_trait]
impl SessionIdResolver for NoSessionIdResolver {
    async fn session_for_trace(&self, _trace_id: &str) -> Option<String> {
        None
    }
}

pub struct TempoLokiProvider {
    tempo: TempoClient,
    loki: LokiClient,
    pricing: Arc<dyn PricingSource>,
    session_resolver: Arc<dyn SessionIdResolver>,
}

/// How many traces to fully fetch when aggregating tokens for stats/finops.
const TOKEN_AGGREGATION_TRACE_CAP: usize = 100;

impl TempoLokiProvider {
    pub fn new(tempo_url: String, loki_url: String, pricing: Arc<dyn PricingSource>) -> Self {
        Self {
            tempo: TempoClient::new(tempo_url),
            loki: LokiClient::new(loki_url),
            pricing,
            session_resolver: Arc::new(NoSessionIdResolver),
        }
    }

    /// Attach a fallback trace_id → session_id resolver (e.g. Redis-backed).
    pub fn with_session_resolver(mut self, resolver: Arc<dyn SessionIdResolver>) -> Self {
        self.session_resolver = resolver;
        self
    }

    async fn search_traces(
        &self,
        query: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<TraceSearchResult>, ObservabilityError> {
        let start = clamp_tempo_range(start, end);
        self.tempo
            .search(query, Some(start), Some(end), limit)
            .await
    }

    /// Fetch tokens/model/latency-p50 over up to
    /// [`TOKEN_AGGREGATION_TRACE_CAP`] traces.
    async fn aggregate_traces(&self, results: &[TraceSearchResult]) -> (u64, u64, Option<String>) {
        let mut input = 0u64;
        let mut output = 0u64;
        let mut model: Option<String> = None;

        for (trace_id, _, _) in results.iter().take(TOKEN_AGGREGATION_TRACE_CAP) {
            match self.tempo.get_trace(trace_id).await {
                Ok(trace) => {
                    let (inp, out, m) = trace.token_totals();
                    input += inp;
                    output += out;
                    if model.is_none() {
                        model = m;
                    }
                }
                Err(e) => {
                    tracing::warn!(trace_id, error = %e, "token fetch failed");
                }
            }
        }
        (input, output, model)
    }
}

/// Per-session accumulator used while grouping traces by `session.id`.
#[derive(Default)]
struct SessionAccum {
    trace_ids: Vec<String>,
    earliest_start: Option<DateTime<Utc>>,
    latest_end: Option<DateTime<Utc>>,
    total_input: u64,
    total_output: u64,
    model_used: Option<String>,
    span_durations: Vec<u64>,
}

#[async_trait]
impl ObservabilityProvider for TempoLokiProvider {
    async fn sessions_for_agent(
        &self,
        agent_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<Session>, ObservabilityError> {
        let results = self
            .search_traces(&agent_query(agent_id), start, end, 100)
            .await?;

        let mut by_session: HashMap<String, SessionAccum> = HashMap::new();

        for (trace_id, started_at, duration_ms) in results {
            let mut session_key: Option<String> = None;
            let mut trace_input = 0u64;
            let mut trace_output = 0u64;
            let mut trace_model: Option<String> = None;
            let mut trace_span_durations: Vec<u64> = Vec::new();

            if let Ok(trace) = self.tempo.get_trace(&trace_id).await {
                for span in &trace.spans {
                    if session_key.is_none() {
                        session_key = span
                            .attributes
                            .get("session.id")
                            .and_then(|v| v.as_str())
                            .map(String::from);
                    }
                    let (inp, out, model) = extract_token_attrs(&span.attributes);
                    if inp > 0 || out > 0 {
                        trace_input += inp;
                        trace_output += out;
                        if trace_model.is_none() {
                            trace_model = model;
                        }
                    }
                    let op = span
                        .attributes
                        .get("gen_ai.operation.name")
                        .and_then(|v| v.as_str());
                    if matches!(op, None | Some("chat"))
                        && let Some(d) = span.duration_ms
                    {
                        trace_span_durations.push(d);
                    }
                }
            }

            // Fallback: for pre-built agents that never set session.id on
            // spans, resolve trace_id → session_id via the injected resolver
            // (agent_proxy records the mapping when forwarding A2A requests).
            if session_key.is_none() {
                session_key = self.session_resolver.session_for_trace(&trace_id).await;
            }

            // Skip traces with no session association — a2a-sdk infrastructure
            // traces (event queue cleanup, dispatch loops, etc.), not user queries.
            let Some(key) = session_key else { continue };

            let end_time = started_at
                .zip(duration_ms)
                .map(|(s, d)| s + Duration::milliseconds(d as i64));

            let entry = by_session.entry(key).or_default();
            entry.trace_ids.push(trace_id);
            if let Some(s) = started_at {
                entry.earliest_start = Some(entry.earliest_start.map_or(s, |p| p.min(s)));
            }
            if let Some(e) = end_time {
                entry.latest_end = Some(entry.latest_end.map_or(e, |p| p.max(e)));
            }
            entry.total_input += trace_input;
            entry.total_output += trace_output;
            if entry.model_used.is_none() {
                entry.model_used = trace_model;
            }
            entry.span_durations.extend(trace_span_durations);
        }

        let mut sessions = Vec::with_capacity(by_session.len());
        for (session_id, acc) in by_session {
            let (p50, p99) = latency_percentiles(acc.span_durations);
            let cost = self
                .cost(acc.model_used.as_deref(), acc.total_input, acc.total_output)
                .await;
            let duration_ms = match (acc.earliest_start, acc.latest_end) {
                (Some(s), Some(e)) => Some((e - s).num_milliseconds().max(0) as u64),
                _ => None,
            };

            sessions.push(Session {
                session_id,
                agent_id: agent_id.to_string(),
                trace_ids: acc.trace_ids,
                started_at: acc.earliest_start,
                ended_at: acc.latest_end,
                duration_ms,
                input_tokens: acc.total_input,
                output_tokens: acc.total_output,
                model_used: acc.model_used,
                latency_ms_p50: p50,
                latency_ms_p99: p99,
                cost,
            });
        }

        sessions.sort_by_key(|s| std::cmp::Reverse(s.started_at));
        Ok(sessions)
    }

    async fn get_session(
        &self,
        session_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<SessionDetails, ObservabilityError> {
        let query = format!(r#"{{span.session.id="{session_id}"}}"#);
        let mut trace_results = self.search_traces(&query, start, end, 100).await?;

        // Agents that never set session.id on spans (anything not running the
        // Python auto-instrumentation patch): fall back to the proxy-recorded
        // session ↔ trace index.
        if trace_results.is_empty() {
            trace_results = self
                .session_resolver
                .traces_for_session(session_id)
                .await
                .into_iter()
                .map(|id| (id, None, None))
                .collect();
        }

        if trace_results.is_empty() {
            return Err(ObservabilityError::NotFound(format!(
                "session '{session_id}'"
            )));
        }

        let mut total_input = 0u64;
        let mut total_output = 0u64;
        let mut model_used: Option<String> = None;
        let mut latencies: Vec<u64> = Vec::new();
        let mut traces: Vec<TraceSummary> = Vec::new();

        for (trace_id, _, _) in &trace_results {
            let Ok(trace) = self.tempo.get_trace(trace_id).await else {
                continue;
            };
            // Resolver-sourced trace ids aren't bounded by the caller's time
            // window (the index has no TTL), so enforce it here.
            if trace.started_at.is_some_and(|s| s < start || s > end) {
                continue;
            }
            let Some(root_span) = find_root_span(&trace.spans) else {
                continue;
            };
            let root_span = root_span.clone();

            let (trace_input, trace_output, trace_model) = trace.token_totals();
            total_input += trace_input;
            total_output += trace_output;
            if model_used.is_none() {
                model_used = trace_model.clone();
            }

            // Fetch Loki prompt/completion content for the root span, best-effort.
            let content = match trace.spans.first().map(|s| s.service_name.clone()) {
                Some(svc) if !svc.is_empty() => self
                    .loki
                    .get_trace_logs(&svc, trace_id, trace.started_at, trace.ended_at)
                    .await
                    .map(parse_trace_logs)
                    .unwrap_or_default()
                    .remove(&root_span.span_id),
                _ => None,
            };

            let duration_ms = root_span.duration_ms;
            if let Some(d) = duration_ms.filter(|&d| d > 0) {
                latencies.push(d);
            }

            let cost = self
                .cost(trace_model.as_deref(), trace_input, trace_output)
                .await;

            traces.push(TraceSummary {
                trace_id: trace_id.clone(),
                root_span,
                input_tokens: trace_input,
                output_tokens: trace_output,
                model_used: trace_model,
                duration_ms,
                cost,
                input_content: content.as_ref().and_then(|c| c.input.clone()),
                output_content: content.and_then(|c| c.output),
            });
        }

        let (p50, _) = latency_percentiles(latencies);
        let cost = self
            .cost(model_used.as_deref(), total_input, total_output)
            .await;

        Ok(SessionDetails {
            session_id: session_id.to_string(),
            traces,
            input_tokens: total_input,
            output_tokens: total_output,
            model_used,
            latency_ms_p50: p50,
            cost,
        })
    }

    async fn get_trace(&self, trace_id: &str) -> Result<TraceDetails, ObservabilityError> {
        self.tempo.get_trace(trace_id).await
    }

    async fn get_span(
        &self,
        trace_id: &str,
        span_id: &str,
    ) -> Result<SpanDetails, ObservabilityError> {
        let trace = self.tempo.get_trace(trace_id).await?;
        let span = trace
            .spans
            .iter()
            .find(|s| s.span_id == span_id)
            .ok_or_else(|| {
                ObservabilityError::NotFound(format!("span '{span_id}' in trace '{trace_id}'"))
            })?
            .clone();

        // Best-effort Loki fetch. service_name comes from resource.service.name;
        // fall back to the code.namespace span attribute when unset.
        let svc = if span.service_name.is_empty() {
            span.attributes
                .get("code.namespace")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        } else {
            span.service_name.clone()
        };

        let content = if svc.is_empty() {
            None
        } else {
            // Pad the window so slight clock skew doesn't exclude logs.
            let start = trace.started_at.map(|t| t - Duration::minutes(1));
            let end = trace.ended_at.map(|t| t + Duration::minutes(1));
            match self.loki.get_trace_logs(&svc, trace_id, start, end).await {
                Ok(lines) => parse_trace_logs(lines).remove(span_id),
                Err(e) => {
                    tracing::debug!(svc, trace_id, error = %e, "loki fetch failed");
                    None
                }
            }
        };

        let (input_tokens, output_tokens, model) = extract_token_attrs(&span.attributes);
        let cost = self
            .cost(model.as_deref(), input_tokens, output_tokens)
            .await;

        Ok(SpanDetails {
            span,
            input_content: content.as_ref().and_then(|c| c.input.clone()),
            output_content: content.and_then(|c| c.output),
            cost,
        })
    }

    async fn agent_stats(
        &self,
        agent_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<AgentStats, ObservabilityError> {
        let results = self
            .search_traces(&agent_session_query(agent_id), start, end, 1000)
            .await?;

        let durations: Vec<u64> = results.iter().filter_map(|(_, _, d)| *d).collect();
        let (p50, p99) = latency_percentiles(durations);
        let (input, output, model) = self.aggregate_traces(&results).await;
        let cost = self.cost(model.as_deref(), input, output).await;

        Ok(AgentStats {
            agent_id: agent_id.to_string(),
            trace_count: results.len(),
            input_tokens: input,
            output_tokens: output,
            model_used: model,
            latency_ms_p50: p50,
            latency_ms_p99: p99,
            cost,
            period_start: start,
        })
    }

    async fn agent_finops(
        &self,
        agent_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<AgentFinOps, ObservabilityError> {
        let results = self
            .search_traces(&agent_session_query(agent_id), start, end, 1000)
            .await?;

        let durations: Vec<u64> = results.iter().filter_map(|(_, _, d)| *d).collect();
        let (p50, _) = latency_percentiles(durations);
        let (input, output, model) = self.aggregate_traces(&results).await;
        let cost = self.cost(model.as_deref(), input, output).await;

        Ok(AgentFinOps {
            agent_id: agent_id.to_string(),
            operations: results.len(),
            input_tokens: input,
            output_tokens: output,
            model_used: model,
            latency_ms_p50: p50,
            cost,
        })
    }

    async fn count_user_traces(
        &self,
        agent_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<usize, ObservabilityError> {
        let results = self
            .search_traces(&agent_session_query(agent_id), start, end, 1000)
            .await?;
        Ok(results.len())
    }

    async fn query_logs(
        &self,
        service_name: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<(DateTime<Utc>, String)>, ObservabilityError> {
        let query = format!(r#"{{service_name="{service_name}"}}"#);
        self.loki.query_range(&query, start, end, limit).await
    }

    async fn cost(
        &self,
        model: Option<&str>,
        input_tokens: u64,
        output_tokens: u64,
    ) -> CostBreakdown {
        compute_cost(self.pricing.as_ref(), model, input_tokens, output_tokens).await
    }
}

/// Root span: one whose parent is absent from the trace.
pub fn find_root_span(spans: &[Span]) -> Option<&Span> {
    let ids: std::collections::HashSet<&str> = spans.iter().map(|s| s.span_id.as_str()).collect();
    spans.iter().find(|s| {
        s.parent_span_id
            .as_ref()
            .map(|p| !ids.contains(p.as_str()))
            .unwrap_or(true)
    })
}
