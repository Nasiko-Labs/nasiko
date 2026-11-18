use std::collections::{HashMap, HashSet};

use base64::Engine as _;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

use crate::error::ObservabilityError;
use crate::types::{Span, SpanEvent, TraceDetails};

// ---------------------------------------------------------------------------
// Tempo search API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TempoSearchResponse {
    traces: Option<Vec<TempoTraceSearchResult>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TempoTraceSearchResult {
    #[serde(rename = "traceID")]
    trace_id: String,
    start_time_unix_nano: Option<String>,
    duration_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// OTLP JSON trace response types (GET /api/traces/{traceID})
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OtlpTraceResponse {
    batches: Vec<OtlpBatch>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OtlpBatch {
    resource: Option<OtlpResource>,
    scope_spans: Option<Vec<OtlpScopeSpans>>,
}

#[derive(Debug, Deserialize)]
struct OtlpResource {
    attributes: Vec<OtlpAttribute>,
}

#[derive(Debug, Deserialize)]
struct OtlpScopeSpans {
    spans: Vec<OtlpSpan>,
}

#[derive(Debug, Deserialize)]
struct OtlpStatus {
    /// OTLP JSON encodes status code as either an integer (0/1/2) or a string
    /// enum name ("STATUS_CODE_UNSET" / "STATUS_CODE_OK" / "STATUS_CODE_ERROR").
    /// Store as a generic Value so both formats deserialize without error.
    #[serde(default)]
    code: Value,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OtlpEvent {
    name: String,
    time_unix_nano: Option<String>,
    attributes: Option<Vec<OtlpAttribute>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OtlpSpan {
    span_id: String,
    parent_span_id: Option<String>,
    name: String,
    kind: Option<String>,
    status: Option<OtlpStatus>,
    start_time_unix_nano: String,
    end_time_unix_nano: Option<String>,
    attributes: Option<Vec<OtlpAttribute>>,
    events: Option<Vec<OtlpEvent>>,
}

#[derive(Debug, Deserialize)]
struct OtlpAttribute {
    key: String,
    value: OtlpAttributeValue,
}

/// OTLP attribute value — one of the typed variants in the protobuf JSON encoding.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OtlpAttributeValue {
    string_value: Option<String>,
    /// May be serialised as a JSON string (`"123"`) or number.
    int_value: Option<Value>,
    bool_value: Option<bool>,
    double_value: Option<f64>,
}

// ---------------------------------------------------------------------------
// TempoClient
// ---------------------------------------------------------------------------

/// One `search()` result: `(trace_id, started_at, duration_ms)`.
pub type TraceSearchResult = (String, Option<DateTime<Utc>>, Option<u64>);

pub struct TempoClient {
    client: Client,
    base_url: String,
}

impl TempoClient {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Search for traces using a TraceQL query.
    ///
    /// Returns `(trace_id, started_at, duration_ms)` tuples.
    pub async fn search(
        &self,
        query: &str,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<TraceSearchResult>, ObservabilityError> {
        let url = format!("{}/api/search", self.base_url);
        let mut params: Vec<(&str, String)> =
            vec![("q", query.to_string()), ("limit", limit.to_string())];
        if let Some(s) = start {
            params.push(("start", s.timestamp().to_string()));
        }
        if let Some(e) = end {
            params.push(("end", e.timestamp().to_string()));
        }

        let resp = self
            .client
            .get(&url)
            .query(&params)
            .send()
            .await
            .map_err(|e| ObservabilityError::TempoError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ObservabilityError::TempoError(format!(
                "HTTP {status}: {body}"
            )));
        }

        let search_resp: TempoSearchResponse = resp
            .json()
            .await
            .map_err(|e| ObservabilityError::Deserialization(e.to_string()))?;

        let mut results = Vec::new();
        let mut seen = HashSet::new();
        for t in search_resp.traces.unwrap_or_default() {
            let id = normalize_trace_id(&t.trace_id);
            if seen.insert(id.clone()) {
                let started_at = t.start_time_unix_nano.as_deref().and_then(parse_nanos_str);
                results.push((id, started_at, t.duration_ms));
            }
        }

        Ok(results)
    }

    /// Fetch a full trace in OTLP JSON format.
    ///
    /// Retries up to 2 times on HTTP 429 (Tempo job queue full) with
    /// exponential backoff (200ms, 600ms) to avoid cascading failures.
    pub async fn get_trace(&self, trace_id: &str) -> Result<TraceDetails, ObservabilityError> {
        let normalized = normalize_trace_id(trace_id);
        let trace_id = normalized.as_str();
        let url = format!("{}/api/traces/{}", self.base_url, trace_id);

        let mut backoff = std::time::Duration::from_millis(200);
        let max_retries = 2u32;

        for attempt in 0..=max_retries {
            let resp = self
                .client
                .get(&url)
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|e| ObservabilityError::TempoError(e.to_string()))?;

            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                return Err(ObservabilityError::NotFound(trace_id.to_string()));
            }
            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < max_retries {
                tokio::time::sleep(backoff).await;
                backoff *= 3;
                continue;
            }
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(ObservabilityError::TempoError(format!(
                    "HTTP {status}: {body}"
                )));
            }

            let otlp: OtlpTraceResponse = resp
                .json()
                .await
                .map_err(|e| ObservabilityError::Deserialization(e.to_string()))?;

            return parse_otlp_trace(trace_id, otlp);
        }

        unreachable!()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Canonical spelling for trace IDs from Tempo and the session index.
///
/// Tempo's `/api/search` endpoint returns `traceID` as a plain hex string with
/// leading zero *nibbles* stripped (a real, observed Tempo behavior — unlike
/// `/api/traces/{id}`'s OTLP JSON, which encodes IDs as base64 bytes that
/// `otlp_id_to_hex` re-derives correctly via `hex::encode`, always exactly 32
/// chars for a 128-bit trace ID). Without re-padding, a trace whose ID happens
/// to start with `0` comes back as 31 (or fewer) hex chars — a different string
/// than the same trace's ID everywhere else it's used (traceparent headers,
/// `get_trace` calls), so it silently becomes a second, duplicate row wherever
/// trace ID is used as a dedup/primary key (confirmed: this caused doubled rows,
/// and inflated totals, in `trace_usage`).
///
/// Only strings that are actually hex of at most 32 chars are touched — anything
/// else (a test fixture, or some future non-hex ID Tempo returns) is left alone
/// rather than corrupted into a 32-char string that matches nothing.
pub(crate) fn normalize_trace_id(id: &str) -> String {
    if !id.is_empty() && id.len() <= 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        format!("{id:0>32}").to_ascii_lowercase()
    } else {
        id.to_owned()
    }
}

/// OTLP protobuf JSON encodes spanId/parentSpanId as base64.
/// Convert to lowercase hex so it matches the format Loki uses.
/// If decoding fails (e.g. already hex), return the input unchanged.
fn otlp_id_to_hex(id: &str) -> String {
    if id.len() == 16 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return id.to_ascii_lowercase();
    }
    base64::engine::general_purpose::STANDARD
        .decode(id)
        .map(hex::encode)
        .unwrap_or_else(|_| id.to_string())
}

fn parse_nanos_str(s: &str) -> Option<DateTime<Utc>> {
    let nanos: i64 = s.parse().ok()?;
    let secs = nanos / 1_000_000_000;
    let nsecs = (nanos % 1_000_000_000) as u32;
    DateTime::from_timestamp(secs, nsecs)
}

fn otlp_attr_to_json(v: &OtlpAttributeValue) -> Value {
    if let Some(s) = &v.string_value {
        return Value::String(s.clone());
    }

    if let Some(i) = &v.int_value {
        if let Some(n) = i.as_u64() {
            return Value::Number(n.into());
        }

        if let Some(s) = i.as_str()
            && let Ok(n) = s.parse::<u64>()
        {
            return Value::Number(n.into());
        }

        return i.clone();
    }

    if let Some(b) = v.bool_value {
        return Value::Bool(b);
    }

    if let Some(d) = v.double_value {
        return serde_json::json!(d);
    }

    Value::Null
}

/// OTLP status code: integer (0/1/2) or string enum name.
fn parse_status_code(v: &Value) -> u8 {
    match v {
        Value::Number(n) => n.as_u64().unwrap_or(0) as u8,
        Value::String(s) => match s.as_str() {
            "STATUS_CODE_OK" => 1,
            "STATUS_CODE_ERROR" => 2,
            _ => 0,
        },
        _ => 0,
    }
}

fn parse_span_kind(kind: Option<&str>) -> u8 {
    match kind {
        Some("SPAN_KIND_INTERNAL") => 1,
        Some("SPAN_KIND_SERVER") => 2,
        Some("SPAN_KIND_CLIENT") => 3,
        Some("SPAN_KIND_PRODUCER") => 4,
        Some("SPAN_KIND_CONSUMER") => 5,
        _ => 0,
    }
}

fn extract_service_name(attrs: &[OtlpAttribute]) -> String {
    attrs
        .iter()
        .find(|a| a.key == "service.name")
        .and_then(|a| a.value.string_value.clone())
        .unwrap_or_default()
}

fn parse_otlp_trace(
    trace_id: &str,
    otlp: OtlpTraceResponse,
) -> Result<TraceDetails, ObservabilityError> {
    let mut spans = Vec::new();
    let mut seen_span_ids = HashSet::new();

    for batch in &otlp.batches {
        let service_name = batch
            .resource
            .as_ref()
            .map(|r| extract_service_name(&r.attributes))
            .unwrap_or_default();
        // A resource attribute, so it is copied onto each span: `Span` keeps only the service
        // name from the resource, and the service name is the agent's name, not unique.
        let agent_uuid = batch.resource.as_ref().and_then(|r| {
            r.attributes
                .iter()
                .find(|a| a.key == crate::injector::AGENT_UUID_RESOURCE_ATTR)
                .and_then(|a| a.value.string_value.clone())
        });

        for scope_spans in batch.scope_spans.as_deref().unwrap_or(&[]) {
            for span in &scope_spans.spans {
                let span_id = otlp_id_to_hex(&span.span_id);
                if !seen_span_ids.insert(span_id.clone()) {
                    continue;
                }
                let started_at = parse_nanos_str(&span.start_time_unix_nano).unwrap_or_default();
                let ended_at = span.end_time_unix_nano.as_deref().and_then(parse_nanos_str);

                let duration_ms =
                    ended_at.map(|e| (e - started_at).num_milliseconds().max(0) as u64);

                let mut attributes: HashMap<String, Value> = span
                    .attributes
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| (a.key.clone(), otlp_attr_to_json(&a.value)))
                    .collect();
                if let Some(uuid) = &agent_uuid {
                    attributes
                        .entry(crate::injector::AGENT_UUID_RESOURCE_ATTR.to_string())
                        .or_insert_with(|| Value::String(uuid.clone()));
                }

                let events: Vec<SpanEvent> = span
                    .events
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|e| SpanEvent {
                        name: e.name.clone(),
                        timestamp: e.time_unix_nano.as_deref().and_then(parse_nanos_str),
                        attributes: e
                            .attributes
                            .as_deref()
                            .unwrap_or(&[])
                            .iter()
                            .map(|a| (a.key.clone(), otlp_attr_to_json(&a.value)))
                            .collect(),
                    })
                    .collect();

                spans.push(Span {
                    span_id,
                    parent_span_id: span.parent_span_id.as_deref().map(otlp_id_to_hex),
                    name: span.name.clone(),
                    kind: parse_span_kind(span.kind.as_deref()),
                    status_code: span
                        .status
                        .as_ref()
                        .map(|s| parse_status_code(&s.code))
                        .unwrap_or(0),
                    status_message: span
                        .status
                        .as_ref()
                        .and_then(|s| s.message.clone())
                        .unwrap_or_default(),
                    started_at,
                    ended_at,
                    duration_ms,
                    service_name: service_name.clone(),
                    attributes,
                    events,
                });
            }
        }
    }

    let started_at = spans.iter().map(|s| s.started_at).min();
    let ended_at = spans.iter().filter_map(|s| s.ended_at).max();
    let duration_ms = match (started_at, ended_at) {
        (Some(s), Some(e)) => Some((e - s).num_milliseconds().max(0) as u64),
        _ => None,
    };

    Ok(TraceDetails {
        trace_id: trace_id.to_string(),
        spans,
        started_at,
        ended_at,
        duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{OtlpTraceResponse, normalize_trace_id, parse_otlp_trace};

    #[test]
    fn normalize_trace_id_restores_a_stripped_leading_zero() {
        assert_eq!(
            normalize_trace_id("81423c37451c04a19701dbf92626ee4"),
            "081423c37451c04a19701dbf92626ee4"
        );
    }

    #[test]
    fn normalize_trace_id_leaves_a_full_length_id_unchanged() {
        let full = "d505d94088a7d0fdc5c7a32bb790ee26";
        assert_eq!(normalize_trace_id(full), full);
    }

    #[test]
    fn normalize_trace_id_leaves_non_hex_ids_unchanged() {
        // Not a real trace ID's shape (e.g. a test fixture) — must not be
        // corrupted into a 32-char string that matches nothing real.
        assert_eq!(normalize_trace_id("t-cheap"), "t-cheap");
    }

    #[test]
    fn hex_span_ids_are_not_misdecoded_as_base64() {
        assert_eq!(
            super::otlp_id_to_hex("abcdef0123456789"),
            "abcdef0123456789"
        );
        assert_eq!(super::otlp_id_to_hex("q83vASNFZ4k="), "abcdef0123456789");
    }

    #[test]
    fn otlp_replayed_span_is_emitted_and_counted_once() {
        let span = json!({
            "spanId": "span-1",
            "name": "ChatCompletion",
            "startTimeUnixNano": "1724068800000000000",
            "endTimeUnixNano": "1724068801000000000",
            "attributes": [
                {"key": "gen_ai.usage.input_tokens", "value": {"intValue": "25"}},
                {"key": "gen_ai.usage.output_tokens", "value": {"intValue": "5"}}
            ],
            "events": []
        });
        let otlp: OtlpTraceResponse = serde_json::from_value(json!({
            "batches": [{
                "resource": {"attributes": []},
                "scopeSpans": [{"spans": [span.clone(), span]}]
            }]
        }))
        .unwrap();

        let trace = parse_otlp_trace("trace-1", otlp).unwrap();

        assert_eq!(trace.spans.len(), 1);
        assert_eq!(trace.token_totals(), (25, 5, None));
    }

    #[test]
    fn the_workload_uuid_on_the_resource_reaches_each_span() {
        let otlp: OtlpTraceResponse = serde_json::from_value(json!({
            "batches": [{
                "resource": {"attributes": [
                    {"key": "service.name", "value": {"stringValue": "meeting-notes"}},
                    {"key": "nasiko.agent.uuid", "value": {"stringValue": "f546d9c0-78d6-4a0c-ac83-002b82ea3615"}}
                ]},
                "scopeSpans": [{"spans": [{
                    "spanId": "span-1",
                    "name": "chat",
                    "startTimeUnixNano": "1724068800000000000",
                    "attributes": [],
                    "events": []
                }]}]
            }]
        }))
        .unwrap();

        let trace = parse_otlp_trace("trace-1", otlp).unwrap();

        assert_eq!(trace.spans[0].service_name, "meeting-notes");
        assert_eq!(
            trace.spans[0].attributes["nasiko.agent.uuid"],
            json!("f546d9c0-78d6-4a0c-ac83-002b82ea3615")
        );
    }
}
