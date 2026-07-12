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
    /// Tries three LogQL queries in order (different traceid field name conventions used
    /// by different OTel Collector/SDK combinations) and merges results.
    pub async fn get_trace_logs(
        &self,
        service_name: &str,
        trace_id: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
    ) -> Result<Vec<String>, ObservabilityError> {
        // Different OTel exporters use different field names for the trace ID in JSON logs.
        let queries = [
            format!(r#"{{service_name="{service_name}"}} | json | traceid = "{trace_id}""#),
            format!(r#"{{service_name="{service_name}"}} | json | trace_id = "{trace_id}""#),
            format!(r#"{{service_name="{service_name}"}} | json | traceId = "{trace_id}""#),
        ];

        let mut all_lines: Vec<String> = Vec::new();
        let mut last_err: Option<ObservabilityError> = None;

        for query in &queries {
            match self.query_range(query, start, end, 500).await {
                Ok(entries) => {
                    all_lines.extend(entries.into_iter().map(|(_, line)| line));
                }
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }

        if all_lines.is_empty()
            && let Some(e) = last_err
        {
            return Err(e);
        }

        Ok(all_lines)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Prompt/completion content captured for one span.
#[derive(Debug, Clone, Default)]
pub struct SpanContent {
    pub input: Option<String>,
    pub output: Option<String>,
}

/// Parse raw Loki log lines (OTel log-record JSON) into a map of
/// span_id → prompt/completion content.
///
/// Handles two formats:
/// - v1 (OTLP protobuf JSON): `spanId`/`span_id`, `attributes` as array of
///   `{key, value: {stringValue}}` objects, discriminated by `event.name`.
/// - v2 (`opentelemetry-instrumentation-openai-v2` EventLogger): `spanid`
///   (lowercase), `attributes` as flat JSON object, content in
///   `gen_ai.input.messages` / `gen_ai.output.messages`.
pub fn parse_trace_logs(lines: Vec<String>) -> std::collections::HashMap<String, SpanContent> {
    use serde_json::Value;
    let mut by_span: std::collections::HashMap<String, SpanContent> =
        std::collections::HashMap::new();

    for line in lines {
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        // v2 uses lowercase "spanid"; v1 uses "spanId" or "span_id"
        let span_id = entry["spanid"]
            .as_str()
            .or_else(|| entry["spanId"].as_str())
            .or_else(|| entry["span_id"].as_str())
            .unwrap_or("")
            .to_string();
        if span_id.is_empty() {
            continue;
        }

        let slot = by_span.entry(span_id).or_default();

        match &entry["attributes"] {
            // v2: flat object — gen_ai.input.messages / gen_ai.output.messages
            Value::Object(attrs) => {
                if let Some(v) = attrs.get("gen_ai.input.messages") {
                    let s = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    if !s.is_empty() {
                        slot.input = Some(s);
                    }
                }
                if let Some(v) = attrs.get("gen_ai.output.messages") {
                    let s = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    if !s.is_empty() {
                        slot.output = Some(s);
                    }
                }
            }
            // v1: OTLP array — discriminated by event.name
            Value::Array(attrs) => {
                let mut event_name: Option<&str> = None;
                let mut prompt_content: Option<String> = None;
                let mut completion_content: Option<String> = None;

                for attr in attrs {
                    let key = attr["key"].as_str().unwrap_or("");
                    let val = attr["value"]["stringValue"].as_str().unwrap_or("");
                    match key {
                        "event.name" => event_name = attr["value"]["stringValue"].as_str(),
                        "gen_ai.content.prompt" | "gen_ai.prompt" => {
                            prompt_content = Some(val.to_string());
                        }
                        "gen_ai.content.completion" | "gen_ai.completion" => {
                            completion_content = Some(val.to_string());
                        }
                        _ => {}
                    }
                }

                match event_name {
                    Some("gen_ai.content.prompt") => {
                        if let Some(c) = prompt_content {
                            slot.input = Some(c);
                        }
                    }
                    Some("gen_ai.content.completion") => {
                        if let Some(c) = completion_content {
                            slot.output = Some(c);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    by_span
}

fn parse_loki_timestamp(ts_str: &str) -> DateTime<Utc> {
    let nanos: i64 = ts_str.parse().unwrap_or(0);
    let secs = nanos / 1_000_000_000;
    let nsecs = (nanos % 1_000_000_000) as u32;
    DateTime::from_timestamp(secs, nsecs).unwrap_or_default()
}