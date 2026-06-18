use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

use crate::error::ObservabilityError;

// ---------------------------------------------------------------------------
// Loki query_range API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LokiQueryResponse {
    data: LokiData,
}

#[derive(Debug, Deserialize)]
struct LokiData {
    result: Vec<LokiStream>,
}

#[derive(Debug, Deserialize)]
struct LokiStream {
    /// Each element is `[timestamp_ns_string, log_line]`.
    values: Vec<[String; 2]>,
}

// ---------------------------------------------------------------------------
// LokiClient
// ---------------------------------------------------------------------------

pub struct LokiClient {
    client: Client,
    base_url: String,
}

impl LokiClient {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    /// Execute a LogQL `query_range` and return `(timestamp, log_line)` pairs
    /// sorted by timestamp ascending.
    pub async fn query_range(
        &self,
        query: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<(DateTime<Utc>, String)>, ObservabilityError> {
        let now = Utc::now();
        let start_ns = start
            .unwrap_or_else(|| now - chrono::Duration::hours(1))
            .timestamp_nanos_opt()
            .unwrap_or(0)
            .to_string();
        let end_ns = end
            .unwrap_or(now)
            .timestamp_nanos_opt()
            .unwrap_or(0)
            .to_string();

        let url = format!("{}/loki/api/v1/query_range", self.base_url);
        let resp = self
            .client
            .get(&url)
            .query(&[
                ("query", query),
                ("start", &start_ns),
                ("end", &end_ns),
                ("limit", &limit.to_string()),
                ("direction", "forward"),
            ])
            .send()
            .await
            .map_err(|e| ObservabilityError::LokiError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ObservabilityError::LokiError(format!(
                "HTTP {status}: {body}"
            )));
        }

        let loki_resp: LokiQueryResponse = resp
            .json()
            .await
            .map_err(|e| ObservabilityError::Deserialization(e.to_string()))?;

        let mut entries: Vec<(DateTime<Utc>, String)> = loki_resp
            .data
            .result
            .into_iter()
            .flat_map(|stream| {
                stream.values.into_iter().map(|[ts_str, line]| {
                    let ts = parse_loki_timestamp(&ts_str);
                    (ts, line)
                })
            })
            .collect();

        entries.sort_by_key(|(ts, _)| *ts);
        Ok(entries)
    }

    /// Fetch all log lines for a given `trace_id` from a specific Loki service stream.
    ///
    /// Uses the LogQL query: `{service_name="<name>"} | json | traceid = "<trace_id>"`.
    pub async fn get_trace_logs(
        &self,
        service_name: &str,
        trace_id: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
    ) -> Result<Vec<String>, ObservabilityError> {
        let query = format!(
            r#"{{service_name="{service_name}"}} | json | traceid = "{trace_id}""#
        );
        let entries = self.query_range(&query, start, end, 500).await?;
        Ok(entries.into_iter().map(|(_, line)| line).collect())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_loki_timestamp(ts_str: &str) -> DateTime<Utc> {
    let nanos: i64 = ts_str.parse().unwrap_or(0);
    let secs = nanos / 1_000_000_000;
    let nsecs = (nanos % 1_000_000_000) as u32;
    DateTime::from_timestamp(secs, nsecs).unwrap_or_default()
}