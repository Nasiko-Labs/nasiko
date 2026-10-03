use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use crate::error::ObservabilityError;
use crate::loki::{LokiClient, parse_trace_logs};
use nasiko_pricing::PricingEngine;

use crate::pricing::{CostBreakdown, CostRequest, compute_cost};
use crate::tempo::{TempoClient, TraceSearchResult};
use crate::types::{
    AgentFinOps, AgentStats, Session, SessionDetails, Span, SpanDetails, TokenUsage, TraceDetails,
    TraceSummary, extract_usage_attrs, is_router_llm_span, latency_percentiles,
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

    /// Resolve a USD cost breakdown for one call.
    ///
    /// One method rather than a `cost`/`cost_with_cache` pair: the pair's
    /// default implementation folded the cache classes into `input_tokens` and
    /// so charged every cached token at the full input rate, which is the error
    /// this whole seam exists to remove.
    async fn cost(&self, request: CostRequest<'_>) -> CostBreakdown;

    /// Like [`Self::agent_finops`], additionally restricted to spans whose
    /// model attribute matches `model`. Additive trait method (default
    /// delegates to `agent_finops` when `model` is `None`) so implementors
    /// that don't override it — including any wrapper that doesn't know
    /// about model filtering yet — keep compiling and silently ignore the
    /// filter rather than failing. Callers MUST check for that: an override
    /// that can't honor a `Some(model)` filter returns `BadRequest` instead
    /// of silently returning unfiltered data.
    async fn agent_finops_filtered(
        &self,
        agent_id: &str,
        model: Option<&str>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<AgentFinOps, ObservabilityError> {
        match model {
            None => self.agent_finops(agent_id, start, end).await,
            Some(_) => Err(ObservabilityError::BadRequest(
                "model filtering not supported by this observability provider".into(),
            )),
        }
    }

    /// Like [`Self::count_user_traces`], additionally restricted by model.
    /// Same additive/default-delegates pattern as [`Self::agent_finops_filtered`].
    async fn count_user_traces_filtered(
        &self,
        agent_id: &str,
        model: Option<&str>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<usize, ObservabilityError> {
        match model {
            None => self.count_user_traces(agent_id, start, end).await,
            Some(_) => Err(ObservabilityError::BadRequest(
                "model filtering not supported by this observability provider".into(),
            )),
        }
    }

    /// Cross-agent spend bucketed by hour or day over `[start, end)`, honoring
    /// the full requested range (not the single-search 168h Tempo limit — see
    /// [`chunk_tempo_range`]). Optionally scoped to one agent and/or model.
    /// Backs the spend-over-time chart and the spend-concentration calendar.
    /// Default: not supported (empty result) — only [`TempoLokiProvider`]
    /// implements this today.
    async fn spend_timeseries(
        &self,
        _agent_id: Option<&str>,
        _model: Option<&str>,
        _start: DateTime<Utc>,
        _end: DateTime<Utc>,
        _bucket: TimeBucket,
    ) -> Result<Vec<SpendBucket>, ObservabilityError> {
        Ok(Vec::new())
    }

    /// Fetch a trace from the trace store and extract per-agent FinOps summary
    /// rows for the `trace_usage` materializer. Returns one row per agent that
    /// has token-bearing spans in the trace (multi-agent traces produce multiple
    /// rows). Returns empty when the trace has no token-bearing spans.
    ///
    /// Default: not supported — only [`TempoLokiProvider`] implements.
    async fn extract_trace_usage(
        &self,
        _trace_id: &str,
    ) -> Result<Vec<crate::types::TraceUsageRow>, ObservabilityError> {
        Ok(Vec::new())
    }

    /// Search for user-query traces across all agents in `[start, end)`.
    /// Used by the materializer to discover traces that need materialization.
    ///
    /// Default: empty — only [`TempoLokiProvider`] implements.
    async fn search_user_traces(
        &self,
        _start: DateTime<Utc>,
        _end: DateTime<Utc>,
        _limit: usize,
    ) -> Result<Vec<String>, ObservabilityError> {
        Ok(Vec::new())
    }
}

/// Granularity for [`ObservabilityProvider::spend_timeseries`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeBucket {
    Hour,
    Day,
}

impl TimeBucket {
    fn duration(self) -> Duration {
        match self {
            TimeBucket::Hour => Duration::hours(1),
            TimeBucket::Day => Duration::days(1),
        }
    }
}

/// One bucket of [`ObservabilityProvider::spend_timeseries`].
#[derive(Debug, Clone, Default)]
pub struct SpendBucket {
    pub bucket_start: DateTime<Utc>,
    pub spend_usd: f64,
    pub operations: usize,
    /// Highest-spend agent observed in this bucket, and its spend.
    pub top_agent_name: Option<String>,
    pub top_agent_spend_usd: Option<f64>,
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
    // Two separate span selectors joined by `&&` = trace-level AND.
    // `session.id` lives on the server's proxy/dispatch spans (service.name =
    // "nasiko"), while the agent's own spans carry service.name = agent name.
    // A single-selector `{A && B}` would require both on the *same* span and
    // always return zero results.
    format!(
        r#"{{span.session.id != ""}} && {{resource.service.name="{0}"}}"#,
        agent_id
    )
}

/// Like [`agent_session_query`], additionally restricted to spans whose model
/// attribute matches `model`. Model may live under any of three attribute
/// names (see `extract_token_attrs`), so this ORs across all three within the
/// `&&`-ed span selector. Delegates to `agent_session_query` unchanged when
/// `model` is `None`, so the zero-filter case is byte-identical to today.
fn agent_session_model_query(agent_id: &str, model: Option<&str>) -> String {
    let base = agent_session_query(agent_id);
    match model {
        None => base,
        Some(m) => format!(
            r#"{base} && ({{span.gen_ai.request.model="{m}"}} || {{span.llm.request.model="{m}"}} || {{span.model="{m}"}})"#
        ),
    }
}

/// Split `[start, end)` into `<=168h` sub-windows, oldest first — Tempo's
/// documented single-search limit is a per-call constraint, not a data
/// limit, so honoring a longer caller-requested range means fanning out
/// multiple searches and unioning results rather than clamping (see
/// [`clamp_tempo_range`], which stays as the cheaper single-window behavior
/// for the per-agent KPI/attribution path).
pub fn chunk_tempo_range(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    const MAX_CHUNK: i64 = 168; // hours
    if end <= start {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut cursor = start;
    while cursor < end {
        let chunk_end = (cursor + Duration::hours(MAX_CHUNK)).min(end);
        chunks.push((cursor, chunk_end));
        cursor = chunk_end;
    }
    chunks
}

// ---------------------------------------------------------------------------
// TempoLokiProvider — OSS implementation
// ---------------------------------------------------------------------------

/// Resolves the session ↔ trace correlation from an external mapping.
///
/// Agents that aren't OTel-instrumented (or whose instrumentation doesn't tag
/// spans) never set `session.id`; the agent_proxy records the session_id ↔
/// trace_id pair when it forwards A2A requests. The server injects a
/// Postgres-backed implementation.
#[async_trait]
pub trait SessionIdResolver: Send + Sync {
    async fn session_for_trace(&self, trace_id: &str) -> Option<String>;

    /// Reverse lookup: all trace_ids recorded for a session, oldest first.
    /// Default: none — only resolvers backed by a real index override this.
    async fn traces_for_session(&self, _session_id: &str) -> Vec<String> {
        Vec::new()
    }

    /// Per-agent lookup: all trace_ids the index recorded for an agent (by
    /// name) in a window. Backs the finops/stats aggregations for agents that
    /// never set `session.id` on their spans, the same way
    /// `traces_for_session` backs session drill-down. Default: none.
    async fn traces_for_agent(
        &self,
        _agent_name: &str,
        _start: DateTime<Utc>,
        _end: DateTime<Utc>,
    ) -> Vec<String> {
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
    pricing: Arc<PricingEngine>,
    session_resolver: Arc<dyn SessionIdResolver>,
}

/// How many traces to fully fetch when aggregating tokens for stats/finops.
const TOKEN_AGGREGATION_TRACE_CAP: usize = 100;
const SESSION_TRACE_PAGE_SIZE: usize = 100;
const SESSION_TRACE_SAFETY_CAP: usize = 2_000;

impl TempoLokiProvider {
    pub fn new(tempo_url: String, loki_url: String, pricing: Arc<PricingEngine>) -> Self {
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
        let results = self
            .tempo
            .search(query, Some(start), Some(end), limit)
            .await?;

        // Traces may live in Tempo's WAL but not yet flushed to searchable
        // blocks, even when older indexed traces made the bounded result nonempty.
        let unbounded = self.tempo.search(query, None, None, limit).await?;
        let mut merged = Vec::with_capacity(results.len() + unbounded.len());
        let mut seen = HashSet::new();
        append_unique_traces(&mut merged, &mut seen, results);
        append_unique_traces(
            &mut merged,
            &mut seen,
            unbounded
                .into_iter()
                .filter(|(_, at, _)| at.is_none_or(|at| at >= start && at <= end))
                .collect(),
        );
        merged.truncate(limit);
        Ok(merged)
    }

    async fn search_session_traces(
        &self,
        query: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<(Vec<TraceSearchResult>, bool), ObservabilityError> {
        let start = clamp_tempo_range(start, end);
        let mut page_end = end;
        let mut traces = Vec::new();
        let mut seen = HashSet::new();
        let mut first_page = true;

        loop {
            let mut page = self
                .tempo
                .search(query, Some(start), Some(page_end), SESSION_TRACE_PAGE_SIZE)
                .await?;
            let page_was_full = page.len() == SESSION_TRACE_PAGE_SIZE;
            if first_page {
                // Merge traces still in Tempo's WAL even when the bounded search
                // already found some indexed traces.
                let wal = self
                    .tempo
                    .search(query, None, None, SESSION_TRACE_PAGE_SIZE)
                    .await?;
                page.extend(
                    wal.into_iter()
                        .filter(|(_, at, _)| at.is_none_or(|at| at >= start && at <= end)),
                );
            }
            first_page = false;
            let next_end = older_search_boundary(&page);

            append_unique_traces(&mut traces, &mut seen, page);

            if traces.len() > SESSION_TRACE_SAFETY_CAP {
                traces.truncate(SESSION_TRACE_SAFETY_CAP);
                return Ok((traces, true));
            }
            if !page_was_full {
                return Ok((traces, false));
            }

            let Some(next_end) = next_end.filter(|next| *next >= start && *next < page_end) else {
                // A full page without a usable timestamp cannot be advanced
                // safely. Preserve the data and report it as incomplete.
                return Ok((traces, true));
            };
            page_end = next_end;
        }
    }

    /// User-query traces for one agent: the Tempo `session.id` search, unioned
    /// with the proxy-recorded session ↔ trace index. Agents that don't run
    /// the Python auto-instrumentation patch never set `session.id` on their
    /// spans, so the TraceQL search alone misses every one of their user
    /// queries — the same gap `get_session` already covers per-session.
    async fn user_traces_for_agent(
        &self,
        agent_id: &str,
        model: Option<&str>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<TraceSearchResult>, ObservabilityError> {
        let mut results = self
            .search_traces(
                &agent_session_model_query(agent_id, model),
                start,
                end,
                limit,
            )
            .await?;
        // The index has no model dimension, so only merge it in for the
        // unfiltered case — a model filter must stay Tempo-search-derived
        // only, or it would silently reintroduce unfiltered rows.
        if model.is_none() {
            let indexed = self
                .session_resolver
                .traces_for_agent(agent_id, start, end)
                .await;
            if !indexed.is_empty() {
                let known: std::collections::HashSet<String> =
                    results.iter().map(|(id, _, _)| id.clone()).collect();
                results.extend(
                    indexed
                        .into_iter()
                        .filter(|id| !known.contains(id))
                        // Start/duration unknown until the trace is fetched.
                        .map(|id| (id, None, None)),
                );
                results.truncate(limit);
            }
        }
        let mut unique = Vec::new();
        append_unique_traces(&mut unique, &mut HashSet::new(), results);
        Ok(unique)
    }

    /// Fetch tokens/model/latency-p50 over up to
    /// [`TOKEN_AGGREGATION_TRACE_CAP`] traces.
    ///
    /// Uses chunked `join_all` (8-in-flight) to avoid stampeding Tempo —
    /// same pattern as `spend_timeseries_impl`. Bails early when too many
    /// consecutive fetches fail (stale index / compacted blocks).
    async fn aggregate_traces(&self, results: &[TraceSearchResult]) -> TraceAggregates {
        let mut agg = TraceAggregates::default();

        let ids: Vec<&str> = results
            .iter()
            .take(TOKEN_AGGREGATION_TRACE_CAP)
            .map(|(id, _, _)| id.as_str())
            .collect();

        let mut consecutive_failures = 0u32;
        const MAX_CONSECUTIVE_FAILURES: u32 = 16;

        'outer: for chunk in ids.chunks(8) {
            let fetches = chunk.iter().map(|id| self.tempo.get_trace(id));
            for result in futures::future::join_all(fetches).await {
                match result {
                    Ok(trace) => {
                        consecutive_failures = 0;
                        let (usage, model) = trace.usage_totals();
                        agg.input += usage.input_tokens;
                        agg.output += usage.output_tokens;
                        agg.cache_read += usage.cache_read_tokens;
                        agg.cache_creation += usage.cache_creation_tokens;
                        agg.cost.add_assign(self.trace_cost(&trace).await);
                        if agg.model.is_none() {
                            agg.model = model;
                        }
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                            tracing::warn!(
                                consecutive_failures,
                                "aborting trace aggregation — Tempo block data likely unavailable"
                            );
                            break 'outer;
                        }
                        tracing::debug!(error = %e, "token fetch failed");
                    }
                }
            }
        }
        agg
    }

    /// Cost of a whole trace, as the session and trace views report it.
    ///
    /// Public so the differential test can hold it against
    /// [`ObservabilityProvider::extract_trace_usage`], which is what FinOps
    /// aggregates — the two must agree, and nothing but a test enforces that.
    pub async fn trace_cost(&self, trace: &TraceDetails) -> CostBreakdown {
        let mut cost = CostBreakdown::default();
        let mut seen = HashSet::new();
        for span in &trace.spans {
            if !seen.insert(&span.span_id) {
                continue;
            }
            let u = extract_usage_attrs(&span.attributes);
            if u.is_empty() {
                continue;
            }
            cost.add_assign(
                compute_cost(
                    self.pricing.as_ref(),
                    CostRequest::from_usage(
                        span_provider(span),
                        u.model.as_deref(),
                        span.started_at,
                        &u,
                    ),
                )
                .await,
            );
        }
        cost
    }

    async fn span_usage_and_cost(
        &self,
        trace: &TraceDetails,
        span: &Span,
    ) -> (TokenUsage, CostBreakdown) {
        if span.name == "coding_agent.turn" {
            return (trace.usage_totals().0, self.trace_cost(trace).await);
        }

        let u = extract_usage_attrs(&span.attributes);
        let usage = TokenUsage {
            input_tokens: u.input,
            output_tokens: u.output,
            cache_read_tokens: u.cache_read,
            cache_creation_tokens: u.cache_creation,
            total_tokens: u.total_prompt() + u.output,
        };
        let cost = self
            .cost(CostRequest::from_usage(
                span_provider(span),
                u.model.as_deref(),
                span.started_at,
                &u,
            ))
            .await;
        (usage, cost)
    }

    /// Real implementation behind `agent_finops`/`agent_finops_filtered`.
    async fn agent_finops_impl(
        &self,
        agent_id: &str,
        model: Option<&str>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<AgentFinOps, ObservabilityError> {
        let results = self
            .user_traces_for_agent(agent_id, model, start, end, 1000)
            .await?;

        let durations: Vec<u64> = results.iter().filter_map(|(_, _, d)| *d).collect();
        let (p50, _) = latency_percentiles(durations);
        let agg = self.aggregate_traces(&results).await;
        Ok(AgentFinOps {
            agent_id: agent_id.to_string(),
            operations: results.len(),
            is_capped: results.len() > TOKEN_AGGREGATION_TRACE_CAP,
            input_tokens: agg.input,
            output_tokens: agg.output,
            cache_read_tokens: agg.cache_read,
            cache_creation_tokens: agg.cache_creation,
            model_used: agg.model,
            latency_ms_p50: p50,
            cost: agg.cost,
        })
    }

    /// Cross-agent spend bucketed by hour/day, honoring the full requested
    /// range via `chunk_tempo_range` (fanned-out sub-searches, unioned)
    /// rather than the single-search 168h clamp. Buckets by each trace's
    /// start time; each trace is priced individually so mixed-model traces
    /// split correctly.
    async fn spend_timeseries_impl(
        &self,
        agent_id: Option<&str>,
        model: Option<&str>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        bucket: TimeBucket,
    ) -> Result<Vec<SpendBucket>, ObservabilityError> {
        let query = match agent_id {
            Some(id) => agent_session_model_query(id, model),
            None => match model {
                None => r#"{span.session.id != ""}"#.to_string(),
                Some(m) => format!(
                    r#"{{span.session.id != ""}} && ({{span.gen_ai.request.model="{m}"}} || {{span.llm.request.model="{m}"}} || {{span.model="{m}"}})"#
                ),
            },
        };

        let chunks = chunk_tempo_range(start, end);
        let searches = chunks
            .iter()
            .map(|(s, e)| self.tempo.search(&query, Some(*s), Some(*e), 1000));
        let chunk_results = futures::future::join_all(searches).await;

        let mut trace_ids: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for r in chunk_results.into_iter().flatten() {
            for (id, _, _) in r {
                if seen.insert(id.clone()) {
                    trace_ids.push(id);
                }
            }
        }

        // Bounded-concurrency trace fetch, chunked to 8-in-flight at a time —
        // same rationale as elsewhere in this provider: unbounded fan-out
        // would hammer Tempo. (Chunked `join_all` rather than
        // `stream::buffered`, which hits a higher-ranked-lifetime error
        // capturing `&self` across iterations.)
        let mut traces: Vec<_> = Vec::with_capacity(trace_ids.len());
        for chunk in trace_ids.chunks(8) {
            let fetches = chunk.iter().map(|id| self.tempo.get_trace(id));
            traces.extend(futures::future::join_all(fetches).await);
        }

        let bucket_seconds = bucket.duration().num_seconds();

        #[derive(Default)]
        struct BucketAcc {
            spend: f64,
            operations: usize,
            per_agent_spend: HashMap<String, f64>,
        }

        let mut buckets: HashMap<i64, BucketAcc> = HashMap::new();
        for trace in traces.into_iter().flatten() {
            let Some(started_at) = trace.started_at else {
                continue;
            };
            let (usage, _) = trace.usage_totals();
            if usage.total_tokens == 0 {
                continue;
            }
            let cost = self.trace_cost(&trace).await;
            // Acting agent: the service_name of the FIRST TOKEN-BEARING span,
            // not the trace's first span overall. A trace's first span is
            // typically the orchestrator's own root dispatch span (service
            // "nasiko-cp"), which never carries `gen_ai.usage.*` — the real
            // agent's LLM-call span is deeper in the tree. Using
            // `spans.first()` blindly attributed every orchestrator-routed
            // trace's spend to the orchestrator itself, not the downstream
            // agent that actually did the work (caught via real-infra
            // testing against a live orchestrator dispatch — a single-span
            // mock trace can't surface this, since first-span and
            // token-bearing-span are trivially the same thing there).
            let agent_name = trace
                .spans
                .iter()
                .find(|s| !extract_usage_attrs(&s.attributes).is_empty())
                .map(|s| s.service_name.clone())
                .filter(|n| !n.is_empty());

            let bucket_key = started_at.timestamp() / bucket_seconds;
            let acc = buckets.entry(bucket_key).or_default();
            acc.spend += cost.total_usd;
            acc.operations += 1;
            if let Some(name) = agent_name {
                *acc.per_agent_spend.entry(name).or_insert(0.0) += cost.total_usd;
            }
        }

        let mut out: Vec<SpendBucket> = buckets
            .into_iter()
            .map(|(key, acc)| {
                let top = acc
                    .per_agent_spend
                    .into_iter()
                    .max_by(|a, b| a.1.total_cmp(&b.1));
                SpendBucket {
                    bucket_start: DateTime::<Utc>::from_timestamp(key * bucket_seconds, 0)
                        .unwrap_or(start),
                    spend_usd: crate::pricing::round6(acc.spend),
                    operations: acc.operations,
                    top_agent_name: top.as_ref().map(|(n, _)| n.clone()),
                    top_agent_spend_usd: top.map(|(_, v)| crate::pricing::round6(v)),
                }
            })
            .collect();
        out.sort_by_key(|b| b.bucket_start);
        Ok(out)
    }
}

/// Token totals accumulated across a set of traces by `aggregate_traces`.
#[derive(Default)]
struct TraceAggregates {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
    model: Option<String>,
    cost: CostBreakdown,
}

/// Tempo search has no cursor. Its time bounds are whole epoch seconds, so move
/// to the final nanosecond of the second before the oldest result. This avoids
/// re-reading the inclusive boundary while trace-ID dedupe handles any backend
/// overlap between pages.
fn older_search_boundary(page: &[TraceSearchResult]) -> Option<DateTime<Utc>> {
    let oldest = page
        .iter()
        .filter_map(|(_, started_at, _)| *started_at)
        .min()?;
    DateTime::from_timestamp(oldest.timestamp().checked_sub(1)?, 999_999_999)
}

fn append_unique_traces(
    traces: &mut Vec<TraceSearchResult>,
    seen: &mut HashSet<String>,
    page: Vec<TraceSearchResult>,
) {
    for mut trace in page {
        trace.0 = crate::tempo::normalize_trace_id(&trace.0);
        if seen.insert(trace.0.clone()) {
            traces.push(trace);
        }
    }
}

fn session_query(session_id: &str) -> String {
    // TraceQL string literals use JSON-compatible escaping. Serializing the
    // value keeps quotes, backslashes, and control characters inside the
    // selector instead of allowing them to become TraceQL syntax.
    let literal = serde_json::to_string(session_id).expect("serializing a string cannot fail");
    format!("{{span.session.id={literal}}}")
}

fn trace_matches_session(trace: &TraceDetails, session_id: &str, resolver_sourced: bool) -> bool {
    let mut seen_spans = HashSet::new();
    let mut has_session_id = false;

    for span in &trace.spans {
        if !seen_spans.insert(&span.span_id) {
            continue;
        }
        let Some(value) = span.attributes.get("session.id").and_then(|v| v.as_str()) else {
            continue;
        };
        has_session_id = true;
        if value == session_id {
            return true;
        }
    }

    // Proxy-recorded resolver IDs are the authority for agents that do not
    // emit session.id. A conflicting emitted value is never accepted.
    resolver_sourced && !has_session_id
}

/// Per-session accumulator used while grouping traces by `session.id`.
#[derive(Default)]
struct SessionAccum {
    trace_ids: Vec<String>,
    earliest_start: Option<DateTime<Utc>>,
    latest_end: Option<DateTime<Utc>>,
    total_input: u64,
    total_output: u64,
    total_cache_read: u64,
    total_cache_creation: u64,
    cost: CostBreakdown,
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
            let mut trace_cache_read = 0u64;
            let mut trace_cache_creation = 0u64;
            let mut trace_cost = CostBreakdown::default();
            let mut trace_model: Option<String> = None;
            let mut trace_span_durations: Vec<u64> = Vec::new();

            if let Ok(trace) = self.tempo.get_trace(&trace_id).await {
                let mut seen_spans = HashSet::new();
                for span in &trace.spans {
                    if !seen_spans.insert(&span.span_id) {
                        continue;
                    }
                    if session_key.is_none() {
                        session_key = span
                            .attributes
                            .get("session.id")
                            .and_then(|v| v.as_str())
                            .map(String::from);
                    }
                    let u = extract_usage_attrs(&span.attributes);
                    if !u.is_empty() {
                        trace_input += u.input;
                        trace_output += u.output;
                        trace_cache_read += u.cache_read;
                        trace_cache_creation += u.cache_creation;
                        if trace_model.is_none() {
                            trace_model = u.model.clone();
                        }
                        trace_cost.add_assign(
                            compute_cost(
                                self.pricing.as_ref(),
                                CostRequest::from_usage(
                                    span_provider(span),
                                    u.model.as_deref(),
                                    span.started_at,
                                    &u,
                                ),
                            )
                            .await,
                        );
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
            entry.total_cache_read += trace_cache_read;
            entry.total_cache_creation += trace_cache_creation;
            entry.cost.add_assign(trace_cost);
            if entry.model_used.is_none() {
                entry.model_used = trace_model;
            }
            entry.span_durations.extend(trace_span_durations);
        }

        let mut sessions = Vec::with_capacity(by_session.len());
        for (session_id, acc) in by_session {
            let (p50, p99) = latency_percentiles(acc.span_durations);
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
                cache_read_tokens: acc.total_cache_read,
                cache_creation_tokens: acc.total_cache_creation,
                model_used: acc.model_used,
                latency_ms_p50: p50,
                latency_ms_p99: p99,
                cost: acc.cost,
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
        let query = session_query(session_id);
        let (mut trace_results, mut has_more_traces) =
            self.search_session_traces(&query, start, end).await?;
        let mut resolver_sourced = false;

        // Agents that never set session.id on spans (anything not running the
        // Python auto-instrumentation patch): fall back to the proxy-recorded
        // session ↔ trace index.
        if trace_results.is_empty() {
            resolver_sourced = true;
            trace_results = self
                .session_resolver
                .traces_for_session(session_id)
                .await
                .into_iter()
                .map(|id| (id, None, None))
                .collect();
        }

        let mut unique = Vec::new();
        append_unique_traces(&mut unique, &mut HashSet::new(), trace_results);
        let mut trace_results = unique;
        if trace_results.len() > SESSION_TRACE_SAFETY_CAP {
            trace_results.truncate(SESSION_TRACE_SAFETY_CAP);
            has_more_traces = true;
        }

        if trace_results.is_empty() {
            return Err(ObservabilityError::NotFound(format!(
                "session '{session_id}'"
            )));
        }

        let mut total_input = 0u64;
        let mut total_output = 0u64;
        let mut total_cache_read = 0u64;
        let mut total_cache_creation = 0u64;
        let mut total_cost = CostBreakdown::default();
        let mut model_used: Option<String> = None;
        let mut latencies: Vec<u64> = Vec::new();
        let mut traces: Vec<TraceSummary> = Vec::new();
        let trace_count = trace_results.len();
        let mut trace_fetch_failed = false;

        for (trace_id, _, _) in &trace_results {
            let trace = match self.tempo.get_trace(trace_id).await {
                Ok(trace) => trace,
                Err(error) => {
                    trace_fetch_failed = true;
                    tracing::warn!(trace_id, %error, "session trace fetch failed");
                    continue;
                }
            };
            if !trace_matches_session(&trace, session_id, resolver_sourced) {
                trace_fetch_failed = true;
                tracing::warn!(
                    trace_id,
                    session_id,
                    resolver_sourced,
                    "session trace did not match requested session"
                );
                continue;
            }
            // Resolver-sourced trace ids aren't bounded by the caller's time
            // window (the index has no TTL), so enforce it here.
            if trace.started_at.is_some_and(|s| s < start || s > end) {
                trace_fetch_failed = true;
                continue;
            }
            let Some(root_span) = find_root_span(&trace.spans) else {
                trace_fetch_failed = true;
                continue;
            };
            let root_span = root_span.clone();

            let (trace_usage, trace_model) = trace.usage_totals();
            total_input += trace_usage.input_tokens;
            total_output += trace_usage.output_tokens;
            total_cache_read += trace_usage.cache_read_tokens;
            total_cache_creation += trace_usage.cache_creation_tokens;
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

            let cost = self.trace_cost(&trace).await;
            total_cost.add_assign(cost);

            // Content precedence: Loki events, then GenAI semconv span attributes
            // recorded directly on the root span (gen_ai.input/output.messages).
            let attr_content = |key: &str| {
                root_span
                    .attributes
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(String::from)
            };
            let input_content = content
                .as_ref()
                .and_then(|c| c.input.clone())
                .or_else(|| attr_content("gen_ai.input.messages"));
            let output_content = content
                .and_then(|c| c.output)
                .or_else(|| attr_content("gen_ai.output.messages"));

            traces.push(TraceSummary {
                trace_id: trace_id.clone(),
                root_span,
                input_tokens: trace_usage.input_tokens,
                output_tokens: trace_usage.output_tokens,
                cache_read_tokens: trace_usage.cache_read_tokens,
                cache_creation_tokens: trace_usage.cache_creation_tokens,
                model_used: trace_model,
                duration_ms,
                cost,
                input_content,
                output_content,
            });
        }

        let avg = (!latencies.is_empty())
            .then(|| latencies.iter().sum::<u64>() as f64 / latencies.len() as f64);
        let (p50, p99) = latency_percentiles(latencies);
        let metrics_complete = !has_more_traces && !trace_fetch_failed;

        Ok(SessionDetails {
            session_id: session_id.to_string(),
            traces,
            trace_count,
            input_tokens: total_input,
            output_tokens: total_output,
            cache_read_tokens: total_cache_read,
            cache_creation_tokens: total_cache_creation,
            model_used,
            latency_ms_p50: metrics_complete.then_some(p50).flatten(),
            latency_ms_p99: metrics_complete.then_some(p99).flatten(),
            latency_ms_avg: metrics_complete.then_some(avg).flatten(),
            has_more_traces,
            metrics_complete,
            cost: total_cost,
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

        let (token_usage, cost) = self.span_usage_and_cost(&trace, &span).await;

        Ok(SpanDetails {
            span,
            input_content: content.as_ref().and_then(|c| c.input.clone()),
            output_content: content.and_then(|c| c.output),
            token_usage,
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
            .user_traces_for_agent(agent_id, None, start, end, 1000)
            .await?;

        let durations: Vec<u64> = results.iter().filter_map(|(_, _, d)| *d).collect();
        let (p50, p99) = latency_percentiles(durations);
        let agg = self.aggregate_traces(&results).await;

        Ok(AgentStats {
            agent_id: agent_id.to_string(),
            trace_count: results.len(),
            is_capped: results.len() > TOKEN_AGGREGATION_TRACE_CAP,
            input_tokens: agg.input,
            output_tokens: agg.output,
            cache_read_tokens: agg.cache_read,
            cache_creation_tokens: agg.cache_creation,
            model_used: agg.model,
            latency_ms_p50: p50,
            latency_ms_p99: p99,
            cost: agg.cost,
            period_start: start,
        })
    }

    async fn agent_finops(
        &self,
        agent_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<AgentFinOps, ObservabilityError> {
        self.agent_finops_impl(agent_id, None, start, end).await
    }

    async fn agent_finops_filtered(
        &self,
        agent_id: &str,
        model: Option<&str>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<AgentFinOps, ObservabilityError> {
        self.agent_finops_impl(agent_id, model, start, end).await
    }

    async fn count_user_traces(
        &self,
        agent_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<usize, ObservabilityError> {
        let results = self
            .user_traces_for_agent(agent_id, None, start, end, 1000)
            .await?;
        Ok(results.len())
    }

    async fn count_user_traces_filtered(
        &self,
        agent_id: &str,
        model: Option<&str>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<usize, ObservabilityError> {
        let results = self
            .user_traces_for_agent(agent_id, model, start, end, 1000)
            .await?;
        Ok(results.len())
    }

    async fn spend_timeseries(
        &self,
        agent_id: Option<&str>,
        model: Option<&str>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        bucket: TimeBucket,
    ) -> Result<Vec<SpendBucket>, ObservabilityError> {
        self.spend_timeseries_impl(agent_id, model, start, end, bucket)
            .await
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

    async fn cost(&self, request: CostRequest<'_>) -> CostBreakdown {
        compute_cost(self.pricing.as_ref(), request).await
    }

    async fn extract_trace_usage(
        &self,
        trace_id: &str,
    ) -> Result<Vec<crate::types::TraceUsageRow>, ObservabilityError> {
        let trace = self.tempo.get_trace(trace_id).await?;
        let trace_id = trace.trace_id.as_str();

        // Session ID from span attributes (shared across all agents in the trace).
        let session_id = trace.spans.iter().find_map(|s| {
            s.attributes
                .get("session.id")
                .and_then(|v| v.as_str())
                .filter(|sid| !sid.is_empty())
                .map(|sid| sid.to_string())
        });

        let started_at = trace.started_at.unwrap_or_else(Utc::now);
        let latency_ms = trace.duration_ms.map(|d| d as i64);

        // Group token-bearing and tool-call spans by agent (service_name).
        // Each agent that made LLM calls or tool calls in this trace gets its
        // own row — a multi-agent trace produces multiple rows.
        struct AgentAcc {
            input: u64,
            output: u64,
            cache_read: u64,
            cache_creation: u64,
            model: Option<String>,
            /// Provider label off the span, so an agent's spend is costed
            /// against the book of whoever actually served the call.
            provider: Option<String>,
            mixed_model: bool,
            mixed_provider: bool,
            cost: CostBreakdown,
            tool_calls: u32,
        }

        let mut by_agent: HashMap<String, AgentAcc> = HashMap::new();
        // Deduplicated like every other aggregator here: Tempo can return a
        // span more than once (a re-export, or a batch replayed), and this is
        // the function that materializes `trace_usage`, so a duplicate would
        // double-count tokens and cost on every FinOps figure while the
        // session view — which does dedup — stayed right. Done once up front
        // because the attribution pass below walks the span set as well.
        let mut seen: HashSet<&str> = HashSet::new();
        let mut spans: Vec<&crate::types::Span> = Vec::new();
        for span in &trace.spans {
            if seen.insert(span.span_id.as_str()) {
                spans.push(span);
            }
        }
        // Pair each LLM-router span with the agent span for the same call, so the
        // call is counted once and priced against the model that actually served
        // it. Shared with the session view's aggregation (`TraceDetails`), which
        // must reach the same totals — `cost_path_differential` is the gate on that.
        let links = crate::types::link_router_spans(&trace.spans);
        let (attributed, excluded) = (links.attributed, links.excluded);

        for span in spans.iter().copied() {
            let u = extract_usage_attrs(&span.attributes);
            let (inp, out, model) = (u.input, u.output, u.model.clone());
            let (cr, cc) = (u.cache_read, u.cache_creation);
            // gen_ai.operation.name = "call_tool" (GenAI semconv) or
            // openinference.span.kind = "TOOL" (OpenInference convention).
            let is_tool_call = span
                .attributes
                .get("gen_ai.operation.name")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == "call_tool")
                || span
                    .attributes
                    .get("openinference.span.kind")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case("tool"));
            if inp == 0 && out == 0 && cr == 0 && cc == 0 && !is_tool_call {
                continue;
            }
            // The router already booked this call, with the model that actually
            // served it. Counting the agent's copy too would double the tokens.
            if excluded.contains(span.span_id.as_str()) && !is_tool_call {
                continue;
            }
            // A router span is booked against the agent it served, never against
            // the control plane; one we could not attribute is dropped above.
            let name = if is_router_llm_span(span) {
                match attributed.get(span.span_id.as_str()) {
                    Some(agent) => *agent,
                    None => continue,
                }
            } else {
                span.service_name.as_str()
            };
            if name.is_empty() {
                continue;
            }
            let acc = by_agent.entry(name.to_string()).or_insert(AgentAcc {
                input: 0,
                output: 0,
                cache_read: 0,
                cache_creation: 0,
                model: None,
                provider: span_provider(span).map(str::to_owned),
                mixed_model: false,
                mixed_provider: false,
                cost: CostBreakdown::default(),
                tool_calls: 0,
            });
            acc.input += inp;
            acc.output += out;
            acc.cache_read += cr;
            acc.cache_creation += cc;
            if acc.model.is_none() && !acc.mixed_model {
                acc.model = model.clone();
            } else if acc.model != model {
                acc.model = None;
                acc.mixed_model = true;
            }
            let provider = span_provider(span).map(str::to_owned);
            if acc.provider != provider {
                acc.provider = None;
                acc.mixed_provider = true;
            }
            acc.cost.add_assign(
                self.cost(CostRequest::from_usage(
                    span_provider(span),
                    u.model.as_deref(),
                    span.started_at,
                    &u,
                ))
                .await,
            );
            if is_tool_call {
                acc.tool_calls += 1;
            }
        }

        let mut rows = Vec::with_capacity(by_agent.len());
        for (agent_name, acc) in by_agent {
            let cost = acc.cost;
            rows.push(crate::types::TraceUsageRow {
                trace_id: trace_id.to_string(),
                agent_name,
                session_id: session_id.clone(),
                model: acc.model,
                provider: if acc.mixed_provider {
                    None
                } else {
                    acc.provider
                },
                input_tokens: acc.input,
                output_tokens: acc.output,
                cache_read_tokens: acc.cache_read,
                cache_creation_tokens: acc.cache_creation,
                tool_call_count: acc.tool_calls,
                cost_estimated: cost.estimated,
                cost_usd: cost.total_usd,
                prompt_cost_usd: cost.prompt_usd,
                completion_cost_usd: cost.completion_usd,
                latency_ms,
                started_at,
            });
        }
        Ok(rows)
    }

    async fn search_user_traces(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<String>, ObservabilityError> {
        let chunks = chunk_tempo_range(start, end);
        let mut seen = std::collections::HashSet::new();
        let mut ids = Vec::new();

        for (s, e) in chunks {
            let results = self
                .tempo
                .search(r#"{span.session.id != ""}"#, Some(s), Some(e), limit)
                .await?;
            for (id, _, _) in results {
                if seen.insert(id.clone()) {
                    ids.push(id);
                }
            }
        }
        Ok(ids)
    }
}

/// The provider that served a span, as the GenAI semconv reports it.
///
/// `gen_ai.provider.name` is the current attribute; `gen_ai.system` is the older
/// spelling that instrumentation in the wild still emits. Resolving it matters
/// because providers resell the same model at their own rates — Bedrock charges
/// $5/$25 for a Claude that costs $15/$75 direct — so pricing without it silently
/// costs a call against whichever book happens to carry the model's name.
pub fn span_provider(span: &Span) -> Option<&str> {
    ["gen_ai.provider.name", "gen_ai.system"]
        .iter()
        .find_map(|key| span.attributes.get(*key))
        .and_then(|v| v.as_str())
        .filter(|label| !label.is_empty())
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

#[cfg(test)]
mod tests {
    /// List prices only — hermetic, per the repo's unit-test rule.
    fn test_engine() -> nasiko_pricing::PricingEngine {
        nasiko_pricing::PricingEngine::offline()
    }

    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    use chrono::{Duration, TimeZone, Utc};

    use super::*;

    fn trace_with_sessions(values: &[(&str, Option<&str>)]) -> TraceDetails {
        let spans = values
            .iter()
            .map(|(span_id, session_id)| {
                let mut attributes = HashMap::new();
                if let Some(session_id) = session_id {
                    attributes.insert("session.id".into(), serde_json::json!(session_id));
                }
                Span {
                    span_id: (*span_id).into(),
                    parent_span_id: None,
                    name: "test".into(),
                    started_at: Utc.with_ymd_and_hms(2026, 8, 19, 12, 0, 0).unwrap(),
                    ended_at: None,
                    duration_ms: None,
                    service_name: "test".into(),
                    kind: 1,
                    status_code: 0,
                    status_message: String::new(),
                    attributes,
                    events: vec![],
                }
            })
            .collect();
        TraceDetails {
            trace_id: "trace-1".into(),
            spans,
            started_at: None,
            ended_at: None,
            duration_ms: None,
        }
    }

    fn model_span(
        span_id: &str,
        parent_span_id: Option<&str>,
        model: &str,
        usage: (u64, u64, u64, u64),
    ) -> Span {
        let mut span = trace_with_sessions(&[(span_id, None)]).spans.remove(0);
        span.parent_span_id = parent_span_id.map(str::to_owned);
        span.name = format!("chat {model}");
        span.attributes
            .insert("gen_ai.request.model".into(), serde_json::json!(model));
        span.attributes.insert(
            "gen_ai.usage.input_tokens".into(),
            serde_json::json!(usage.0),
        );
        span.attributes.insert(
            "gen_ai.usage.output_tokens".into(),
            serde_json::json!(usage.1),
        );
        span.attributes.insert(
            "gen_ai.usage.cache_read_input_tokens".into(),
            serde_json::json!(usage.2),
        );
        span.attributes.insert(
            "gen_ai.usage.cache_creation_input_tokens".into(),
            serde_json::json!(usage.3),
        );
        // These fixtures use the disjoint convention — `input_tokens` excludes the cached
        // counts. Say so via `total_tokens` rather than leaving it to be inferred: without it
        // the counts are equally consistent with the inclusive (OpenAI) convention, and
        // `extract_usage_attrs` would have to guess. Real instrumentation emits this too.
        span.attributes.insert(
            "gen_ai.usage.total_tokens".into(),
            serde_json::json!(usage.0 + usage.1 + usage.2 + usage.3),
        );
        span
    }

    #[tokio::test]
    async fn mixed_model_trace_costs_each_span_with_its_own_rates() {
        let provider = TempoLokiProvider::new(
            "http://tempo.invalid".into(),
            "http://loki.invalid".into(),
            std::sync::Arc::new(test_engine()),
        );
        let trace = TraceDetails {
            trace_id: "mixed".into(),
            spans: vec![
                model_span("gpt", None, "gpt-4o", (1_000_000, 0, 0, 0)),
                model_span(
                    "claude",
                    None,
                    "claude-sonnet-4",
                    (0, 1_000_000, 1_000_000, 1_000_000),
                ),
            ],
            started_at: None,
            ended_at: None,
            duration_ms: None,
        };

        let cost = provider.trace_cost(&trace).await;
        assert_eq!(cost.prompt_usd, 2.5);
        assert_eq!(cost.completion_usd, 15.0);
        assert_eq!(cost.cache_read_usd, 0.3);
        assert_eq!(cost.cache_creation_usd, 3.75);
        assert_eq!(cost.total_usd, 21.55);
    }

    #[tokio::test]
    async fn coding_agent_root_is_aggregate_and_child_is_per_call() {
        let provider = TempoLokiProvider::new(
            "http://tempo.invalid".into(),
            "http://loki.invalid".into(),
            std::sync::Arc::new(test_engine()),
        );
        let mut root = trace_with_sessions(&[("root", None)]).spans.remove(0);
        root.name = "coding_agent.turn".into();
        let child = model_span("child", Some("root"), "claude-sonnet-4", (10, 5, 2, 3));
        let zero = model_span("zero", Some("root"), "gpt-4o", (0, 0, 0, 0));
        let trace = TraceDetails {
            trace_id: "turn".into(),
            spans: vec![root.clone(), child.clone(), zero.clone()],
            started_at: None,
            ended_at: None,
            duration_ms: None,
        };

        let (root_usage, root_cost) = provider.span_usage_and_cost(&trace, &root).await;
        let (child_usage, child_cost) = provider.span_usage_and_cost(&trace, &child).await;
        let (zero_usage, zero_cost) = provider.span_usage_and_cost(&trace, &zero).await;

        assert_eq!(root_usage.total_tokens, 20);
        assert_eq!(root_cost, child_cost);
        assert_eq!(child_usage.total_tokens, 20);
        assert_eq!(zero_usage.total_tokens, 0);
        assert_eq!(zero_cost, Default::default());
    }

    #[test]
    fn session_query_escapes_traceql_string_literal() {
        let session_id = "quote\" backslash\\ newline\n";
        let query = session_query(session_id);
        let literal = query
            .strip_prefix("{span.session.id=")
            .and_then(|query| query.strip_suffix('}'))
            .unwrap();

        assert_eq!(serde_json::from_str::<String>(literal).unwrap(), session_id);
        assert_eq!(
            query,
            "{span.session.id=\"quote\\\" backslash\\\\ newline\\n\"}"
        );
    }

    #[test]
    fn injected_session_query_cannot_authorize_another_sessions_trace() {
        let payload = "attacker\"} || {true} || {span.session.id=\"victim";
        let query = session_query(payload);
        let literal = query
            .strip_prefix("{span.session.id=")
            .and_then(|query| query.strip_suffix('}'))
            .unwrap();
        let victim_trace = trace_with_sessions(&[("span-1", Some("victim"))]);

        assert_eq!(serde_json::from_str::<String>(literal).unwrap(), payload);
        assert!(!trace_matches_session(&victim_trace, payload, false));
        assert!(!trace_matches_session(&victim_trace, payload, true));
    }

    #[test]
    fn direct_and_resolver_session_matching_use_deduplicated_spans() {
        let direct = trace_with_sessions(&[("span-1", Some("requested"))]);
        let replay_conflict =
            trace_with_sessions(&[("span-1", Some("other")), ("span-1", Some("requested"))]);
        let missing = trace_with_sessions(&[("span-1", None)]);

        assert!(trace_matches_session(&direct, "requested", false));
        assert!(!trace_matches_session(&replay_conflict, "requested", false));
        assert!(!trace_matches_session(&missing, "requested", false));
        assert!(trace_matches_session(&missing, "requested", true));
    }

    #[test]
    fn session_search_boundary_moves_before_oldest_result_second() {
        let newest =
            Utc.with_ymd_and_hms(2026, 8, 19, 12, 0, 10).unwrap() + Duration::milliseconds(800);
        let oldest =
            Utc.with_ymd_and_hms(2026, 8, 19, 12, 0, 5).unwrap() + Duration::milliseconds(200);
        let page = vec![
            ("newest".into(), Some(newest), None),
            ("oldest".into(), Some(oldest), None),
        ];

        let boundary = older_search_boundary(&page).unwrap();

        assert_eq!(boundary.timestamp(), oldest.timestamp() - 1);
        assert!(boundary < oldest);
    }

    #[test]
    fn session_search_pages_dedupe_trace_ids_at_boundaries() {
        let at = Utc.with_ymd_and_hms(2026, 8, 19, 12, 0, 0).unwrap();
        let mut traces = Vec::new();
        let mut seen = HashSet::new();
        let first_page = (0..100)
            .map(|id| (format!("trace-{id}"), Some(at), None))
            .collect();
        let second_page = (99..199)
            .map(|id| (format!("trace-{id}"), Some(at), None))
            .collect();

        append_unique_traces(&mut traces, &mut seen, first_page);
        append_unique_traces(&mut traces, &mut seen, second_page);

        assert_eq!(traces.len(), 199);
        assert_eq!(traces.first().unwrap().0, "trace-0");
        assert_eq!(traces.last().unwrap().0, "trace-198");
    }

    // ── chunk_tempo_range ───────────────────────────────────────────────────

    #[test]
    fn chunk_tempo_range_empty_when_end_before_or_equal_start() {
        let t = Utc::now();
        assert!(chunk_tempo_range(t, t).is_empty(), "end == start");
        assert!(
            chunk_tempo_range(t, t - Duration::hours(1)).is_empty(),
            "end < start"
        );
    }

    #[test]
    fn chunk_tempo_range_single_chunk_under_168h() {
        let end = Utc::now();
        let start = end - Duration::hours(1);
        let chunks = chunk_tempo_range(start, end);
        assert_eq!(chunks, vec![(start, end)]);
    }

    #[test]
    fn chunk_tempo_range_single_chunk_at_exactly_168h() {
        let end = Utc::now();
        let start = end - Duration::hours(168);
        let chunks = chunk_tempo_range(start, end);
        assert_eq!(
            chunks,
            vec![(start, end)],
            "168h is the boundary, not over it"
        );
    }

    #[test]
    fn chunk_tempo_range_splits_just_over_168h_into_two() {
        let end = Utc::now();
        let start = end - Duration::hours(169);
        let chunks = chunk_tempo_range(start, end);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].0, start);
        assert_eq!(chunks[0].1, start + Duration::hours(168));
        assert_eq!(chunks[1].0, chunks[0].1, "chunks must be contiguous");
        assert_eq!(chunks[1].1, end, "last chunk must end exactly at `end`");
    }

    #[test]
    fn chunk_tempo_range_30_days_splits_into_five_chunks_with_short_tail() {
        // 30 days = 720h = 4*168 + 48 -> 4 full 168h chunks + one 48h tail.
        let end = Utc::now();
        let start = end - Duration::days(30);
        let chunks = chunk_tempo_range(start, end);
        assert_eq!(chunks.len(), 5);
        for c in &chunks[..4] {
            assert_eq!(c.1 - c.0, Duration::hours(168));
        }
        let tail = chunks[4];
        assert_eq!(tail.1 - tail.0, Duration::hours(48));
        assert_eq!(tail.1, end);
        // Contiguity end-to-end, and total coverage equals the input range.
        for w in chunks.windows(2) {
            assert_eq!(w[0].1, w[1].0);
        }
        assert_eq!(chunks.first().unwrap().0, start);
    }

    // ── TraceQL query builders ──────────────────────────────────────────────

    #[test]
    fn agent_session_model_query_with_no_model_matches_agent_session_query_exactly() {
        assert_eq!(
            agent_session_model_query("my-agent", None),
            agent_session_query("my-agent"),
        );
    }

    #[test]
    fn agent_session_model_query_with_model_ands_in_an_or_group_over_three_attr_names() {
        let q = agent_session_model_query("my-agent", Some("gpt-4o"));
        assert!(q.starts_with(&agent_session_query("my-agent")));
        assert!(q.contains(r#"{span.gen_ai.request.model="gpt-4o"}"#));
        assert!(q.contains(r#"{span.llm.request.model="gpt-4o"}"#));
        assert!(q.contains(r#"{span.model="gpt-4o"}"#));
        // The three model predicates must be OR'd together, not AND'd — any
        // one of the three attribute names should be enough to match.
        assert!(q.contains(" || "));
        // ...and that OR group itself must be AND'd onto the base query, not
        // OR'd with it (an unscoped OR would match spans from ANY agent).
        assert!(q.contains(" && ("));
    }

    // ── is_capped / operations bookkeeping (mockito) ────────────────────────

    fn provider_against(base_url: &str) -> TempoLokiProvider {
        TempoLokiProvider::new(
            base_url.to_string(),
            "http://127.0.0.1:1".to_string(), // unused by the paths under test
            Arc::new(test_engine()),
        )
    }

    fn search_response_with_n_traces(n: usize) -> serde_json::Value {
        let traces: Vec<_> = (0..n)
            .map(|i| {
                serde_json::json!({
                    "traceID": format!("trace-{i:04}"),
                    "startTimeUnixNano": "1700000000000000000",
                    "durationMs": 50,
                })
            })
            .collect();
        serde_json::json!({ "traces": traces })
    }

    #[tokio::test]
    async fn agent_finops_reports_is_capped_true_and_full_operation_count_beyond_the_trace_cap() {
        let mut server = mockito::Server::new_async().await;
        // 105 traces in the search result — over TOKEN_AGGREGATION_TRACE_CAP (100).
        let _search = server
            .mock("GET", "/api/search")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(search_response_with_n_traces(105).to_string())
            .create_async()
            .await;
        // Every individual trace fetch 404s — aggregate_traces must warn and
        // skip rather than fail the whole call, and it must still only
        // attempt up to the cap (this mock alone can't observe call count,
        // but a non-panicking, correctly-summarized result proves the cap is
        // respected without the token side blowing up).
        let _traces = server
            .mock("GET", mockito::Matcher::Regex(r"^/api/traces/.*$".into()))
            .with_status(404)
            .create_async()
            .await;

        let provider = provider_against(&server.url());
        let now = Utc::now();
        let finops = provider
            .agent_finops("busy-agent", now - Duration::hours(1), now)
            .await
            .expect("search succeeded even though every trace fetch 404s");

        assert_eq!(
            finops.operations, 105,
            "operations reflects the full search count"
        );
        assert!(finops.is_capped, "105 > 100-trace cap");
        assert_eq!(
            finops.input_tokens, 0,
            "no trace fetch succeeded, so no tokens"
        );
    }

    #[tokio::test]
    async fn agent_finops_reports_is_capped_false_under_the_trace_cap() {
        let mut server = mockito::Server::new_async().await;
        let _search = server
            .mock("GET", "/api/search")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(search_response_with_n_traces(3).to_string())
            .create_async()
            .await;
        let _traces = server
            .mock("GET", mockito::Matcher::Regex(r"^/api/traces/.*$".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                otlp_trace_json(
                    "busy-agent",
                    "1700000000000000000",
                    100,
                    50,
                    Some("gpt-4o-mini"),
                )
                .to_string(),
            )
            .create_async()
            .await;

        let provider = provider_against(&server.url());
        let now = Utc::now();
        let finops = provider
            .agent_finops("busy-agent", now - Duration::hours(1), now)
            .await
            .unwrap();

        assert_eq!(finops.operations, 3);
        assert!(!finops.is_capped);
        // 3 traces x 100 input / 50 output tokens each.
        assert_eq!(finops.input_tokens, 300);
        assert_eq!(finops.output_tokens, 150);
        // gpt-4o-mini: (0.15, 0.60) USD/1M — see pricing.rs's static table.
        let expected_cost = (300.0 / 1_000_000.0 * 0.15) + (150.0 / 1_000_000.0 * 0.60);
        assert!(
            (finops.cost.total_usd - expected_cost).abs() < 1e-9,
            "cost {} != expected {expected_cost}",
            finops.cost.total_usd
        );
    }

    #[tokio::test]
    async fn agent_finops_filtered_sends_the_model_predicate_and_none_falls_back_to_unfiltered() {
        let mut server = mockito::Server::new_async().await;
        let expected_unfiltered = agent_session_model_query("my-agent", None);
        let expected_filtered = agent_session_model_query("my-agent", Some("gpt-4o"));
        assert_ne!(expected_unfiltered, expected_filtered);

        let _unfiltered = server
            .mock("GET", "/api/search")
            .match_query(mockito::Matcher::UrlEncoded(
                "q".into(),
                expected_unfiltered.clone(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(search_response_with_n_traces(1).to_string())
            .create_async()
            .await;
        let _filtered = server
            .mock("GET", "/api/search")
            .match_query(mockito::Matcher::UrlEncoded(
                "q".into(),
                expected_filtered.clone(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(search_response_with_n_traces(2).to_string())
            .create_async()
            .await;
        let _traces = server
            .mock("GET", mockito::Matcher::Regex(r"^/api/traces/.*$".into()))
            .with_status(404)
            .create_async()
            .await;

        let provider = provider_against(&server.url());
        let now = Utc::now();
        let start = now - Duration::hours(1);

        let unfiltered = provider
            .agent_finops_filtered("my-agent", None, start, now)
            .await
            .unwrap();
        assert_eq!(
            unfiltered.operations, 1,
            "None routes through the unfiltered TraceQL"
        );

        let filtered = provider
            .agent_finops_filtered("my-agent", Some("gpt-4o"), start, now)
            .await
            .unwrap();
        assert_eq!(
            filtered.operations, 2,
            "Some(model) routes through the model-scoped TraceQL"
        );
    }

    // ── spend_timeseries bucketing + top-agent selection ────────────────────

    #[tokio::test]
    async fn spend_timeseries_buckets_by_hour_and_picks_the_highest_spender_per_bucket() {
        let mut server = mockito::Server::new_async().await;

        // Two traces in the SAME hour bucket (12:00-12:59 UTC on 2023-11-14),
        // from two different agents with different spend; one trace in the
        // NEXT hour bucket, from a third agent.
        // 1700000000 UTC = 2023-11-14T22:13:20Z; pick round numbers instead.
        let bucket0_start_nanos: i64 = 1_700_000_000_000_000_000; // arbitrary anchor
        let bucket0_ts = bucket0_start_nanos.to_string();
        let bucket1_ts = (bucket0_start_nanos + 3_600_000_000_000).to_string(); // +1h

        let search_body = serde_json::json!({
            "traces": [
                {"traceID": "t-cheap", "startTimeUnixNano": bucket0_ts, "durationMs": 10},
                {"traceID": "t-expensive", "startTimeUnixNano": bucket0_ts, "durationMs": 10},
                {"traceID": "t-next-hour", "startTimeUnixNano": bucket1_ts, "durationMs": 10},
            ]
        });
        let _search = server
            .mock("GET", "/api/search")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(search_body.to_string())
            .create_async()
            .await;

        let _t_cheap = server
            .mock("GET", "/api/traces/t-cheap")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                otlp_trace_json("cheap-agent", &bucket0_ts, 100, 100, Some("gpt-4o-mini"))
                    .to_string(),
            )
            .create_async()
            .await;
        let _t_expensive = server
            .mock("GET", "/api/traces/t-expensive")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                otlp_trace_json(
                    "expensive-agent",
                    &bucket0_ts,
                    1_000_000,
                    1_000_000,
                    Some("gpt-4o"),
                )
                .to_string(),
            )
            .create_async()
            .await;
        let _t_next_hour = server
            .mock("GET", "/api/traces/t-next-hour")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                otlp_trace_json("solo-agent", &bucket1_ts, 500, 500, Some("gpt-4o-mini"))
                    .to_string(),
            )
            .create_async()
            .await;

        let provider = provider_against(&server.url());
        let start =
            DateTime::<Utc>::from_timestamp(bucket0_start_nanos / 1_000_000_000 - 60, 0).unwrap();
        let end = start + Duration::hours(3);

        let buckets = provider
            .spend_timeseries(None, None, start, end, TimeBucket::Hour)
            .await
            .unwrap();

        assert_eq!(buckets.len(), 2, "two distinct hour buckets were populated");

        let b0 = &buckets[0];
        assert_eq!(b0.operations, 2, "two traces landed in the first bucket");
        assert_eq!(
            b0.top_agent_name.as_deref(),
            Some("expensive-agent"),
            "the far larger spend must win top_agent, not just the last-seen trace"
        );
        assert!(
            b0.top_agent_spend_usd.unwrap() > 1.0,
            "expensive-agent's 1M/1M gpt-4o tokens should cost multiple dollars"
        );
        assert!(
            b0.spend_usd > b0.top_agent_spend_usd.unwrap(),
            "bucket spend must include BOTH agents, not just the top one"
        );

        let b1 = &buckets[1];
        assert_eq!(b1.operations, 1);
        assert_eq!(b1.top_agent_name.as_deref(), Some("solo-agent"));

        // Buckets must come back sorted ascending by time.
        assert!(buckets[0].bucket_start < buckets[1].bucket_start);
    }

    #[tokio::test]
    async fn spend_timeseries_agent_scoped_uses_the_agent_session_query_not_the_global_one() {
        let mut server = mockito::Server::new_async().await;
        let expected_query = agent_session_model_query("one-agent", None);
        let _search = server
            .mock("GET", "/api/search")
            .match_query(mockito::Matcher::UrlEncoded("q".into(), expected_query))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(search_response_with_n_traces(0).to_string())
            .create_async()
            .await;

        let provider = provider_against(&server.url());
        let now = Utc::now();
        let buckets = provider
            .spend_timeseries(
                Some("one-agent"),
                None,
                now - Duration::hours(1),
                now,
                TimeBucket::Hour,
            )
            .await
            .unwrap();
        assert!(buckets.is_empty(), "no traces in the (empty) search result");
    }

    /// Regression test for a real bug caught only against live infra (a
    /// single-span mock trace can't reproduce it): an orchestrator-routed
    /// trace's FIRST batch/span is the orchestrator's own root dispatch span
    /// (no `gen_ai.usage.*`), with the real agent's token-bearing span
    /// deeper in the tree, in a SEPARATE resource batch. `top_agent` must
    /// resolve to the real agent, not the orchestrator.
    #[tokio::test]
    async fn spend_timeseries_attributes_spend_to_the_token_bearing_span_not_the_trace_root() {
        let mut server = mockito::Server::new_async().await;
        let ts = "1700000000000000000";
        let _search = server
            .mock("GET", "/api/search")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(search_response_with_n_traces(1).to_string())
            .create_async()
            .await;
        let _trace = server
            .mock("GET", mockito::Matcher::Regex(r"^/api/traces/.*$".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "batches": [
                        {
                            "resource": {"attributes": [
                                {"key": "service.name", "value": {"stringValue": "nasiko-cp"}},
                            ]},
                            "scopeSpans": [{"spans": [{
                                "spanId": "AAAAAAAAAAE=",
                                "name": "a2a.dispatch",
                                "kind": "SPAN_KIND_SERVER",
                                "startTimeUnixNano": ts,
                                "attributes": [
                                    {"key": "gen_ai.operation.name", "value": {"stringValue": "invoke_agent"}},
                                ],
                            }]}],
                        },
                        {
                            "resource": {"attributes": [
                                {"key": "service.name", "value": {"stringValue": "real-downstream-agent"}},
                            ]},
                            "scopeSpans": [{"spans": [{
                                "spanId": "AAAAAAAAAAI=",
                                "parentSpanId": "AAAAAAAAAAE=",
                                "name": "chat",
                                "kind": "SPAN_KIND_INTERNAL",
                                "startTimeUnixNano": ts,
                                "attributes": [
                                    {"key": "gen_ai.usage.input_tokens", "value": {"intValue": 500}},
                                    {"key": "gen_ai.usage.output_tokens", "value": {"intValue": 200}},
                                    {"key": "gen_ai.request.model", "value": {"stringValue": "gpt-4o-mini"}},
                                ],
                            }]}],
                        },
                    ],
                })
                .to_string(),
            )
            .create_async()
            .await;

        let provider = provider_against(&server.url());
        let start = DateTime::<Utc>::from_timestamp(1_700_000_000 - 60, 0).unwrap();
        let end = start + Duration::hours(2);
        let buckets = provider
            .spend_timeseries(None, None, start, end, TimeBucket::Hour)
            .await
            .unwrap();

        assert_eq!(buckets.len(), 1);
        assert_eq!(
            buckets[0].top_agent_name.as_deref(),
            Some("real-downstream-agent"),
            "must attribute to the token-bearing span's service, not the orchestrator root span"
        );
        assert!(buckets[0].spend_usd > 0.0);
    }

    /// Builds a minimal, valid OTLP JSON `/api/traces/{id}` response body with
    /// one span carrying `service.name` on the resource and GenAI token/model
    /// attributes on the span — enough for `token_totals()`/`extract_token_attrs`
    /// to resolve real numbers, matching the real Tempo wire shape exactly
    /// (verified against `tempo.rs`'s `OtlpTraceResponse`/`OtlpSpan` structs).
    fn otlp_trace_json(
        service_name: &str,
        start_time_unix_nano: &str,
        input_tokens: u64,
        output_tokens: u64,
        model: Option<&str>,
    ) -> serde_json::Value {
        let mut attributes = vec![
            serde_json::json!({"key": "gen_ai.usage.input_tokens", "value": {"intValue": input_tokens}}),
            serde_json::json!({"key": "gen_ai.usage.output_tokens", "value": {"intValue": output_tokens}}),
        ];
        if let Some(m) = model {
            attributes.push(
                serde_json::json!({"key": "gen_ai.request.model", "value": {"stringValue": m}}),
            );
        }
        serde_json::json!({
            "batches": [{
                "resource": {
                    "attributes": [
                        {"key": "service.name", "value": {"stringValue": service_name}},
                    ],
                },
                "scopeSpans": [{
                    "spans": [{
                        "spanId": "AAAAAAAAAAE=",
                        "name": "chat",
                        "kind": "SPAN_KIND_INTERNAL",
                        "startTimeUnixNano": start_time_unix_nano,
                        "attributes": attributes,
                    }],
                }],
            }],
        })
    }
}
