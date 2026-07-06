use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::ObservabilityError;
use crate::loki::LokiClient;
use crate::tempo::TempoClient;
use crate::types::{AgentFinOps, AgentStats, FinOpsDashboard, Session, SpanDetails, TokenUsage, TraceDetails};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstracts access to distributed trace and log data.
///
/// OSS impl: [`TempoLokiProvider`] — queries Tempo and Loki directly.
/// EE impl: `RbacObservabilityProvider` — wraps the OSS impl with RBAC filtering.
#[async_trait]
pub trait ObservabilityProvider: Send + Sync {
    /// List recent sessions (traces) across one or more agents.
    ///
    /// Results are sorted by `started_at` descending and capped at `limit`.
    async fn list_sessions(
        &self,
        agent_ids: &[String],
        start_time: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<Session>, ObservabilityError>;

    /// Fetch full trace with all spans.
    async fn get_trace(&self, trace_id: &str) -> Result<TraceDetails, ObservabilityError>;

    /// Fetch a single span, enriched with Loki prompt/completion content if available.
    ///
    /// `trace_id` is required because Tempo has no standalone span-search endpoint.
    async fn get_span(
        &self,
        trace_id: &str,
        span_id: &str,
    ) -> Result<SpanDetails, ObservabilityError>;

    /// Aggregate performance stats for one agent over the given time window.
    async fn get_agent_stats(
        &self,
        agent_id: &str,
        start_time: DateTime<Utc>,
    ) -> Result<AgentStats, ObservabilityError>;

    /// Aggregate token usage and cost across multiple agents for the FinOps dashboard.
    async fn get_finops_dashboard(
        &self,
        agent_ids: &[String],
        start_time: Option<DateTime<Utc>>,
    ) -> Result<FinOpsDashboard, ObservabilityError>;

    /// Query raw log lines for an agent by Loki service name.
    ///
    /// Returns `(timestamp, log_line)` pairs sorted ascending.
    /// Used by the `/observe/agents/{id}/logs` endpoint to include Loki-sourced log lines.
    async fn query_logs(
        &self,
        service_name: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<(DateTime<Utc>, String)>, ObservabilityError>;
}

// ---------------------------------------------------------------------------
// TempoLokiProvider — OSS implementation
// ---------------------------------------------------------------------------

pub struct TempoLokiProvider {
    tempo: TempoClient,
    loki: LokiClient,
}

impl TempoLokiProvider {
    pub fn new(tempo_url: String, loki_url: String) -> Self {
        Self {
            tempo: TempoClient::new(tempo_url),
            loki: LokiClient::new(loki_url),
        }
    }
}

#[async_trait]
impl ObservabilityProvider for TempoLokiProvider {
    async fn list_sessions(
        &self,
        agent_ids: &[String],
        start_time: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<Session>, ObservabilityError> {
        let mut sessions = Vec::new();

        for agent_id in agent_ids {
            let query = format!(r#"{{ resource.service.name = "{agent_id}" }}"#);
            let results = self.tempo.search(&query, start_time, None, limit).await?;

            for (trace_id, started_at, duration_ms) in results {
                let trace = self.tempo.get_trace(&trace_id).await?;
                let token_usage = trace.token_usage();
                let span_count = trace.spans.len() as u32;

                let ended_at = started_at.zip(duration_ms).map(|(s, d)| {
                    s + chrono::Duration::milliseconds(d as i64)
                });

                sessions.push(Session {
                    trace_id,
                    agent_ids: vec![agent_id.clone()],
                    started_at: started_at.unwrap_or_default(),
                    ended_at,
                    duration_ms,
                    span_count,
                    total_input_tokens: token_usage.input_tokens,
                    total_output_tokens: token_usage.output_tokens,
                });
            }
        }

        sessions.sort_by_key(|s| std::cmp::Reverse(s.started_at));
        sessions.truncate(limit);
        Ok(sessions)
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
            .into_iter()
            .find(|s| s.span_id == span_id)
            .ok_or_else(|| ObservabilityError::NotFound(span_id.to_string()))?;

        // Best-effort: fetch Loki logs. Missing logs are not an error.
        let logs = self
            .loki
            .get_trace_logs(&span.service_name, trace_id, None, None)
            .await
            .unwrap_or_default();

        let prompt_content = logs
            .iter()
            .find(|l| l.contains("\"prompt\""))
            .cloned();
        let completion_content = logs
            .iter()
            .find(|l| l.contains("\"completion\""))
            .cloned();

        Ok(SpanDetails {
            span,
            prompt_content,
            completion_content,
        })
    }

    async fn get_agent_stats(
        &self,
        agent_id: &str,
        start_time: DateTime<Utc>,
    ) -> Result<AgentStats, ObservabilityError> {
        let query = format!(
            r#"{{ resource.service.name = "{agent_id}" && span.gen_ai.operation.name = "chat" }}"#
        );
        let results = self
            .tempo
            .search(&query, Some(start_time), None, 1000)
            .await?;

        let request_count = results.len() as u64;
        let mut total_input = 0u64;
        let mut total_output = 0u64;
        let mut total_cost = 0f64;
        let mut total_duration_ms = 0u64;
        let mut error_count = 0u64;

        for (trace_id, _, duration_ms) in &results {
            let trace = self.tempo.get_trace(trace_id).await?;
            let usage = trace.token_usage();
            total_input += usage.input_tokens;
            total_output += usage.output_tokens;
            total_cost += usage.estimated_cost_usd;
            if let Some(d) = duration_ms {
                total_duration_ms += d;
            }
            let has_error = trace.spans.iter().any(|s| {
                s.attributes
                    .get("otel.status_code")
                    .and_then(|v| v.as_str())
                    .map(|code| code == "ERROR")
                    .unwrap_or(false)
            });
            if has_error {
                error_count += 1;
            }
        }

        let avg_latency_ms = if request_count > 0 {
            total_duration_ms as f64 / request_count as f64
        } else {
            0.0
        };
        let error_rate = if request_count > 0 {
            error_count as f64 / request_count as f64
        } else {
            0.0
        };

        Ok(AgentStats {
            agent_id: agent_id.to_string(),
            total_requests: request_count,
            total_tokens: TokenUsage {
                input_tokens: total_input,
                output_tokens: total_output,
                total_tokens: total_input + total_output,
                estimated_cost_usd: total_cost,
            },
            avg_latency_ms,
            error_rate,
            period_start: start_time,
        })
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

    async fn get_finops_dashboard(
        &self,
        agent_ids: &[String],
        start_time: Option<DateTime<Utc>>,
    ) -> Result<FinOpsDashboard, ObservabilityError> {
        let period_start =
            start_time.unwrap_or_else(|| Utc::now() - chrono::Duration::days(30));
        let period_end = Utc::now();

        let mut agents = Vec::new();
        let mut total_input = 0u64;
        let mut total_output = 0u64;

        for agent_id in agent_ids {
            let query = format!(r#"{{ resource.service.name = "{agent_id}" }}"#);
            let results = self
                .tempo
                .search(&query, Some(period_start), None, 1000)
                .await?;

            let request_count = results.len() as u64;
            let mut input_tokens = 0u64;
            let mut output_tokens = 0u64;
            let mut estimated_cost = 0f64;

            for (trace_id, _, _) in &results {
                let trace = self.tempo.get_trace(trace_id).await?;
                let usage = trace.token_usage();
                input_tokens += usage.input_tokens;
                output_tokens += usage.output_tokens;
                estimated_cost += usage.estimated_cost_usd;
            }

            total_input += input_tokens;
            total_output += output_tokens;

            agents.push(AgentFinOps {
                agent_id: agent_id.clone(),
                total_input_tokens: input_tokens,
                total_output_tokens: output_tokens,
                estimated_cost_usd: estimated_cost,
                request_count,
            });
        }

        let total_estimated_cost_usd = agents.iter().map(|a| a.estimated_cost_usd).sum();

        Ok(FinOpsDashboard {
            period_start,
            period_end,
            agents,
            total_input_tokens: total_input,
            total_output_tokens: total_output,
            total_estimated_cost_usd,
        })
    }
}