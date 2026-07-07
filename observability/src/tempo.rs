use std::collections::HashMap;

use base64::Engine as _;
use chrono::{ DateTime, Utc };
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

use crate::error::ObservabilityError;
use crate::types::{ Span, TraceDetails };

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
    code: Option<u8>,
    message: Option<String>,
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
        limit: usize
    ) -> Result<Vec<(String, Option<DateTime<Utc>>, Option<u64>)>, ObservabilityError> {
        let url = format!("{}/api/search", self.base_url);
        let mut params: Vec<(&str, String)> = vec![
            ("q", query.to_string()),
            ("limit", limit.to_string())
        ];
        if let Some(s) = start {
            params.push(("start", s.timestamp().to_string()));
        }
        if let Some(e) = end {
            params.push(("end", e.timestamp().to_string()));
        }

        let resp = self.client
            .get(&url)
            .query(&params)
            .send().await
            .map_err(|e| ObservabilityError::TempoError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ObservabilityError::TempoError(format!("HTTP {status}: {body}")));
        }

        let search_resp: TempoSearchResponse = resp
            .json().await
            .map_err(|e| ObservabilityError::Deserialization(e.to_string()))?;

        let results = search_resp.traces
            .unwrap_or_default()
            .into_iter()
            .map(|t| {
                let started_at = t.start_time_unix_nano.as_deref().and_then(parse_nanos_str);
                (t.trace_id, started_at, t.duration_ms)
            })
            .collect();

        Ok(results)
    }

    /// Fetch a full trace in OTLP JSON format.
    pub async fn get_trace(&self, trace_id: &str) -> Result<TraceDetails, ObservabilityError> {
        let url = format!("{}/api/traces/{}", self.base_url, trace_id);

        let resp = self.client
            .get(&url)
            .header("Accept", "application/json")
            .send().await
            .map_err(|e| ObservabilityError::TempoError(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ObservabilityError::NotFound(trace_id.to_string()));
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ObservabilityError::TempoError(format!("HTTP {status}: {body}")));
        }

        let otlp: OtlpTraceResponse = resp
            .json().await
            .map_err(|e| ObservabilityError::Deserialization(e.to_string()))?;

        parse_otlp_trace(trace_id, otlp)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// OTLP protobuf JSON encodes spanId/parentSpanId as base64.
/// Convert to lowercase hex so it matches the format Loki uses.
/// If decoding fails (e.g. already hex), return the input unchanged.
fn otlp_id_to_hex(id: &str) -> String {
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
            && let Ok(n) = s.parse::<u64>() {
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
    otlp: OtlpTraceResponse
) -> Result<TraceDetails, ObservabilityError> {
    let mut spans = Vec::new();

    for batch in &otlp.batches {
        let service_name = batch.resource
            .as_ref()
            .map(|r| extract_service_name(&r.attributes))
            .unwrap_or_default();

        for scope_spans in batch.scope_spans.as_deref().unwrap_or(&[]) {
            for span in &scope_spans.spans {
                let started_at = parse_nanos_str(&span.start_time_unix_nano).unwrap_or_default();
                let ended_at = span.end_time_unix_nano.as_deref().and_then(parse_nanos_str);

                let duration_ms = ended_at.map(
                    |e| (e - started_at).num_milliseconds().max(0) as u64
                );

                let attributes: HashMap<String, Value> = span.attributes
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| (a.key.clone(), otlp_attr_to_json(&a.value)))
                    .collect();

                spans.push(Span {
                    span_id: otlp_id_to_hex(&span.span_id),
                    parent_span_id: span.parent_span_id.as_deref().map(otlp_id_to_hex),
                    name: span.name.clone(),
                    kind: parse_span_kind(span.kind.as_deref()),
                    status_code: span.status
                        .as_ref()
                        .and_then(|s| s.code)
                        .unwrap_or(0),
                    status_message: span.status
                        .as_ref()
                        .and_then(|s| s.message.clone())
                        .unwrap_or_default(),
                    started_at,
                    ended_at,
                    duration_ms,
                    service_name: service_name.clone(),
                    attributes,
                });
            }
        }
    }

    let started_at = spans
        .iter()
        .map(|s| s.started_at)
        .min();
    let ended_at = spans
        .iter()
        .filter_map(|s| s.ended_at)
        .max();
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
